//! Bind filesystem arguments in Python evaluation order, retaining the destination's type.

use super::*;

impl Checker<'_> {
    pub(super) fn shutil_rmtree(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let params = [
            ("path", Type::Str, None),
            (
                "ignore_errors",
                Type::Bool,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                }),
            ),
            ("onerror", Type::None, Some(none.clone())),
            ("onexc", Type::None, Some(none.clone())),
            ("dir_fd", Type::Int, Some(none)),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, ty, default))| Parameter {
            name: name.into(),
            ty,
            kind: if i < 3 {
                ParameterKind::PositionalOrKeyword
            } else {
                ParameterKind::KeywordOnly
            },
            default,
            sign_only: false,
        })
        .collect::<Vec<_>>();
        let args = self.arguments_with(call, &params, None, |checker, i, expr| {
            if i == 1 {
                return checker.expr_expected(expr, Type::Bool);
            }
            let v = checker.expr(expr)?;
            match (i, v.ty) {
                (0, Type::None) | (2 | 3, Type::None) | (4, Type::None | Type::Int | Type::Str) => {
                    Ok(v)
                }
                (0, _) => checker.fspath_value(v, expr),
                (4, _) if checker.types.payload(v.ty) == Some(Type::Int) => Ok(v),
                (4, _) => Err(error(
                    checker.source,
                    expr,
                    "rmtree dir_fd requires an integer or None",
                )),
                _ => Err(error(
                    checker.source,
                    expr,
                    "rmtree error callbacks require a supported callable",
                )),
            }
        })?;
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::RmTree),
            Type::None,
            args,
        ))
    }

    pub(super) fn shutil_copy(
        &mut self,
        op: ir::CopyOperation,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let params = [
            ("src", Type::Str, ParameterKind::PositionalOrKeyword, None),
            ("dst", Type::Str, ParameterKind::PositionalOrKeyword, None),
            (
                "follow_symlinks",
                Type::Bool,
                ParameterKind::KeywordOnly,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                }),
            ),
        ]
        .map(|(name, ty, kind, default)| Parameter {
            name: name.into(),
            ty,
            kind,
            default,
            sign_only: false,
        });
        let args = self.arguments_with(call, &params, None, |checker, i, expr| {
            if i == 2 {
                return checker.expr_expected(expr, Type::Bool);
            }
            let value = checker.expr(expr)?;
            if matches!(value.ty, Type::Str | Type::Bytes | Type::Path) {
                Ok(value)
            } else {
                Err(error(
                    checker.source,
                    expr,
                    "expected str, bytes or pathlib.Path",
                ))
            }
        })?;
        let dst = args.values[args.order[1]].ty;
        let result = match op {
            ir::CopyOperation::Mode | ir::CopyOperation::Stat | ir::CopyOperation::Xattr => {
                Type::None
            }
            ir::CopyOperation::Copy | ir::CopyOperation::Copy2 if dst == Type::Path => {
                self.types.union(&[Type::Str, Type::Path]).unwrap()
            }
            _ => dst,
        };
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::Copy(op)),
            result,
            args,
        ))
    }
}
