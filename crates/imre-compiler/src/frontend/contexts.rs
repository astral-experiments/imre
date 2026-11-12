//! Enter and resume single-yield generator context managers through the same
//! finally lowering as sealed file and temporary-directory scopes.

use super::*;
use crate::ir::ProcessOperation as Op;

pub(super) fn decorated(method: &ast::StmtFunctionDef, graph: &SourceGraph, module: usize) -> bool {
    matches!(method.decorator_list.as_slice(), [decorator]
        if graph.resolve(module, &decorator.expression) == Some(Binding::Intrinsic(Intrinsic::ContextManager)))
}

/// Early no-cleanup branches must yield and return together. The final yield
/// can have a finally suite, which runs both on normal exit and on injection.
pub(super) fn single_yield(method: &ast::StmtFunctionDef) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
    struct HasYield(bool);
    impl<'a> Visitor<'a> for HasYield {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if matches!(expr, ast::Expr::Yield(_) | ast::Expr::YieldFrom(_)) {
                self.0 = true;
            } else {
                walk_expr(self, expr);
            }
        }
    }
    fn bare(stmt: &ast::Stmt) -> bool {
        matches!(stmt, ast::Stmt::Expr(stmt) if matches!(&*stmt.value, ast::Expr::Yield(y) if y.value.is_none()))
    }
    fn none(body: &[ast::Stmt]) -> bool {
        let mut has = HasYield(false);
        has.visit_body(body);
        !has.0
    }
    fn prelude(body: &[ast::Stmt]) -> bool {
        match body {
            [rest @ .., y, ast::Stmt::Return(ret)] if bare(y) && ret.value.is_none() => {
                prelude(rest)
            }
            _ => body.iter().all(|stmt| {
                if let ast::Stmt::If(branch) = stmt {
                    prelude(&branch.body)
                        && branch
                            .elif_else_clauses
                            .iter()
                            .all(|clause| prelude(&clause.body))
                } else {
                    none(std::slice::from_ref(stmt))
                }
            }),
        }
    }
    // A return in finally can suppress a thrown body error. That would require
    // making an otherwise terminating with statement fall through.
    struct NonlocalExit(bool);
    impl<'a> Visitor<'a> for NonlocalExit {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if matches!(stmt, ast::Stmt::Return(_)) {
                self.0 = true;
            } else if !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                walk_stmt(self, stmt);
            }
        }
    }
    fn cleanup(body: &[ast::Stmt]) -> bool {
        let mut exit = NonlocalExit(false);
        exit.visit_body(body);
        !exit.0 && none(body)
    }
    let Some((last, setup)) = method.body.split_last() else {
        return false;
    };
    prelude(setup)
        && (bare(last)
            || matches!(last, ast::Stmt::Try(t) if !t.is_star && t.handlers.is_empty()
        && t.orelse.is_empty() && t.body.len() == 1 && bare(&t.body[0]) && cleanup(&t.finalbody)))
}

pub(super) fn op(op: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Process(op), args),
    }
}

pub(super) type Protected<'a> = std::ops::ControlFlow<bool, &'a mut Vec<Statement>>;

/// Each acquisition opens its checked protected suite after a successful enter.
/// The next acquisition (or the user suite) goes after its target binding.
pub(super) fn with_items<C, I: IntoIterator, E>(
    scope: &mut C,
    items: I,
    mut enter: impl for<'s> FnMut(
        &mut C,
        I::Item,
        &'s mut Vec<Statement>,
    ) -> StdResult<Protected<'s>, E>,
    body: impl FnOnce(&mut C) -> StdResult<statements::Block, E>,
) -> StdResult<statements::Block, E> {
    let mut statements = Vec::new();
    let mut protected = &mut statements;
    for item in items {
        protected = match enter(scope, item, protected)? {
            Protected::Continue(suite) => suite,
            Protected::Break(falls) => return Ok((statements, falls)),
        };
    }
    let falls = finish(protected, body(scope)?);
    Ok((statements, falls))
}

pub(super) fn finish(protected: &mut Vec<Statement>, (body, falls): statements::Block) -> bool {
    if protected.is_empty() {
        *protected = body;
    } else {
        protected.extend(body);
    }
    falls
}

/// Return the protected checked suite in the operation just appended.
fn open(enclosing: &mut Vec<Statement>, region: Statement) -> &mut Vec<Statement> {
    match enclosing.push_mut(region) {
        Statement::Scope { body, .. }
        | Statement::Try { body, .. }
        | Statement::With { body, .. } => body,
        _ => unreachable!("a checked manager opens a scope, try, or Python protocol region"),
    }
}

pub(super) fn enter_native(
    enclosing: &mut Vec<Statement>,
    enter: Vec<Statement>,
    binding: Vec<Statement>,
    handlers: Vec<ir::ExceptHandler>,
    exit: Statement,
    clear: Vec<usize>,
) -> &mut Vec<Statement> {
    let acquired = if clear.is_empty() {
        enclosing.extend(enter);
        enclosing
    } else {
        open(enclosing, Statement::Scope { body: enter, clear })
    };
    open(
        acquired,
        Statement::Try {
            body: binding,
            handlers,
            orelse: Vec::new(),
            finalbody: vec![exit],
        },
    )
}

pub(super) fn python_with(
    managers: impl IntoIterator<Item = (ir::ContextSite, Expr, Option<ir::ContextBinding>)>,
    body: Vec<Statement>,
) -> Vec<Statement> {
    let Ok((statements, _)) = with_items(
        &mut (),
        managers,
        |(), (site, manager, binding), protected| {
            Ok(Protected::Continue(open(
                protected,
                Statement::With {
                    manager,
                    site,
                    binding,
                    body: Vec::new(),
                },
            )))
        },
        |()| Ok::<_, std::convert::Infallible>((body, true)),
    );
    statements
}

impl Checker<'_> {
    pub(super) fn enter_generator_context<'s>(
        &mut self,
        out: &'s mut Vec<Statement>,
        item: &ast::WithItem,
        manager: Expr,
    ) -> Result<&'s mut Vec<Statement>, String> {
        let slot = self.hidden(manager.ty);
        let mut protected = Vec::new();
        if let Some(target) = &item.optional_vars {
            self.assign(target, Expr::none(), Some(&mut protected))?;
        }
        let exception = self.hidden(Type::Exception);
        let handled = op(
            Op::ContextThrow,
            Type::Bool,
            vec![self.local(slot), self.local(exception)],
        );
        Ok(enter_native(
            out,
            vec![
                Statement::Assign(slot, manager),
                Statement::Expr(op(Op::ContextEnter, Type::None, vec![self.local(slot)])),
            ],
            protected,
            vec![ir::ExceptHandler {
                matcher: ir::ExceptionMatcher::Categories(vec![0]),
                name: Some(exception),
                body: vec![Statement::If(handled, vec![], vec![Statement::Raise(None)])],
            }],
            Statement::Expr(op(Op::ContextExit, Type::None, vec![self.local(slot)])),
            vec![slot],
        ))
    }
}
