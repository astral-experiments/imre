//! Collects fixed class shapes before nominal type resolution and executable lowering.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_expr};

use crate::modules::{DefinitionId, SourceGraph, SourceModule};

pub(crate) struct Classes {
    pub(crate) definitions: Vec<Class>,
    pub(crate) ids: HashMap<DefinitionId, usize>,
}

pub(crate) struct Class {
    pub(crate) definition: DefinitionId,
    pub(crate) name: String,
    pub(crate) node: ast::StmtClassDef,
    pub(crate) fields: Vec<Field>,
    pub(crate) methods: Vec<ast::StmtFunctionDef>,
    /// Index into `methods`, including the initializer itself.
    pub(crate) initializer: usize,
}

pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) annotation: ast::Expr,
}

impl Classes {
    /// Assigns nominal identities in module and source order, independently of imports' aliases.
    pub(crate) fn collect(graph: &SourceGraph) -> Result<Self, String> {
        let mut definitions = Vec::new();
        let mut ids = HashMap::new();
        for (module_id, module) in graph.modules.iter().enumerate() {
            for (statement, node) in module.parsed.syntax().body.iter().enumerate() {
                if let ast::Stmt::ClassDef(node) = node {
                    let definition = DefinitionId {
                        module: module_id,
                        statement,
                    };
                    let class = collect_class(module, definition, node)?;
                    ids.insert(definition, definitions.len());
                    definitions.push(class);
                }
            }
        }
        Ok(Self { definitions, ids })
    }
}

fn collect_class(
    module: &SourceModule,
    definition: DefinitionId,
    node: &ast::StmtClassDef,
) -> Result<Class, String> {
    if !node.decorator_list.is_empty()
        || node.type_params.is_some()
        || node
            .arguments
            .as_ref()
            .is_some_and(|args| !args.args.is_empty() || !args.keywords.is_empty())
    {
        return Err(module.error(
            node,
            "class inheritance, metaclasses, decorators, and type parameters are unsupported",
        ));
    }
    let mut names = HashSet::new();
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    let mut initializer = None;
    for statement in &node.body {
        match statement {
            ast::Stmt::AnnAssign(field) if field.value.is_none() => {
                let ast::Expr::Name(name) = &*field.target else {
                    return Err(module.error(field, "a field declaration must annotate one name"));
                };
                let name = name.id.as_str();
                if name.starts_with("__") {
                    return Err(module.error(field, "field names starting with '__' are reserved"));
                }
                if !names.insert(name.to_owned()) {
                    return Err(module.error(field, &format!("duplicate class member {name:?}")));
                }
                fields.push(Field {
                    name: name.to_owned(),
                    annotation: (*field.annotation).clone(),
                });
            }
            ast::Stmt::FunctionDef(method) => {
                if method.is_async
                    || !method.decorator_list.is_empty()
                    || method.type_params.is_some()
                {
                    return Err(module.error(
                        method,
                        "async methods, method decorators, and method type parameters are unsupported",
                    ));
                }
                let name = method.name.as_str();
                if name.starts_with("__") && name != "__init__" {
                    return Err(module.error(
                        method,
                        "method names starting with '__' are reserved except '__init__'",
                    ));
                }
                if !names.insert(name.to_owned()) {
                    return Err(module.error(method, &format!("duplicate class member {name:?}")));
                }
                let first = method
                    .parameters
                    .posonlyargs
                    .first()
                    .or_else(|| method.parameters.args.first());
                if first.is_none_or(|parameter| parameter.name().as_str() != "self") {
                    return Err(module.error(
                        method,
                        "an instance method's first positional parameter must be named 'self'",
                    ));
                }
                if name == "__init__" {
                    initializer = Some(methods.len());
                }
                methods.push(method.clone());
            }
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {}
            _ => {
                return Err(module.error(
                    statement,
                    "class bodies allow only field annotations without defaults, methods, and docstrings",
                ));
            }
        }
    }
    let initializer = initializer
        .ok_or_else(|| module.error(node, "a fixed class requires an explicit __init__ method"))?;
    check_initializer(module, &methods[initializer], &fields)?;
    Ok(Class {
        definition,
        name: node.name.as_str().to_owned(),
        node: node.clone(),
        fields,
        methods,
        initializer,
    })
}

