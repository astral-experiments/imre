//! Normalize nonescaping keyword forwarding onto a known callee's fixed signature.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr};

pub(super) struct Defaults {
    pub(super) module: ModuleId,
    pub(super) forwarded: Option<(usize, ModuleId)>,
}

impl Defaults {
    pub(super) fn local(module: ModuleId) -> Self {
        Self {
            module,
            forwarded: None,
        }
    }
}

/// Carry the callee's source along with any imported default expressions.
pub(super) fn prepare(
    graph: &SourceGraph,
    module: ModuleId,
    f: &mut Cow<'_, ast::StmtFunctionDef>,
    classes: &Classes,
) -> Defaults {
    Defaults {
        module,
        forwarded: expand(graph, module, f, classes),
    }
}

/// In an otherwise unchanged initializer, bind packs at a statically known
/// base call only when the call is first and no other statement observes them.
pub(super) fn prepare_method(
    graph: &SourceGraph,
    module: ModuleId,
    method: &mut Cow<'_, ast::StmtFunctionDef>,
    context: records::MethodContext,
    classes: &Classes,
) -> (Defaults, Option<DefinitionId>) {
    if method.name.as_str() == "__init__"
        && let Some(origin) = expand_initializer(graph, module, method, context, classes)
    {
        (
            Defaults {
                module,
                forwarded: Some((1, origin.module)),
            },
            Some(origin),
        )
    } else {
        (prepare(graph, module, method, classes), None)
    }
}

fn expand_initializer(
    graph: &SourceGraph,
    module: ModuleId,
    method: &mut Cow<'_, ast::StmtFunctionDef>,
    context: records::MethodContext,
    classes: &Classes,
) -> Option<DefinitionId> {
    let parameters = &method.parameters;
    let positional = parameters.vararg.as_ref()?;
    let keywords = parameters.kwarg.as_ref()?;
    if positional.annotation.is_some()
        || keywords.annotation.is_some()
        || !parameters.posonlyargs.is_empty()
        || !parameters.kwonlyargs.is_empty()
        || parameters.args.len() != 1
    {
        return None;
    }
    let after_doc = usize::from(docstring_literal(&method.body).is_some());
    let (first, tail) = method.body.get(after_doc..)?.split_first()?;
    let ast::Stmt::Expr(stmt) = first else {
        return None;
    };
    let call = stmt.value.as_call_expr()?;
    let origin = classes.definitions[context.class].method_origins[context.index];
    let parent = &classes.definitions[classes.definitions[classes.ids[&origin]].storage_base?];
    let direct = crate::classes::direct_base_init(graph, module, parent.definition, method, call);
    if !direct
        && (!crate::classes::super_init(call)
            || graph.modules[module].bindings.contains_key("super")
            || [
                parameters.args[0].name().as_str(),
                positional.name.as_str(),
                keywords.name.as_str(),
            ]
            .contains(&"super"))
    {
        return None;
    }
    let [ast::Expr::Starred(star)] = &call.arguments.args[usize::from(direct)..] else {
        return None;
    };
    let [kw] = &*call.arguments.keywords else {
        return None;
    };
    if star.value.as_name_expr()?.id.as_str() != positional.name.as_str()
        || kw.arg.is_some()
        || kw.value.as_name_expr()?.id.as_str() != keywords.name.as_str()
    {
        return None;
    }

    let index = parent.initializer?;
    let base = &parent.methods[index];
    let params = &base.parameters;
    let base_module = parent.method_origins[index].module;
    if params.vararg.is_some()
        || params.kwarg.is_some()
        || !params.kwonlyargs.is_empty()
        || !params.posonlyargs.is_empty()
        || params.args.is_empty()
    {
        return None;
    }
    let mut uses = Uses {
        names: HashSet::new(),
        bound: HashSet::new(),
        keyword: keywords.name.as_str(),
        reads: 0,
        safe: true,
    };
    // A base parameter may have the same name as the original callable or module.
    uses.visit_expr(&call.func);
    for stmt in tail {
        uses.visit_stmt(stmt);
    }
    if !uses.safe
        || uses.names.contains(positional.name.as_str())
        || uses.names.contains(keywords.name.as_str())
        || uses.bound.contains("super")
    {
        return None;
    }
    let receiver = parameters.args[0].name();
    let fixed = &params.args[1..];
    // A newly materialized parameter must not shadow a free name or a receiver.
    // Limit inherited defaults to immutables lowered from their defining module.
    if fixed.iter().any(|p| {
        p.name() == receiver
            || ruff_python_stdlib::identifiers::is_mangled_private(p.name().as_str())
            || uses.names.contains(p.name().as_str())
            || !p.annotation().is_some_and(scalar_annotation)
            || p.default.as_deref().is_some_and(|d| {
                calls::scalar_literal(d, &graph.modules[base_module].source).is_err()
            })
    }) {
        return None;
    }
    let range = call.range();
    let mut forwarded: Vec<_> = call.arguments.args[..usize::from(direct)].to_vec();
    forwarded.extend(fixed.iter().map(|p| {
        ast::Expr::Name(ast::ExprName {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            id: p.name().as_str().into(),
            ctx: ast::ExprContext::Load,
        })
    }));
    let method = method.to_mut();
    let ast::Stmt::Expr(stmt) = &mut method.body[after_doc] else {
        unreachable!()
    };
    let ast::Expr::Call(call) = &mut *stmt.value else {
        unreachable!()
    };
    call.arguments.args = forwarded.into_iter().collect();
    call.arguments.keywords.clear();
    method.parameters.vararg = None;
    method.parameters.kwarg = None;
    method.parameters.args.extend(fixed.iter().cloned());
    Some(parent.method_origins[index])
}

