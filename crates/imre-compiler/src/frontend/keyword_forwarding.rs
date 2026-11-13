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
) -> Defaults {
    Defaults {
        module,
        forwarded: expand(graph, module, f),
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
        (prepare(graph, module, method), None)
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

/// Expand a single forwarding call only when all of the callee's positional
/// slots are supplied and every remaining keyword has an immutable default.
/// Return the origin of the appended AST nodes so their original ranges remain valid.
fn expand(
    graph: &SourceGraph,
    module: ModuleId,
    function: &mut Cow<'_, ast::StmtFunctionDef>,
) -> Option<(usize, ModuleId)> {
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
            && let [kw] = call.arguments.keywords.as_slice()
            && kw.arg.is_none()
            && matches!(&kw.value, ast::Expr::Name(n) if n.id.as_str() == keyword.name.as_str())
            && !calls::starred(call)
            && let ast::Expr::Name(name) = &*call.func
            && let Some(Binding::Definition(def)) = graph.resolve(module, &call.func)
        {
            site = Some((
                index,
                def,
                name.id.as_str(),
                call.arguments.args.len(),
                kw.range,
            ));
        }
    }
    let (index, def, name, supplied, range) = site?;
    if uses.reads != 1 || !uses.safe || uses.bound.contains(name) {
        return None;
    }
    let ast::Stmt::FunctionDef(target) =
        &graph.modules[def.module].parsed.syntax().body[def.statement]
    else {
        return None;
    };
    let callee = &target.parameters;
    if !target.decorator_list.is_empty()
        || target.is_async
        || target.type_params.is_some()
        || callee.kwarg.is_some()
        || callee.vararg.is_some()
        || callee.posonlyargs.len() + callee.args.len() != supplied
    {
        return None;
    }
    for option in &callee.kwonlyargs {
        let name = option.name().as_str();
        if name == uses.keyword
            || uses.names.contains(name)
            || option.annotation().is_some_and(|a| !scalar_annotation(a))
            || !option.default.as_deref().is_some_and(|e| {
                calls::scalar_literal(e, &graph.modules[def.module].source).is_ok()
            })
        {
            return None;
        }
    }
    let expanded = callee
        .kwonlyargs
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
    let first = parameters.iter_non_variadic_params().count();
    let function = function.to_mut();
    function.parameters.kwarg = None;
    function
        .parameters
        .kwonlyargs
        .extend(callee.kwonlyargs.iter().cloned());
    let value = match &mut function.body[index] {
        ast::Stmt::Assign(s) => &mut *s.value,
        ast::Stmt::Expr(s) => &mut *s.value,
        ast::Stmt::Return(s) => s.value.as_deref_mut().unwrap(),
        _ => unreachable!(),
    };
    let ast::Expr::Call(call) = value else {
        unreachable!()
    };
    call.arguments.keywords = expanded;
    Some((first, def.module))
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
