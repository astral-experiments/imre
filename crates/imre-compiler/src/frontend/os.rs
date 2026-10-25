use super::*;
use crate::ir::{FdOperation, ProcessOperation};

mod credentials;

impl Checker<'_> {
    pub(super) fn fd_call(
        &mut self,
        call: &ast::ExprCall,
        op: FdOperation,
    ) -> Result<Expr, String> {
        use FdOperation::*;

        if op == Open {
            return self.os_open(call);
        }
        let param = |name: &str| Parameter {
            name: name.into(),
            kind: if matches!(op, Close | Dup2) {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::PositionalOnly
            },
            ty: Type::Int,
            default: None,
            sign_only: false,
        };
        let mut params = if op == Pipe {
            vec![]
        } else {
            vec![param("fd")]
        };
        match op {
            Seek => params.extend([param("position"), param("whence")]),
            SetBlocking => params.push(param("blocking")),
            SetInheritable => params.push(param("inheritable")),
            Dup2 => {
                params.push(param("fd2"));
                let mut inherit = param("inheritable");
                inherit.default = Some(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("1".into()),
                });
                params.push(inherit);
            }
            _ => {}
        }
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            let mut value = checker.expr(arg)?;
            if (op == SetBlocking && index == 1) || (op == Dup2 && index == 2) {
                value = checker.truth_value(value);
            }
            if value.ty == Type::Bool {
                Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                })
            } else {
                checker.coerce(value, Type::Int, arg)
            }
        })?;
        let returns = match op {
            Close | SetBlocking | SetInheritable => Type::None,
            Open | Seek | Dup | Dup2 => Type::Int,
            Pipe => self.types.tuple(vec![Type::Int, Type::Int]),
            Isatty | GetBlocking | GetInheritable => Type::Bool,
        };
        Ok(self.ordered_builtin(Builtin::Process(ProcessOperation::Fd(op)), returns, args))
    }

    fn os_open(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let dir_fd = self.types.optional(Type::Int);
        let params = [
            ("path", Type::Str, None),
            ("flags", Type::Int, None),
            (
                "mode",
                Type::Int,
                Some(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("511".into()),
                }),
            ),
            (
                "dir_fd",
                dir_fd,
                Some(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
            ),
        ]
        .map(|(name, ty, default)| Parameter {
            name: name.into(),
            ty,
            default,
            sign_only: false,
            kind: if name == "dir_fd" {
                ParameterKind::KeywordOnly
            } else {
                ParameterKind::PositionalOrKeyword
            },
        });
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index == 0 {
                checker.fspath_argument(arg)
            } else {
                checker.expr_expected(arg, params[index].ty)
            }
        })?;
        Ok(self.ordered_builtin(
            Builtin::Process(ProcessOperation::Fd(FdOperation::Open)),
            Type::Int,
            args,
        ))
    }
}
