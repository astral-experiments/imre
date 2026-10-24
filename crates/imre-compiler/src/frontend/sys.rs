//! Sealed bindings for the process's standard streams and exit exception.
use super::*;

impl Checker<'_> {
    pub(super) fn sys_call(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::SysExit)) {
            return self.system_exit(call, true).map(Some);
        }
        let ast::Expr::Attribute(attribute) = &*call.func else {
            return Ok(None);
        };
        let stderr = match self.resolve(&attribute.value) {
            Some(Binding::Intrinsic(Intrinsic::Stdout)) => false,
            Some(Binding::Intrinsic(Intrinsic::Stderr)) => true,
            _ => return Ok(None),
        };
        self.reject_builtin_keywords(call)?;
        let (ty, op, args) = match (attribute.attr.as_str(), call.arguments.args.as_slice()) {
            ("write", [text]) => (
                Type::Int,
                Builtin::SysStreamWrite { stderr },
                vec![self.expr_expected(text, Type::Str)?],
            ),
            ("flush", []) => (Type::None, Builtin::SysStreamFlush { stderr }, Vec::new()),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "standard streams support write(str) and flush()",
                ));
            }
        };
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Builtin(op, args),
        }))
    }

    pub(super) fn system_exit(
        &mut self,
        call: &ast::ExprCall,
        raise: bool,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let args = match call.arguments.args.as_slice() {
            [] => Vec::new(),
            [value] => {
                let value = self.expr(value)?;
                if !matches!(
                    value.ty,
                    Type::None | Type::Bool | Type::Int | Type::Str | Type::StrLiteral(_)
                ) {
                    return Err(error(
                        self.source,
                        call,
                        "exit status requires None, bool, int, or str",
                    ));
                }
                vec![value]
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "exit accepts zero or one positional argument",
                ));
            }
        };
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Builtin(Builtin::SystemExitNew { omit_none: raise }, args),
        };
        Ok(if raise {
            Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::SysExit, vec![exception]),
            }
        } else {
            exception
        })
    }
}
