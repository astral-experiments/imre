//! Conservative closed-world roots for native compilation of a program entry.

use std::collections::HashSet;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};

use super::{Binding, DefinitionId, Intrinsic, ModuleId, SourceGraph};

#[derive(Default)]
pub(super) struct Reachable {
    pub(super) definitions: HashSet<DefinitionId>,
    pub(super) methods: HashSet<(ModuleId, TextSize)>,
    pub(super) skipped: Vec<Vec<TextRange>>,
    attributes: HashSet<String>,
    properties: HashSet<String>,
    class_attributes: HashSet<(DefinitionId, String)>,
    instance_attributes: HashSet<String>,
}

struct References<'a> {
    graph: &'a SourceGraph,
    module: ModuleId,
    found: &'a mut Reachable,
}

impl<'a> Visitor<'a> for References<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if !self
            .graph
            .entry_profile
            .ranges(self.module)
            .any(|range| range.contains_range(stmt.range()))
        {
            walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let Some(Binding::Definition(id)) = self.graph.resolve_type(self.module, expr) {
            self.found.definitions.insert(id);
        }
        if let ast::Expr::Attribute(attr) = expr {
            let name = attr.attr.to_string();
            if let Some(Binding::Definition(owner)) = self.graph.resolve(self.module, &attr.value) {
                self.found.class_attributes.insert((owner, name.clone()));
                self.found.attributes.insert(name);
            } else if self.graph.session.data_attribute(self.module, expr) {
                self.found.properties.insert(name);
            } else if !self.graph.session.sealed_native_method(self.module, attr) {
                self.found.instance_attributes.insert(name.clone());
                self.found.attributes.insert(name);
            }
        }
        walk_expr(self, expr);
    }
}

impl Reachable {
    pub(super) fn inherited_class_method(&self, owner: DefinitionId, name: &str) -> bool {
        self.instance_attributes.contains(name)
            || self.class_attributes.contains(&(owner, name.to_owned()))
    }
    pub(super) fn collect(graph: &SourceGraph) -> Self {
        let mut found = Self::default();
        for (module, source) in graph.modules.iter().enumerate() {
            for (statement, node) in source.parsed.syntax().body.iter().enumerate() {
                let id = DefinitionId { module, statement };
                match node {
                    ast::Stmt::FunctionDef(f) => {
                        if definition_effect(graph, module, f) {
                            found.definitions.insert(id);
                        }
                    }
                    ast::Stmt::ClassDef(c) => {
                        // An arbitrary class body or decorator can execute at import time.
                        if class_effect(graph, module, c) {
                            found.definitions.insert(id);
                        }
                    }
                    _ => References {
                        graph,
                        module,
                        found: &mut found,
                    }
                    .visit_stmt(node),
                }
            }
        }
        let mut expanded = HashSet::new();
        loop {
            let before = (
                found.definitions.len(),
                found.methods.len(),
                found.attributes.len(),
                found.properties.len(),
            );
            for id in found.definitions.iter().copied().collect::<Vec<_>>() {
                let source = &graph.modules[id.module];
                let node = &source.parsed.syntax().body[id.statement];
                let mut refs = References {
                    graph,
                    module: id.module,
                    found: &mut found,
                };
                if let ast::Stmt::ClassDef(class) = node {
                    if expanded.insert(id) {
                        if let Some(args) = &class.arguments {
                            refs.visit_arguments(args);
                        }
                        for decorator in &class.decorator_list {
                            refs.visit_decorator(decorator);
                        }
                        for field in &class.body {
                            if !matches!(field, ast::Stmt::FunctionDef(_)) {
                                refs.visit_stmt(field);
                            }
                        }
                    }
                    for item in &class.body {
                        if let ast::Stmt::FunctionDef(method) = item {
                            let name = method.name.as_str();
                            // These protocols have implicit call sites in the supported language.
                            // An unsupported operator still fails when checking its expression.
                            let property = refs.found.properties.contains(name) && method.decorator_list.iter().any(|d| matches!(&d.expression, ast::Expr::Name(n) if n.id == "property"));
                            if (property
                                || refs.found.attributes.contains(name)
                                || matches!(
                                    name,
                                    "__init__"
                                        | "__post_init__"
                                        | "__hash__"
                                        | "__bool__"
                                        | "__iter__"
                                        | "__str__"
                                )
                                || (name == "__repr__" && exception_base(graph, id))
                                || definition_effect(graph, id.module, method))
                                && refs.found.methods.insert((id.module, method.start()))
                            {
                                refs.visit_stmt(item);
                            }
                        }
                    }
                } else if expanded.insert(id) {
                    refs.visit_stmt(node);
                }
            }
            if before
                == (
                    found.definitions.len(),
                    found.methods.len(),
                    found.attributes.len(),
                    found.properties.len(),
                )
            {
                break;
            }
        }
        for (module, source) in graph.modules.iter().enumerate() {
            let mut skipped = graph.entry_profile.ranges(module).collect::<Vec<_>>();
            for (statement, node) in source.parsed.syntax().body.iter().enumerate() {
                let id = DefinitionId { module, statement };
                match node {
                    ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)
                        if !found.definitions.contains(&id) =>
                    {
                        skipped.push(node.range())
                    }
                    ast::Stmt::ClassDef(class) => {
                        for item in &class.body {
                            if let ast::Stmt::FunctionDef(method) = item
                                && !found.methods.contains(&(module, method.start()))
                            {
                                skipped.push(item.range());
                            }
                        }
                    }
                    _ => {}
                }
            }
            found.skipped.push(skipped);
        }
        found
    }
}

