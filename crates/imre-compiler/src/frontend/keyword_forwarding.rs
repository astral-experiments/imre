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
    f: &mut ast::StmtFunctionDef,
) -> Defaults {
    Defaults {
        module,
        forwarded: expand(graph, module, f),
    }
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
    function: &mut ast::StmtFunctionDef,
) -> Option<(usize, ModuleId)> {
    let parameters = &function.parameters;
    let keyword = parameters.kwarg.as_ref()?;
    if function.is_async
        || parameters.vararg.is_some()
        || keyword.annotation.is_some()
        || !function.decorator_list.is_empty()
    {
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
