//! Admit worker functions whose compiled body touches only its explicitly transferable arguments.
//! No Python object or cell in the parent interpreter may be reached by such a body.

use ruff_python_ast as ast;

#[derive(Clone, Copy)]
struct Proof<'a> {
    params: &'a [&'a str],
    lock_only: bool,
}

impl Proof<'_> {
    fn expression(self, expr: &ast::Expr) -> bool {
        match expr {
            ast::Expr::BooleanLiteral(_) | ast::Expr::NoneLiteral(_) => true,
            ast::Expr::NumberLiteral(_) => !self.lock_only,
            ast::Expr::Compare(expr) if !self.lock_only => {
                expr.operands.iter().all(|e| self.expression(e))
            }
            ast::Expr::UnaryOp(expr) if expr.op == ast::UnaryOp::Not => {
                self.expression(&expr.operand)
            }
            ast::Expr::BoolOp(expr) => expr.values.iter().all(|operand| self.expression(operand)),
            ast::Expr::Call(call) if call.arguments.keywords.is_empty() => {
                if let ast::Expr::Name(name) = &*call.func {
                    return !self.lock_only
                        && self.params.contains(&name.id.as_str())
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
                        "release" => !self.lock_only && call.arguments.args.is_empty(),
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
) -> Option<(Vec<&'a str>, bool)> {
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
    let proof = Proof {
        params: &names,
        lock_only: false,
    };
    if !proof.suite(stmts) {
        return None;
    }
    // Joinable workers cannot dispatch a Python exception hook across heaps.
    let lock_only = Proof {
        lock_only: true,
        ..proof
    }
    .suite(stmts);
    Some((names, lock_only))
}

/// Certify a final lock acquire with at most one unconditional release before it.
/// No cell can be read after a potentially blocking operation.
pub(super) fn captured_locks<'a>(
    stmts: &'a [ast::Stmt],
    params: &ast::Parameters,
) -> Option<Vec<&'a str>> {
    fn receiver<'a>(stmt: &'a ast::Stmt, method: &str) -> Option<&'a str> {
        let ast::Stmt::Expr(expr) = stmt else {
            return None;
        };
        let ast::Expr::Call(call) = &*expr.value else {
            return None;
        };
        let ast::Expr::Attribute(attr) = &*call.func else {
            return None;
        };
        let ast::Expr::Name(name) = &*attr.value else {
            return None;
        };
        (attr.attr.as_str() == method && call.arguments.is_empty()).then(|| name.id.as_str())
    }
    if params.iter().next().is_some() {
        return None;
    }
    match stmts {
        [last] => Some(vec![receiver(last, "acquire")?]),
        [first, last] => {
            let first = receiver(first, "release")?;
            let last = receiver(last, "acquire")?;
            (first != last).then(|| vec![first, last])
        }
        _ => None,
    }
}

/// The only heap lookup in this method precedes the blocking native acquire.
pub(super) fn bound_lock_field<'a>(
    stmts: &'a [ast::Stmt],
    params: &ast::Parameters,
) -> Option<&'a str> {
    if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
        return None;
    }
    let mut positional = params.posonlyargs.iter().chain(&params.args);
    let receiver = positional.next()?.name();
    if positional.next().is_some() || receiver.starts_with("__") {
        return None;
    }
    let [ast::Stmt::Expr(statement)] = stmts else {
        return None;
    };
    let ast::Expr::Call(call) = &*statement.value else {
        return None;
    };
    let ast::Expr::Attribute(method) = &*call.func else {
        return None;
    };
    let ast::Expr::Attribute(field) = &*method.value else {
        return None;
    };
    let ast::Expr::Name(owner) = &*field.value else {
        return None;
    };
    (call.arguments.is_empty()
        && method.attr.as_str() == "acquire"
        && owner.id.as_str() == receiver.as_str()
        && !field.attr.starts_with("__"))
    .then(|| field.attr.as_str())
}
