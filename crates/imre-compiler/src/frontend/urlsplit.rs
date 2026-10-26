//! Bind urllib.parse.urlsplit and its named tuple result using normal argument evaluation.
use super::*;
use crate::ir::{TextOperation, UrlProperty};
use crate::types::{SPLIT_RESULT, SPLIT_RESULT_BYTES};

impl Checker<'_> {
    pub(super) fn url_split(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let slot = |name: &str, default| Parameter {
            name: name.into(),
            ty: Type::None,
            default,
            kind: ParameterKind::PositionalOrKeyword,
            sign_only: false,
        };
        let params = [
            slot("url", None),
            slot("scheme", Some(unpacking::string(""))),
            slot(
                "allow_fragments",
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                }),
            ),
        ];
        let mut args =
            self.arguments_with(call, &params, None, |checker, _, node| checker.expr(node))?;
        for index in args.order[..2].iter().copied() {
            let ty = args.values[index].ty;
            if !matches!(ty, Type::Str | Type::StrLiteral(_) | Type::Bytes) {
                return Err(error(
                    self.source,
                    call,
                    "urlsplit requires str or bytes URL and scheme",
                ));
            }
        }
        let bytes = args.values[args.order[0]].ty == Type::Bytes;
        let flag = args.order[2];
        args.values[flag] = self.truth_value(args.values[flag].clone());
        Ok(self.ordered_builtin(
            Builtin::Text(TextOperation::UrlSplit),
            self.types.url_split_result(bytes),
            args,
        ))
    }

    pub(super) fn url_property(&self, receiver: &Expr, name: &str) -> Option<Expr> {
        let ty = match self.types.tuple_origin(receiver.ty)? {
            SPLIT_RESULT => Type::Str,
            SPLIT_RESULT_BYTES => Type::Bytes,
            _ => return None,
        };
        let property = match name {
            "username" => UrlProperty::Username,
            "password" => UrlProperty::Password,
            "hostname" => UrlProperty::Hostname,
            "port" => UrlProperty::Port,
            _ => return None,
        };
        let field = Expr {
            ty,
            kind: ExprKind::Field(Box::new(receiver.clone()), 1),
        };
        Some(Expr {
            ty: self
                .types
                .optional(if matches!(property, UrlProperty::Port) {
                    Type::Int
                } else {
                    ty
                }),
            kind: ExprKind::Builtin(
                Builtin::Text(TextOperation::UrlProperty(property)),
                vec![field],
            ),
        })
    }

    pub(super) fn url_result_method(
        &mut self,
        receiver: &Expr,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        let ty = match self.types.tuple_origin(receiver.ty) {
            Some(SPLIT_RESULT) => Type::Str,
            Some(SPLIT_RESULT_BYTES) => Type::Bytes,
            _ => return Ok(None),
        };
        if (ty == Type::Str && name == "encode") || (ty == Type::Bytes && name == "decode") {
            let params =
                [("encoding", "ascii"), ("errors", "strict")].map(|(name, default)| Parameter {
                    name: name.into(),
                    ty: Type::Str,
                    default: Some(unpacking::string(default)),
                    kind: ParameterKind::PositionalOrKeyword,
                    sign_only: false,
                });
            let args = self.arguments(call, &params, Some(receiver.clone()))?;
            return Ok(Some(self.ordered_builtin(
                Builtin::Text(TextOperation::UrlTranscode {
                    encode: ty == Type::Str,
                }),
                self.types.url_split_result(ty == Type::Str),
                args,
            )));
        }
        if name != "geturl" {
            return Ok(None);
        }
        if !call.arguments.is_empty() {
            return Err(error(self.source, call, "geturl takes no arguments"));
        }
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Builtin(
                Builtin::Text(TextOperation::UrlGetUrl),
                vec![receiver.clone()],
            ),
        }))
    }
}
