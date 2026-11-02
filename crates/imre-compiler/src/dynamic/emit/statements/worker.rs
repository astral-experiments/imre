//! Admit worker functions whose compiled body touches only its explicitly transferable arguments.
//! No Python object or cell in the parent interpreter may be reached by such a body.

use ruff_python_ast as ast;

#[derive(Clone, Copy)]
struct Proof<'a> {
    params: &'a [&'a str],
}

impl Proof<'_> {
    fn expression(self, expr: &ast::Expr) -> bool {
        match expr {
            ast::Expr::BooleanLiteral(_)
            | ast::Expr::NoneLiteral(_)
            | ast::Expr::NumberLiteral(_) => true,
            ast::Expr::Compare(expr) => expr.operands.iter().all(|e| self.expression(e)),
            ast::Expr::UnaryOp(expr) if expr.op == ast::UnaryOp::Not => {
                self.expression(&expr.operand)
            }
            ast::Expr::BoolOp(expr) => expr.values.iter().all(|operand| self.expression(operand)),
            ast::Expr::Call(call) if call.arguments.keywords.is_empty() => {
                if let ast::Expr::Name(name) = &*call.func {
                    return self.params.contains(&name.id.as_str())
                        && call.arguments.args.iter().all(|arg| self.expression(arg));
                }
                let ast::Expr::Attribute(attr) = &*call.func else {
                    return false;
                };
                matches!(&*attr.value, ast::Expr::Name(name) if self.params.contains(&name.id.as_str()))
                    && match attr.attr.as_str() {
                        "locked" => call.arguments.args.is_empty(),
                        "acquire" => matches!(
                            call.arguments.args.as_ref(),
                            [] | [ast::Expr::BooleanLiteral(_)]
                        ),
                        "release" => call.arguments.args.is_empty(),
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    fn suite(self, stmts: &[ast::Stmt]) -> bool {
        stmts.iter().all(|stmt| match stmt {
            ast::Stmt::Pass(_) | ast::Stmt::Break(_) | ast::Stmt::Continue(_) => true,
            ast::Stmt::Expr(stmt) => self.expression(&stmt.value),
            ast::Stmt::Return(stmt) => stmt.value.as_deref().is_none_or(|v| self.expression(v)),
            ast::Stmt::If(stmt) => {
                self.expression(&stmt.test)
                    && self.suite(&stmt.body)
                    && stmt.elif_else_clauses.iter().all(|clause| {
                        clause.test.as_ref().is_none_or(|v| self.expression(v))
                            && self.suite(&clause.body)
                    })
            }
            ast::Stmt::While(stmt) => {
                self.expression(&stmt.test) && self.suite(&stmt.body) && self.suite(&stmt.orelse)
            }
            ast::Stmt::Try(stmt) if !stmt.is_star => {
                let [ast::ExceptHandler::ExceptHandler(handler)] = stmt.handlers.as_slice() else {
                    return false;
                };
                handler.type_.is_none()
                    && handler.name.is_none()
                    && self.suite(&stmt.body)
                    && self.suite(&handler.body)
                    && self.suite(&stmt.orelse)
                    && self.suite(&stmt.finalbody)
            }
            _ => false,
        })
    }
}

pub(super) fn parameters<'a>(
    stmts: &[ast::Stmt],
    params: &'a ast::Parameters,
) -> Option<Vec<&'a str>> {
    if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for param in params.posonlyargs.iter().chain(&params.args) {
        let name = param.name().as_str();
        if param.parameter.annotation.is_some() || name.starts_with("__") {
            return None;
        }
        names.push(name);
    }
    let proof = Proof { params: &names };
    proof.suite(stmts).then_some(names)
}
