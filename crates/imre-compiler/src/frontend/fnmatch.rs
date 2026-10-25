//! Bind public fnmatch calls in evaluation order while preserving str and bytes owners.
use super::*;
use crate::ir::FnmatchOperation;

impl Checker<'_> {
    pub(super) fn fnmatch_call(
        &mut self,
        op: FnmatchOperation,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let names: &[&str] = match op {
            FnmatchOperation::Match => &["name", "pat"],
            FnmatchOperation::Filter { .. } => &["names", "pat"],
            FnmatchOperation::Translate => &["pat"],
        };
        let params: Vec<_> = names
            .iter()
            .map(|name| Parameter {
                name: (*name).into(),
                ty: Type::Str,
                kind: ParameterKind::PositionalOrKeyword,
                default: None,
                sign_only: false,
            })
            .collect();
        let args = self.arguments_with(call, &params, None, |checker, i, source| {
            let filter_input = i == 0 && matches!(op, FnmatchOperation::Filter { .. });
            let value = if filter_input && matches!(source, ast::Expr::List(l) if l.elts.is_empty())
            {
                Expr {
                    ty: checker.types.list(Type::Str),
                    kind: ExprKind::List(Vec::new()),
                }
            } else {
                checker.expr(source)?
            };
            if filter_input {
                if !matches!(value.ty, Type::List(_))
                    || !matches!(
                        checker.types.element(value.ty),
                        Some(Type::Str | Type::Bytes)
                    )
                {
                    return Err(error(
                        checker.source,
                        source,
                        "fnmatch filtering requires a list of str or bytes",
                    ));
                }
            } else if op == FnmatchOperation::Translate {
                checker.expect(Type::Str, value.ty, source)?;
            } else if !matches!(value.ty, Type::Str | Type::Bytes) {
                return Err(error(
                    checker.source,
                    source,
                    "fnmatch requires str or bytes",
                ));
            }
            Ok(value)
        })?;
        let returns = match op {
            FnmatchOperation::Match => Type::Bool,
            FnmatchOperation::Filter { .. } => args.values[args.order[0]].ty,
            FnmatchOperation::Translate => Type::Str,
        };
        Ok(self.ordered_builtin(Builtin::Fnmatch(op), returns, args))
    }
}