struct Uses<'a> {
    names: HashSet<&'a str>,
    bound: HashSet<&'a str>,
    keyword: &'a str,
    reads: usize,
    safe: bool,
}

impl<'a> Visitor<'a> for Uses<'a> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let ast::Expr::Name(n) = expr {
            let name = n.id.as_str();
            self.names.insert(name);
            if n.ctx != ast::ExprContext::Load {
                self.bound.insert(name);
            }
            self.reads += usize::from(name == self.keyword);
            self.safe &= !matches!(name, "locals" | "vars" | "exec" | "eval");
        }
        self.safe &= !matches!(
            expr,
            ast::Expr::Lambda(_)
                | ast::Expr::Named(_)
                | ast::Expr::ListComp(_)
                | ast::Expr::SetComp(_)
                | ast::Expr::DictComp(_)
                | ast::Expr::Generator(_)
        );
        walk_expr(self, expr);
    }
}

/// The source of each forwarded default and the single pack use it replaces.
struct Forwarding<'a> {
    statement: usize,
    range: ruff_text_size::TextRange,
    origin: ModuleId,
    options: Vec<&'a ast::ParameterWithDefault>,
}

fn forwarding<'a>(
    graph: &'a SourceGraph,
    module: ModuleId,
    function: &ast::StmtFunctionDef,
    classes: &'a Classes,
    constructor_only: bool,
) -> Option<Forwarding<'a>> {
    let parameters = &function.parameters;
    let keyword = parameters.kwarg.as_ref()?;
    if function.is_async || parameters.vararg.is_some() || keyword.annotation.is_some() {
        return None;
    }
    let bound: HashSet<_> = parameters
        .iter_non_variadic_params()
        .map(|p| p.name().as_str())
        .collect();
    let mut uses = Uses {
        names: bound.clone(),
        bound,
        keyword: keyword.name.as_str(),
        reads: 0,
        safe: true,
    };
    let mut site = None;
    for (index, stmt) in function.body.iter().enumerate() {
        let expr = match stmt {
            ast::Stmt::Assign(s) if matches!(s.targets.as_slice(), [ast::Expr::Name(_)]) => {
                Some(&*s.value)
            }
            ast::Stmt::Expr(s) => Some(&*s.value),
            ast::Stmt::Return(s) => s.value.as_deref(),
            ast::Stmt::Pass(_) => None,
            _ => return None,
        };
        uses.visit_stmt(stmt);
        if let Some(ast::Expr::Call(call)) = expr
            && let Some((kw, written)) = call.arguments.keywords.split_last()
            && written.iter().all(|kw| kw.arg.is_some())
            && kw.arg.is_none()
            && matches!(&kw.value, ast::Expr::Name(n) if n.id.as_str() == keyword.name.as_str())
            && !calls::starred(call)
            && let ast::Expr::Name(name) = &*call.func
            && let Some(Binding::Definition(def)) = graph.resolve(module, &call.func)
        {
            site = Some((index, def, name.id.as_str(), call, written, kw.range));
        }
    }
    let (index, def, name, call, written, range) = site?;
    if uses.reads != 1 || !uses.safe || uses.bound.contains(name) {
        return None;
    }
    let (target, receiver, origin) = match &graph.modules[def.module].parsed.syntax().body
        [def.statement]
    {
        ast::Stmt::FunctionDef(f) if !constructor_only && written.is_empty() => (f, 0, def.module),
        ast::Stmt::ClassDef(_) if graph.modules[module].bundled && module == def.module => {
            let class = &classes.definitions[*classes.ids.get(&def)?];
            // Only an ordinary class call uses the initializer's keyword contract.
            if class.abc
                || class.enum_members.is_some()
                || class.methods.iter().any(|m| m.name.as_str() == "__new__")
            {
                return None;
            }
            let init = class.initializer?;
            (&*class.methods[init], 1, class.method_origins[init].module)
        }
        _ => return None,
    };
    let callee = &target.parameters;
    let supplied = receiver + call.arguments.args.len();
    if !target.decorator_list.is_empty()
        || target.is_async
        || target.type_params.is_some()
        || callee.vararg.is_some()
        || (receiver == 0 && supplied != callee.posonlyargs.len() + callee.args.len())
    {
        return None;
    }
    let remaining = callee
        .args
        .get(supplied.checked_sub(callee.posonlyargs.len())?..)?;
    let (options, origin) = if callee.kwarg.is_some() {
        // Another wrapper can forward to the same closed constructor, even
        // when the caller's module is checked before the implementation.
        if receiver != 0 || !graph.modules[def.module].bundled || !callee.kwonlyargs.is_empty() {
            return None;
        }
        let next = forwarding(graph, def.module, target, classes, true)?;
        (next.options, next.origin)
    } else {
        let remaining = remaining.iter().chain(&callee.kwonlyargs);
        let options = remaining
            .filter(|p| {
                !written
                    .iter()
                    .any(|kw| kw.arg.as_deref() == Some(p.name().as_str()))
            })
            .collect();
        (options, origin)
    };
    for option in &options {
        let name = option.name().as_str();
        if name == uses.keyword
            || uses.names.contains(name)
            || option.annotation().is_some_and(|a| !scalar_annotation(a))
            || !option
                .default
                .as_deref()
                .is_some_and(|e| calls::scalar_literal(e, &graph.modules[origin].source).is_ok())
        {
            return None;
        }
    }
    Some(Forwarding {
        statement: index,
        range,
        origin,
        options,
    })
}