/// Initializes each field once before `self` can be read or passed to another operation.
/// This permits allocation before argument assignment without exposing a partial object.
fn check_initializer(
    module: &SourceModule,
    initializer: &ast::StmtFunctionDef,
    fields: &[Field],
) -> Result<(), String> {
    let declared = fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<HashSet<_>>();
    let mut initialized = HashSet::new();
    for statement in &initializer.body {
        let assignment = match statement {
            ast::Stmt::Assign(assignment) => assignment,
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {
                continue;
            }
            ast::Stmt::Pass(_) => continue,
            _ => {
                return Err(module.error(
                    statement,
                    "__init__ must contain only straight-line self.field assignments, docstrings, or pass",
                ));
            }
        };
        let mut targets = Vec::new();
        for target in &assignment.targets {
            initializer_targets(target, &mut targets);
        }
        for target in targets {
            let ast::Expr::Attribute(attribute) = target else {
                return Err(module.error(target, "__init__ must assign a declared self.field"));
            };
            if !matches!(&*attribute.value, ast::Expr::Name(name) if name.id == "self") {
                return Err(module.error(target, "__init__ must assign a declared self.field"));
            }
            let name = attribute.attr.as_str();
            if !declared.contains(name) {
                return Err(module.error(
                    target,
                    &format!("field {name:?} is not declared in the class"),
                ));
            }
            if !initialized.insert(name) {
                return Err(module.error(
                    target,
                    &format!("field {name:?} is initialized more than once"),
                ));
            }
        }
        let mut self_use = SelfUse(None);
        self_use.visit_expr(&assignment.value);
        if let Some(expression) = self_use.0 {
            return Err(module.error(
                expression,
                "__init__ assignment values cannot read or expose self before construction finishes",
            ));
        }
    }
    let missing = fields
        .iter()
        .filter(|field| !initialized.contains(field.name.as_str()))
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(module.error(
            initializer,
            &format!(
                "__init__ does not initialize fields: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}

/// Chained and nested assignments must still initialize only declared fields.
/// A starred target receives one list, so it has the same field requirement.
fn initializer_targets<'a>(target: &'a ast::Expr, targets: &mut Vec<&'a ast::Expr>) {
    match target {
        ast::Expr::Tuple(tuple) => {
            for target in &tuple.elts {
                initializer_targets(target, targets);
            }
        }
        ast::Expr::List(list) => {
            for target in &list.elts {
                initializer_targets(target, targets);
            }
        }
        ast::Expr::Starred(starred) => initializer_targets(&starred.value, targets),
        _ => targets.push(target),
    }
}

struct SelfUse<'a>(Option<&'a ast::Expr>);

impl<'a> Visitor<'a> for SelfUse<'a> {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if self.0.is_some() {
            return;
        }
        // A comprehension target named self is distinct from the incomplete
        // receiver. Its leftmost iterable still belongs to the enclosing scope.
        let clauses = match expression {
            ast::Expr::ListComp(value) => Some(value.generators.as_slice()),
            ast::Expr::SetComp(value) => Some(value.generators.as_slice()),
            ast::Expr::DictComp(value) => Some(value.generators.as_ref()),
            ast::Expr::Generator(value) => Some(value.generators.as_slice()),
            _ => None,
        };
        fn binds_self(target: &ast::Expr) -> bool {
            match target {
                ast::Expr::Name(name) => name.id == "self",
                ast::Expr::Tuple(value) => value.elts.iter().any(binds_self),
                ast::Expr::List(value) => value.elts.iter().any(binds_self),
                ast::Expr::Starred(value) => binds_self(&value.value),
                _ => false,
            }
        }
        if let Some(clauses) = clauses
            && clauses.iter().any(|clause| binds_self(&clause.target))
        {
            self.visit_expr(&clauses[0].iter);
            return;
        }
        if matches!(expression, ast::Expr::Name(name) if name.id == "self") {
            self.0 = Some(expression);
        } else {
            walk_expr(self, expression);
        }
    }
}
