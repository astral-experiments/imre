//! Bind the positional buffer and the keyword-only base64 flag in Python call order.

use super::*;

impl Checker<'_> {
    pub(super) fn binascii_base64(
        &mut self,
        call: &ast::ExprCall,
        decode: bool,
    ) -> Result<Expr, String> {
        let params = [
            Parameter {
                name: "data".into(),
                ty: Type::Bytes,
                kind: ParameterKind::PositionalOnly,
                default: None,
                sign_only: false,
            },
            Parameter {
                name: if decode { "strict_mode" } else { "newline" }.into(),
                ty: Type::Bool,
                kind: ParameterKind::KeywordOnly,
                default: Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(!decode),
                }),
                sign_only: false,
            },
        ];
        let args =
            self.arguments_with(call, &params, None, |c, _, arg| c.evaluated_argument(arg))?;
        let ty = args.values[args.order[0]].ty;
        let text = matches!(ty, Type::Str | Type::StrLiteral(_));
        if ty != Type::Bytes && !(decode && text) {
            let name = match ty {
                Type::Str | Type::StrLiteral(_) => Some("str"),
                // User-defined classes can supply Python's buffer protocol.
                Type::Class(_) => None,
                _ => self.exact_argument_type_name(ty),
            };
            let Some(name) = name else {
                return Err(error(
                    self.source,
                    call,
                    "binascii requires a concrete bytes-like input or ASCII text for decoding",
                ));
            };
            let message = if decode {
                format!("argument should be bytes, buffer or ASCII string, not '{name}'")
            } else {
                format!("a bytes-like object is required, not '{name}'")
            };
            return Ok(calls::type_error(Type::Bytes, args.values, message));
        }
        // Evaluate both supplied expressions, then convert the input before calling
        // the flag's __bool__. CPython's argument converter has this ordering.
        let mut scope = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let mut input = values[args.order[0]].clone();
        if text {
            input = Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::BinasciiAsciiInput, vec![input]),
            };
        }
        let flag = self.truth_value(values[args.order[1]].clone());
        let op = if decode {
            Builtin::BinasciiDecode
        } else {
            Builtin::BinasciiEncode
        };
        Ok(scope.finish(Expr {
            ty: Type::Bytes,
            kind: ExprKind::Builtin(op, vec![input, flag]),
        }))
    }
}
