//! Bind filesystem arguments in Python evaluation order, retaining the destination's type.

use super::*;

impl Checker<'_> {
    pub(super) fn shutil_copyfile(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
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
        let result = args.values[args.order[1]].ty;
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::CopyFile),
            result,
            args,
        ))
    }
}