fn expand(
    graph: &SourceGraph,
    module: ModuleId,
    function: &mut Cow<'_, ast::StmtFunctionDef>,
    classes: &Classes,
) -> Option<(usize, ModuleId)> {
    let Forwarding {
        statement: index,
        range,
        origin,
        options,
    } = forwarding(graph, module, function, classes, false)?;
    let expanded: Vec<_> = options
        .iter()
        .map(|p| {
            let name = p.name().as_str();
            ast::Keyword {
                range,
                node_index: ast::AtomicNodeIndex::NONE,
                arg: Some(ast::Identifier::new(name, range)),
                value: ast::Expr::Name(ast::ExprName {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    id: name.into(),
                    ctx: ast::ExprContext::Load,
                }),
            }
        })
        .collect();
    let first = function.parameters.iter_non_variadic_params().count();
    let function = function.to_mut();
    function.parameters.kwarg = None;
    function
        .parameters
        .kwonlyargs
        .extend(options.into_iter().cloned());
    let value = match &mut function.body[index] {
        ast::Stmt::Assign(s) => &mut *s.value,
        ast::Stmt::Expr(s) => &mut *s.value,
        ast::Stmt::Return(s) => s.value.as_deref_mut().unwrap(),
        _ => unreachable!(),
    };
    let ast::Expr::Call(call) = value else {
        unreachable!()
    };
    call.arguments.keywords.pop();
    call.arguments.keywords.extend(expanded);
    Some((first, origin))
}

fn scalar_annotation(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Name(n) => matches!(n.id.as_str(), "bool" | "int" | "float" | "str" | "bytes"),
        ast::Expr::NoneLiteral(_) => true,
        ast::Expr::BinOp(b) if b.op == ast::Operator::BitOr => {
            scalar_annotation(&b.left) && scalar_annotation(&b.right)
        }
        _ => false,
    }
}
