//! Native codec registry lookup and stateless byte/text conversion.
use super::*;
use ir::CodecStreamOperation as Stream;

pub(super) fn writer_op(op: Stream, ty: Type, values: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::CodecStream(op), values),
    }
}

pub(super) fn writer_attribute(receiver: &Expr, name: &str) -> Option<Expr> {
    let (op, ty) = match (receiver.ty, name) {
        (Type::CodecInfo, "streamwriter") => (Stream::Class, Type::CodecWriterClass),
        (Type::CodecWriterClass, "encoding") => (Stream::Encoding, Type::Str),
        (Type::CodecWriter, "stream") => (Stream::Stream, Type::BinaryIO),
        (Type::CodecWriter, "errors") => (Stream::Errors, Type::Str),
        _ => return None,
    };
    Some(writer_op(op, ty, vec![receiver.clone()]))
}

fn parameter(name: &str, ty: Type, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        default,
        kind: ParameterKind::PositionalOrKeyword,
        sign_only: false,
    }
}

impl Checker<'_> {
    pub(super) fn codec_getwriter(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let args = self.arguments(call, &[parameter("encoding", Type::Str, None)], None)?;
        let entry = self.ordered_builtin(Builtin::CodecLookup, Type::CodecInfo, args);
        Ok(writer_op(
            Stream::Class,
            Type::CodecWriterClass,
            vec![entry],
        ))
    }

    pub(super) fn codec_writer_new(
        &mut self,
        call: &ast::ExprCall,
        class: Expr,
    ) -> Result<Expr, String> {
        let parameters = [
            parameter("stream", Type::BinaryIO, None),
            parameter("errors", Type::Str, Some(unpacking::string("strict"))),
        ];
        let args = self.arguments(call, &parameters, Some(class))?;
        Ok(self.ordered_builtin(Builtin::CodecStream(Stream::New), Type::CodecWriter, args))
    }

    pub(super) fn codec_writer_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (op, parameters, owner) = match attr.attr.as_str() {
            "write" => (
                Builtin::CodecStream(Stream::Write),
                vec![parameter("object", Type::Str, None)],
                receiver,
            ),
            "reset" => (Builtin::CodecStream(Stream::Reset), vec![], receiver),
            "flush" => (
                Builtin::TextFlush,
                vec![],
                writer_op(Stream::Stream, Type::BinaryIO, vec![receiver]),
            ),
            "close" => (
                Builtin::TextClose,
                vec![],
                writer_op(Stream::Stream, Type::BinaryIO, vec![receiver]),
            ),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "unsupported codec stream writer method",
                ));
            }
        };
        let args = self.arguments(call, &parameters, Some(owner))?;
        Ok(self.ordered_builtin(op, Type::None, args))
    }

    pub(super) fn codec_lookup(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if !call.arguments.keywords.is_empty() {
            return self.sys_type_error(
                call,
                Type::CodecInfo,
                "_codecs.lookup() takes no keyword arguments".into(),
            );
        }
        let [name] = call.arguments.args.as_slice() else {
            return self.sys_type_error(
                call,
                Type::CodecInfo,
                format!(
                    "_codecs.lookup() takes exactly one argument ({} given)",
                    call.arguments.args.len()
                ),
            );
        };
        let name = self.expr(name)?;
        if !matches!(name.ty, Type::Str | Type::StrLiteral(_)) {
            let ty = match name.ty {
                Type::Int | Type::Float | Type::Bool | Type::None | Type::Bytes => {
                    name.ty.to_string()
                }
                Type::List(_) | Type::ListInt => "list".into(),
                Type::Tuple(_) => "tuple".into(),
                Type::Dict(_) => "dict".into(),
                Type::Set(_) => "set".into(),
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "codec lookup needs a concrete encoding name",
                    ));
                }
            };
            return self.sys_type_error(
                call,
                Type::CodecInfo,
                format!("lookup() argument must be str, not {ty}"),
            );
        }
        Ok(Expr {
            ty: Type::CodecInfo,
            kind: ExprKind::Builtin(Builtin::CodecLookup, vec![Self::string_value(name)]),
        })
    }

    /// Codecs return (converted, consumed). Capture the input once before the
    /// error handler expression so its length and conversion see the same value.
    pub(super) fn codec_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (op, input_type, result_type) = match attr.attr.as_str() {
            "encode" => (Builtin::CodecEncode, Type::Str, Type::Bytes),
            "decode" => (Builtin::CodecDecode, Type::Bytes, Type::Str),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "codec entry supports native encode and decode",
                ));
            }
        };
        self.reject_builtin_keywords(call)?;
        let (input, errors) = match call.arguments.args.as_slice() {
            [input] => (input, unpacking::string("strict")),
            [input, errors] => (input, self.expr_expected(errors, Type::Str)?),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "codec conversion takes a value and an optional error handler",
                ));
            }
        };
        let input = self.expr_expected(input, input_type)?;
        let entry_slot = self.hidden(Type::CodecInfo);
        let input_slot = self.hidden(input_type);
        let converted = Expr {
            ty: result_type,
            kind: ExprKind::Builtin(
                op,
                vec![self.local(entry_slot), self.local(input_slot), errors],
            ),
        };
        let consumed = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![self.local(input_slot)]),
        };
        let tuple_type = self.types.tuple(vec![result_type, Type::Int]);
        Ok(Expr {
            ty: tuple_type,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(entry_slot, receiver),
                    Statement::Assign(input_slot, input),
                ],
                value: Box::new(Expr {
                    ty: tuple_type,
                    kind: ExprKind::Tuple(vec![converted, consumed]),
                }),
                clear: vec![input_slot, entry_slot],
            },
        })
    }
}
