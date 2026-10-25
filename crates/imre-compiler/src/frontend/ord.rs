//! Builtin character ordinals and their exact, catchable call errors.

use super::*;

impl Checker<'_> {
    pub(super) fn ordinal_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        if let ([argument], []) = (args.args.as_ref(), args.keywords.as_ref()) {
            let value = self.expr(argument)?;
            self.graph.checked_overload(self.module, call.range());
            return self.ordinal(value, argument);
        }
        self.one_positional_error(call, "ord", "c", Type::Int)
    }

    /// Prepare one conversion; the same expression runs on each pull inside map.
    pub(super) fn ordinal(&self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        let value = Self::string_value(value);
        let ty = self.types.payload(value.ty).unwrap_or(value.ty);
        if matches!(ty, Type::Str | Type::StrLiteral(_) | Type::Bytes)
            || self.types.members(ty).is_some_and(|members| {
                members
                    .iter()
                    .all(|member| matches!(member, Type::Str | Type::Bytes))
            })
        {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Ord, vec![value]),
            });
        }
        let Some(actual) = self.exact_argument_type_name(value.ty) else {
            return Err(error(
                self.source,
                at,
                "ord requires a supported character or an exact argument type",
            ));
        };
        // CPython's %.200s type-name display discards a partial UTF-8 code point.
        let actual = &actual[..actual.floor_char_boundary(200)];
        let message = format!("ord() expected string of length 1, but {actual} found");
        Ok(calls::type_error(Type::Int, vec![value], message))
    }
}
