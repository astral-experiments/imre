//! Bind binascii and the standard library's base64 wrappers in Python call order.

use super::*;

fn param(name: &str, ty: Type, kind: ParameterKind, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        kind,
        default,
        sign_only: false,
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn bytes_op(op: Builtin, args: Vec<Expr>) -> Expr {
    Expr {
        ty: Type::Bytes,
        kind: ExprKind::Builtin(op, args),
    }
}

impl Checker<'_> {
    /// Resolve the bytes buffer before invoking the seed's integer index protocol.
    pub(super) fn adler32(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let one = Expr {
            ty: Type::Int,
            kind: ExprKind::Int("1".into()),
        };
        let params = [
            param("data", Type::Bytes, ParameterKind::PositionalOnly, None),
            param("value", Type::Int, ParameterKind::PositionalOnly, Some(one)),
        ];
        let args = self.arguments_with(call, &params, None, |checker, _, node| {
            checker.evaluated_argument(node)
        })?;
        let mut scope = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let data = values[args.order[0]].clone();
        if data.ty != Type::Bytes {
            let name = self.known_nonbuffer_type(data.ty).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "adler32 requires a known native bytes-like input",
                )
            })?;
            return Ok(scope.finish(calls::type_error(
                Type::Int,
                vec![],
                format!("a bytes-like object is required, not '{name}'"),
            )));
        }
        let mut seed = values[args.order[1]].clone();
        if let Some(node) = call.arguments.args.get(1) {
            seed = self.index_word(seed, node, false)?;
        }
        Ok(scope.finish(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Adler32, vec![data, seed]),
        }))
    }

    pub(super) fn known_nonbuffer_type(&self, ty: Type) -> Option<&str> {
        match ty {
            Type::Str | Type::StrLiteral(_) => Some("str"),
            // Python classes can implement the buffer protocol.
            Type::Class(_) => None,
            _ => self.exact_argument_type_name(ty),
        }
    }

    fn base64_buffer(
        &self,
        input: Expr,
        decode: bool,
        wrapper: bool,
        at: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let ty = input.ty;
        if ty == Type::Bytes {
            return Ok(input);
        }
        if decode && matches!(ty, Type::Str | Type::StrLiteral(_)) {
            return Ok(bytes_op(Builtin::BinasciiAsciiInput, vec![input]));
        }
        let Some(name) = self.known_nonbuffer_type(ty) else {
            return Err(error(
                self.source,
                at,
                if wrapper {
                    "base64 requires a concrete bytes-like input or ASCII text for decoding"
                } else {
                    "binascii requires a concrete bytes-like input or ASCII text for decoding"
                },
            ));
        };
        let message = if decode && wrapper {
            format!("argument should be a bytes-like object or ASCII string, not '{name}'")
        } else if decode {
            format!("argument should be bytes, buffer or ASCII string, not '{name}'")
        } else {
            format!("a bytes-like object is required, not '{name}'")
        };
        Ok(calls::type_error(Type::Bytes, vec![input], message))
    }

    pub(super) fn binascii_base64(
        &mut self,
        call: &ast::ExprCall,
        decode: bool,
    ) -> Result<Expr, String> {
        let params = [
            param("data", Type::Bytes, ParameterKind::PositionalOnly, None),
            param(
                if decode { "strict_mode" } else { "newline" },
                Type::Bool,
                ParameterKind::KeywordOnly,
                Some(boolean(!decode)),
            ),
        ];
        let args =
            self.arguments_with(call, &params, None, |c, _, arg| c.evaluated_argument(arg))?;
        let mut scope = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let input = self.base64_buffer(values[args.order[0]].clone(), decode, false, call)?;
        let flag = self.truth_value(values[args.order[1]].clone());
        let op = if decode {
            Builtin::BinasciiDecode
        } else {
            Builtin::BinasciiEncode
        };
        Ok(scope.finish(bytes_op(op, vec![input, flag])))
    }

    pub(super) fn base64_codec(
        &mut self,
        call: &ast::ExprCall,
        decode: bool,
    ) -> Result<Expr, String> {
        let kind = ParameterKind::PositionalOrKeyword;
        let mut params = vec![
            param("s", Type::Bytes, kind, None),
            param(
                "altchars",
                Type::None,
                kind,
                Some(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
            ),
        ];
        if decode {
            params.push(param("validate", Type::Bool, kind, Some(boolean(false))));
        }
        let args =
            self.arguments_with(call, &params, None, |c, _, arg| c.evaluated_argument(arg))?;
        let mut scope = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let input = self.base64_buffer(values[args.order[0]].clone(), decode, true, call)?;
        let data = if decode {
            input
        } else {
            bytes_op(Builtin::BinasciiEncode, vec![input, boolean(false)])
        };
        let alphabet = values[args.order[1]].clone();
        let data = if alphabet.ty == Type::None {
            data
        } else {
            // The original wrapper converts the input (or encodes it) before
            // validating altchars. This also roots it across later callbacks.
            let data = scope.hold(self, data);
            if let Some(payload) = self.types.payload(alphabet.ty) {
                let absent = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![alphabet.clone()]),
                };
                let present = Expr {
                    ty: payload,
                    kind: ExprKind::Unwrap(Box::new(alphabet)),
                };
                let translated = self.base64_alphabet(data.clone(), present, decode, call)?;
                Expr {
                    ty: Type::Bytes,
                    kind: ExprKind::Conditional(
                        Box::new(absent),
                        Box::new(data),
                        Box::new(translated),
                    ),
                }
            } else {
                self.base64_alphabet(data, alphabet, decode, call)?
            }
        };
        let result = if decode {
            let flag = self.truth_value(values[args.order[2]].clone());
            bytes_op(Builtin::BinasciiDecode, vec![data, flag])
        } else {
            data
        };
        Ok(scope.finish(result))
    }

    // b64encode checks the length and formats the actual value before bytes.maketrans
    // checks the buffer protocol. Reuse native len and repr for exact non-buffer types.
    fn bad_encode_alphabet(&self, alphabet: Expr, name: &str) -> Expr {
        let length = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![alphabet.clone()]),
        };
        let two = Expr {
            ty: Type::Int,
            kind: ExprKind::Int("2".into()),
        };
        let correct_length = Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(vec![length, two], vec![Compare::Equal]),
        };
        let buffer_error = calls::type_error(
            Type::Bytes,
            vec![],
            format!("a bytes-like object is required, not '{name}'"),
        );
        let shown = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Repr, vec![alphabet]),
        };
        let assertion = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("AssertionError").unwrap(),
                message: Some(Box::new(shown)),
            },
        };
        let length_error = bytes_op(Builtin::RaiseException, vec![assertion]);
        Expr {
            ty: Type::Bytes,
            kind: ExprKind::Conditional(
                Box::new(correct_length),
                Box::new(buffer_error),
                Box::new(length_error),
            ),
        }
    }

    fn base64_alphabet(
        &self,
        data: Expr,
        alphabet: Expr,
        decode: bool,
        at: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let alphabet = if decode {
            self.base64_buffer(alphabet, true, true, at)?
        } else {
            match alphabet.ty {
                Type::Bytes => alphabet,
                ty @ (Type::Str
                | Type::StrLiteral(_)
                | Type::ListInt
                | Type::List(_)
                | Type::Dict(_)
                | Type::Tuple(_))
                    if self.has_builtin_repr(ty) =>
                {
                    let name = if matches!(ty, Type::Str | Type::StrLiteral(_)) {
                        Some("str")
                    } else {
                        self.exact_argument_type_name(ty)
                    };
                    if let Some(name) = name {
                        return Ok(self.bad_encode_alphabet(alphabet, name));
                    }
                    return Err(error(
                        self.source,
                        at,
                        "base64 encode altchars requires a known buffer or exact builtin container",
                    ));
                }
                ty @ (Type::Int | Type::Bool | Type::Float | Type::PlainObject) => {
                    let name = self.exact_argument_type_name(ty).unwrap();
                    return Ok(calls::type_error(
                        Type::Bytes,
                        vec![alphabet],
                        format!("object of type '{name}' has no len()"),
                    ));
                }
                _ => {
                    return Err(error(
                        self.source,
                        at,
                        "base64 encode altchars requires concrete bytes or None",
                    ));
                }
            }
        };
        Ok(bytes_op(
            Builtin::Base64Translate,
            vec![data, alphabet, boolean(decode)],
        ))
    }
}
