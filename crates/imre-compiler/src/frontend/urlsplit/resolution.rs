//! Bind form queries and redirect references to their concrete native layouts.
use super::*;

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn text_or_bytes(ty: Type) -> bool {
    matches!(ty, Type::Str | Type::StrLiteral(_) | Type::Bytes)
}

impl Checker<'_> {
    pub(in crate::frontend) fn url_join(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [
            slot("base", None),
            slot("url", None),
            slot("allow_fragments", Some(boolean(true))),
        ];
        let mut args =
            self.arguments_with(call, &params, None, |checker, _, node| checker.expr(node))?;
        let base = &args.values[args.order[0]];
        let target = &args.values[args.order[1]];
        if !text_or_bytes(base.ty) || !text_or_bytes(target.ty) {
            return Err(error(
                self.source,
                call,
                "urljoin requires str or bytes references",
            ));
        }
        let base_bytes = base.ty == Type::Bytes;
        let url_bytes = target.ty == Type::Bytes;
        let result = if base_bytes == url_bytes {
            if base_bytes { Type::Bytes } else { Type::Str }
        } else {
            // Empty references return an original operand before mixed-layout coercion.
            self.types.union(&[Type::Str, Type::Bytes]).unwrap()
        };
        let flag = args.order[2];
        args.values[flag] = self.truth_value(args.values[flag].clone());
        Ok(self.ordered_builtin(Builtin::Text(TextOperation::UrlJoin), result, args))
    }

    pub(in crate::frontend) fn parse_qsl(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let params = [
            slot("qs", None),
            slot("keep_blank_values", Some(boolean(false))),
            slot("strict_parsing", Some(boolean(false))),
            slot("encoding", Some(unpacking::string("utf-8"))),
            slot("errors", Some(unpacking::string("replace"))),
            slot("max_num_fields", Some(none)),
            slot("separator", Some(unpacking::string("&"))),
        ];
        let args = self.arguments_with(call, &params, None, |checker, index, node| {
            let value = checker.expr(node)?;
            let ty = checker.types.payload(value.ty).unwrap_or(value.ty);
            match index {
                0 if !text_or_bytes(ty) && ty != Type::None => Err(error(
                    checker.source,
                    node,
                    "parse_qsl requires a string or bytes query",
                )),
                1 | 2 => Ok(checker.truth_value(value)),
                3 | 4 if !matches!(ty, Type::Str | Type::StrLiteral(_) | Type::None) => Err(error(
                    checker.source,
                    node,
                    "parse_qsl encoding and errors require strings",
                )),
                5 => checker.coerce(value, checker.types.optional(Type::Int), node),
                _ => Ok(value),
            }
        })?;
        let source = args.values[args.order[0]].ty;
        let field = if self.types.payload(source).unwrap_or(source) == Type::Bytes {
            Type::Bytes
        } else {
            Type::Str
        };
        let result = self.types.list(self.types.tuple(vec![field, field]));
        Ok(self.ordered_builtin(Builtin::Text(TextOperation::ParseQsl), result, args))
    }
}