fn known_decorator(graph: &SourceGraph, module: ModuleId, expr: &ast::Expr) -> bool {
    let callee = if let ast::Expr::Call(call) = expr {
        &*call.func
    } else {
        expr
    };
    matches!(
        graph.resolve(module, callee),
        Some(Binding::Intrinsic(
            Intrinsic::Dataclass
                | Intrinsic::AbstractMethod
                | Intrinsic::ContextManager
                | Intrinsic::FunctionCache
        ))
    ) || matches!(expr, ast::Expr::Name(name) if matches!(name.id.as_str(), "property" | "classmethod" | "staticmethod") && graph.resolve(module, expr).is_none())
}

fn definition_effect(graph: &SourceGraph, module: ModuleId, func: &ast::StmtFunctionDef) -> bool {
    func.decorator_list
        .iter()
        .any(|d| !known_decorator(graph, module, &d.expression))
        || func
            .parameters
            .iter_non_variadic_params()
            .any(|p| p.default.as_deref().is_some_and(|d| !inert_default(d)))
}

/// Creating a lambda does not execute its body, but its own parameter defaults
/// still run immediately, even on a function that is never called.
fn inert_default(expr: &ast::Expr) -> bool {
    if let ast::Expr::Lambda(lambda) = expr {
        return lambda
            .parameters
            .iter()
            .flat_map(|p| p.iter_non_variadic_params())
            .all(|p| p.default.as_deref().is_none_or(inert_default));
    }
    literal(expr)
}

fn literal(expr: &ast::Expr) -> bool {
    if let ast::Expr::Tuple(tuple) = expr {
        return tuple.elts.iter().all(literal);
    }
    matches!(
        expr,
        ast::Expr::StringLiteral(_)
            | ast::Expr::NumberLiteral(_)
            | ast::Expr::BooleanLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::EllipsisLiteral(_)
    )
}

