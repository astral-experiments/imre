//! Sealed POSIX readiness operations for integer file descriptors.
use super::*;
use crate::ir::{ProcessOperation, Readiness};

fn op(operation: Readiness, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(
            Builtin::Process(ProcessOperation::Readiness(operation)),
            args,
        ),
    }
}

impl Checker<'_> {
    fn readiness_timeout(&mut self, arg: Option<&ast::Expr>) -> Result<Expr, String> {
        let Some(arg) = arg else {
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            });
        };
        let value = self.expr(arg)?;
        if !matches!(
            self.types.payload(value.ty).unwrap_or(value.ty),
            Type::None | Type::Int | Type::Bool | Type::Float | Type::Str | Type::Bytes
        ) {
            return Err(error(
                self.source,
                arg,
                "readiness timeout needs a number or None",
            ));
        }
        Ok(value)
    }

    pub(super) fn readiness_call(
        &mut self,
        call: &ast::ExprCall,
        operation: Readiness,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let args = call.arguments.args.as_ref();
        match (operation, args) {
            (Readiness::New, []) => Ok(op(operation, Type::Poll, vec![])),
            (Readiness::Select, [r, w, x, ..]) if args.len() <= 4 => {
                let args = vec![
                    self.expr_expected(r, Type::ListInt)?,
                    self.expr_expected(w, Type::ListInt)?,
                    self.expr_expected(x, Type::ListInt)?,
                    self.readiness_timeout(args.get(3))?,
                ];
                Ok(op(
                    operation,
                    self.types.tuple(vec![Type::ListInt; 3]),
                    args,
                ))
            }
            _ => Err(error(
                self.source,
                call,
                "expected poll() or select(rlist, wlist, xlist[, timeout])",
            )),
        }
    }

    pub(super) fn poll_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        use Readiness::*;
        self.reject_builtin_keywords(call)?;
        let supplied = call.arguments.args.as_ref();
        let mut args = vec![receiver];
        let (operation, returns) = match (attr.attr.as_str(), supplied) {
            ("poll", [] | [_]) => {
                args.push(self.readiness_timeout(supplied.first())?);
                let pair = self.types.tuple(vec![Type::Int, Type::Int]);
                (Poll, self.types.list(pair))
            }
            ("register", [fd] | [fd, _]) | ("modify", [fd, _]) | ("unregister", [fd]) => {
                args.push(self.expr_expected(fd, Type::Int)?);
                if let Some(mask) = supplied.get(1) {
                    args.push(self.expr_expected(mask, Type::Int)?);
                }
                (
                    match attr.attr.as_str() {
                        "register" => Register,
                        "modify" => Modify,
                        _ => Unregister,
                    },
                    Type::None,
                )
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "expected poll.register(fd[, mask]), modify(fd, mask), unregister(fd), or poll([timeout])",
                ));
            }
        };
        Ok(op(operation, returns, args))
    }
}
