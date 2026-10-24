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

impl Checker<'_> {
    pub(super) fn generator_scope(
        &mut self,
        item: &ast::WithItem,
        rest: &[ast::WithItem],
        suite: &[ast::Stmt],
        manager: Expr,
    ) -> Result<(Vec<Statement>, bool), String> {
        let slot = self.hidden(manager.ty);
        let mut protected = Vec::new();
        if let Some(target) = &item.optional_vars {
            protected.push(self.assign(
                target,
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
            )?);
        }
        let (body, falls) = self.with_items(rest, suite)?;
        protected.extend(body);
        let exception = self.hidden(Type::Exception);
        let handled = op(
            Op::ContextThrow,
            Type::Bool,
            vec![self.local(slot), self.local(exception)],
        );
        let body = vec![
            Statement::Assign(slot, manager),
            Statement::Expr(op(Op::ContextEnter, Type::None, vec![self.local(slot)])),
            Statement::Try {
                body: protected,
                handlers: vec![ir::ExceptHandler {
                    kinds: vec![0],
                    name: Some(exception),
                    body: vec![Statement::If(handled, vec![], vec![Statement::Raise(None)])],
                }],
                orelse: vec![],
                finalbody: vec![Statement::Expr(op(
                    Op::ContextExit,
                    Type::None,
                    vec![self.local(slot)],
                ))],
            },
        ];
        Ok((
            vec![Statement::Scope {
                body,
                clear: vec![slot],
            }],
            falls,
        ))
    }
}