/// A bare dataclass with ordinary marker bases can create its fields without
/// running user code. Check positional default order: the decorator itself
/// raises when a later required positional field follows an optional one.
fn simple_dataclass(graph: &SourceGraph, module: ModuleId, class: &ast::StmtClassDef) -> bool {
    if !matches!(class.decorator_list.as_slice(), [d] if graph.resolve(module, &d.expression) == Some(Binding::Intrinsic(Intrinsic::Dataclass)))
    {
        return false;
    }
    let mut inherited_names = HashSet::new();
    if let Some(bases) = &class.arguments
        && (!bases.keywords.is_empty()
            || bases.args.iter().any(|base| {
                let Some(Binding::Definition(id)) = graph.resolve(module, base) else {
                    return true;
                };
                let ast::Stmt::ClassDef(base) =
                    &graph.modules[id.module].parsed.syntax().body[id.statement]
                else {
                    return true;
                };
                !base.decorator_list.is_empty()
                    || base.arguments.is_some()
                    || !base.body.iter().all(|stmt| match stmt {
                        ast::Stmt::Pass(_) => true,
                        ast::Stmt::Expr(expr) => {
                            matches!(&*expr.value, ast::Expr::StringLiteral(_))
                        }
                        ast::Stmt::FunctionDef(f) => {
                            inherited_names.insert(f.name.as_str());
                            f.name.as_str() != "__init_subclass__"
                        }
                        _ => false,
                    })
            }))
    {
        return false;
    }
    let mut had_default = false;
    let mut names = HashSet::new();
    for stmt in &class.body {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                if !names.insert(f.name.as_str()) {
                    return false;
                }
            }
            ast::Stmt::Assign(_) => return false,
            _ => {}
        }
        if let ast::Stmt::AnnAssign(field) = stmt {
            let ast::Expr::Name(name) = &*field.target else {
                return false;
            };
            if !names.insert(name.id.as_str())
                || inherited_names.contains(name.id.as_str())
                || !field.simple
                || !annotation_without_calls(&field.annotation)
            {
                return false;
            }
            let has_default = match field.value.as_deref() {
                None => false,
                Some(value) if literal(value) => true,
                Some(value) if empty_field(graph, module, value, &names) => false,
                _ => return false,
            };
            if had_default && !has_default {
                return false;
            }
            had_default |= has_default;
        }
    }
    true
}

fn annotation_without_calls(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Name(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::EllipsisLiteral(_) => true,
        ast::Expr::Attribute(attr) => annotation_without_calls(&attr.value),
        ast::Expr::Tuple(tuple) => tuple.elts.iter().all(annotation_without_calls),
        ast::Expr::Subscript(sub) => {
            annotation_without_calls(&sub.value) && annotation_without_calls(&sub.slice)
        }
        ast::Expr::BinOp(op) if op.op == ast::Operator::BitOr => {
            annotation_without_calls(&op.left) && annotation_without_calls(&op.right)
        }
        _ => false,
    }
}

/// These two flags only change generated method metadata. A default, a factory,
/// or a more general expression still requires checking the whole declaration.
fn empty_field(
    graph: &SourceGraph,
    module: ModuleId,
    expr: &ast::Expr,
    shadowed: &HashSet<&str>,
) -> bool {
    let ast::Expr::Call(call) = expr else {
        return false;
    };
    let root = match &*call.func {
        ast::Expr::Name(n) => Some(n.id.as_str()),
        ast::Expr::Attribute(a) => a.value.as_name_expr().map(|n| n.id.as_str()),
        _ => None,
    };
    let mut options = HashSet::new();
    root.is_some_and(|name| !shadowed.contains(name))
        && graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::DataclassField))
        && call.arguments.args.is_empty()
        && call.arguments.keywords.iter().all(|kw| {
            kw.arg.as_ref().is_some_and(|arg| {
                matches!(arg.as_str(), "repr" | "compare") && options.insert(arg.as_str())
            }) && matches!(kw.value, ast::Expr::BooleanLiteral(_))
        })
}

fn class_effect(graph: &SourceGraph, module: ModuleId, class: &ast::StmtClassDef) -> bool {
    let dataclass = class.decorator_list.iter().any(|d| {
        let expr = if let ast::Expr::Call(call) = &d.expression {
            &*call.func
        } else {
            &d.expression
        };
        graph.resolve(module, expr) == Some(Binding::Intrinsic(Intrinsic::Dataclass))
    });
    let verified = dataclass && simple_dataclass(graph, module, class);
    class
        .decorator_list
        .iter()
        .any(|d| !known_decorator(graph, module, &d.expression))
        || class.arguments.as_ref().is_some_and(|args| {
            !args.keywords.is_empty()
                || args
                    .args
                    .iter()
                    .any(|e| !matches!(e, ast::Expr::Name(_) | ast::Expr::Attribute(_)))
        })
        || class.body.iter().any(|s| match s {
            ast::Stmt::Pass(_) => false,
            ast::Stmt::Expr(s) => !matches!(&*s.value, ast::Expr::StringLiteral(_)),
            ast::Stmt::Assign(s) => !literal(&s.value),
            ast::Stmt::AnnAssign(s) => s.value.as_deref().is_some_and(|v| {
                // A tuple is a constant; dataclass must also accept the declaration.
                if matches!(v, ast::Expr::Tuple(_)) && dataclass {
                    !verified || !literal(v)
                } else {
                    !literal(v) && !(verified && matches!(v, ast::Expr::Call(_)))
                }
            }),
            ast::Stmt::FunctionDef(f) => definition_effect(graph, module, f),
            _ => true,
        })
}

