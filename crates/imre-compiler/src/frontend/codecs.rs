//! Native codec registry lookup and stateless byte/text conversion.
use super::*;

fn argument_type(ty: Type) -> Option<String> {
    Some(match ty {
        Type::Int | Type::Float | Type::Bool | Type::None | Type::Bytes => ty.to_string(),
        Type::List(_) | Type::ListInt => "list".into(),
        Type::Tuple(_) => "tuple".into(),
        Type::Dict(_) => "dict".into(),
        Type::Set(_) => "set".into(),
        _ => return None,
    })
}

impl Checker<'_> {
    /// Bind str.encode's two options after evaluating its receiver and arguments in source order.
    pub(super) fn str_encode(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if call.arguments.is_empty() {
            return Ok(Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::Encode, vec![receiver]),
            });
        }
        let mut values = vec![receiver];
        for node in &call.arguments.args {
            values.push(Self::string_value(self.expr(node)?));
        }
        for keyword in &call.arguments.keywords {
            if keyword.arg.is_none() {
                return Err(error(
                    self.source,
                    keyword,
                    "expanded keyword arguments are unsupported",
                ));
            }
            values.push(Self::string_value(self.expr(&keyword.value)?));
        }
        let positional = call.arguments.args.len();
        let count = values.len() - 1;
        if count > 2 {
            let kind = if positional == 0 {
                "keyword arguments"
            } else {
                "arguments"
            };
            return Ok(Self::type_error_after(
                values,
                Type::Bytes,
                format!("encode() takes at most 2 {kind} ({count} given)"),
            ));
        }
        let names = ["encoding", "errors"];
        let mut order = [0; 3];
        for (i, slot) in order.iter_mut().enumerate().take(positional + 1) {
            *slot = i;
        }
        for (i, kw) in call.arguments.keywords.iter().enumerate() {
            let key = kw.arg.as_ref().unwrap().as_str();
            let Some(slot) = names.iter().position(|name| *name == key).map(|i| i + 1) else {
                return Ok(Self::type_error_after(
                    values,
                    Type::Bytes,
                    format!("encode() got an unexpected keyword argument '{key}'"),
                ));
            };
            if order[slot] != 0 {
                return Ok(Self::type_error_after(
                    values,
                    Type::Bytes,
                    format!("argument for encode() given by name ('{key}') and position ({slot})"),
                ));
            }
            order[slot] = positional + i + 1;
        }
        for (slot, default) in [(1, "utf-8"), (2, "strict")] {
            if order[slot] == 0 {
                order[slot] = values.len();
                values.push(unpacking::string(default));
            }
            let ty = values[order[slot]].ty;
            if ty != Type::Str {
                let name = names[slot - 1];
                let ty = argument_type(ty).ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "encode() requires concrete encoding and error handler types",
                    )
                })?;
                return Ok(Self::type_error_after(
                    values,
                    Type::Bytes,
                    format!("encode() argument '{name}' must be str, not {ty}"),
                ));
            }
        }
        Ok(self.ordered_builtin(
            Builtin::StrEncodeOptions,
            Type::Bytes,
            ir::CallArgs {
                values,
                order: order.to_vec(),
                exhaustive_enum_match: false,
            },
        ))
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
            let ty = argument_type(name.ty).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "codec lookup needs a concrete encoding name",
                )
            })?;
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
