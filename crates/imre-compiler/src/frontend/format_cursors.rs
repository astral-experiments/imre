//! The two positional-only builtin operations used by string.Formatter.
use super::*;
impl Checker<'_> {
    pub(super) fn format_cursor(
        &mut self,
        call: &ast::ExprCall,
        split: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let (name, ty) = if split {
            let key = self.types.string_or_int();
            let row = self.types.tuple(vec![Type::Bool, key]);
            (
                "_string.formatter_field_name_split",
                self.types.tuple(vec![key, self.types.cursor(row)]),
            )
        } else {
            let text = self.types.optional(Type::Str);
            (
                "_string.formatter_parser",
                self.types
                    .cursor(self.types.tuple(vec![Type::Str, text, text, text])),
            )
        };
        let args = &call.arguments;
        if let ([arg], []) = (args.args.as_ref(), args.keywords.as_ref()) {
            let value = Self::string_value(self.expr(arg)?);
            if matches!(value.ty, Type::Str | Type::StrLiteral(_)) {
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::FormatCursor { split }, vec![value]),
                });
            }
            let Some(actual) = self.exact_argument_type_name(value.ty) else {
                return Err(error(
                    self.source,
                    arg,
                    "formatter requires str or an exact argument type",
                ));
            };
            self.graph.checked_overload(self.module, call.range());
            return Ok(calls::type_error(
                ty,
                vec![value],
                format!("expected str, got {actual}"),
            ));
        }
        self.one_positional_error(call, name, "string", ty)
    }
}