/// Rich exception display overrides are reachable through base-class catches.
fn exception_base(graph: &SourceGraph, id: DefinitionId) -> bool {
    let mut pending = vec![id];
    let mut seen = HashSet::new();
    while let Some(def) = pending.pop() {
        if !seen.insert(def) {
            continue;
        }
        let ast::Stmt::ClassDef(c) = &graph.modules[def.module].parsed.syntax().body[def.statement]
        else {
            continue;
        };
        let Some(args) = &c.arguments else {
            continue;
        };
        for base in &args.args {
            match graph.resolve(def.module, base) {
                Some(Binding::Definition(parent)) => pending.push(parent),
                None if base
                    .as_name_expr()
                    .and_then(|n| crate::frontend::exception_kind(n.id.as_str()))
                    .is_some() =>
                {
                    return true;
                }
                _ => {}
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    fn reachable(source: &str) -> Result<String, String> {
        compile(
            source,
            &CompileOptions {
                reachable_only: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn follows_aliases_and_methods_transitively() {
        let source = r#"
class Used:
    def __init__(self) -> None:
        pass
    def value(self) -> int:
        return answer()
    def unused(self):
        return globals()

def answer() -> int:
    return 42

def cold():
    return globals()

def run() -> None:
    print(Used().value())

run()
"#;
        assert!(compile(source, &CompileOptions::default()).is_err());
        reachable(source).unwrap();
        assert!(
            reachable(&source.replace("print(Used().value())", "print(Used().unused())")).is_err()
        );
    }

    #[test]
    fn cold_lambda_defaults_preserve_definition_time_evaluation() {
        let source =
            "def cold(callback = lambda x: globals()):\n    return callback(0)\nprint(42)\n";
        reachable(source).unwrap();
        assert!(reachable(&source.replace("lambda x:", "lambda x=globals():")).is_err());
        assert!(reachable(&source.replace("print(42)", "print(cold())")).is_err());
    }

    #[test]
    fn bound_native_methods_do_not_retain_unrelated_methods_with_the_same_name() {
        let source = "from pathlib import Path\nclass Local:\n    def __init__(self) -> None:\n        pass\n    def exists(self):\n        return globals()\nlocal = Local()\nprint(Path('.').exists())\n";
        reachable(source).unwrap();
        assert!(reachable(&source.replace("Path('.').exists()", "local.exists()")).is_err());
    }

    #[test]
    fn sealed_imports_without_lowering_stay_confined_to_cold_bodies() {
        let source = "from configparser import ConfigParser as parser\ndef cold():\n    return parser()\nprint(42)\n";
        reachable(source).unwrap();
        assert!(compile(source, &CompileOptions::default()).is_err());
        assert!(
            reachable(&source.replace("print(42)", "print(cold())"))
                .unwrap_err()
                .contains("configparser.ConfigParser")
        );
        assert!(reachable(&source.replace("ConfigParser", "Missing")).is_err());
        assert!(reachable("import this\nprint(42)\n").is_err());
    }

    #[test]
    fn cold_tuple_defaults_and_field_options_do_not_hide_definition_errors() {
        let cold = "from dataclasses import dataclass, field\n@dataclass\nclass Cold:\n    key: int = field(repr=False)\n    items: tuple[int, ...] = ()\n    def unused(self):\n        return globals()\nprint(42)\n";
        reachable(cold).unwrap();
        assert!(
            reachable(&cold.replace("    def unused", "    required: str\n    def unused"))
                .is_err()
        );
        assert!(reachable(&cold.replace("repr=False", "repr=globals()")).is_err());
    }

    #[test]
    fn retains_definition_time_side_effects() {
        let source = "def unused(value=globals()):\n    pass\nprint(42)\n";
        assert!(reachable(source).is_err());
        let source = "def wrap(f):\n    return f\n@wrap\ndef unused():\n    pass\nprint(42)\n";
        assert!(reachable(source).is_err());
    }
}
