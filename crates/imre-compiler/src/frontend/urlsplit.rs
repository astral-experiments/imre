//! Bind URL splitting, result methods, and quoting with normal argument evaluation.
mod resolution;
mod urlencode;

use super::*;
use crate::ir::{TextOperation, UrlProperty};
use crate::types::{SPLIT_RESULT, SPLIT_RESULT_BYTES};

fn slot(name: &str, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty: Type::None,
        default,
        kind: ParameterKind::PositionalOrKeyword,
        sign_only: false,
    }
}

impl Checker<'_> {
    /// Bind the bytes-only public signature to the existing octet quoting operation.
    pub(super) fn url_quote_from_bytes(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [slot("bs", None), slot("safe", Some(unpacking::string("/")))];
        calls::reject_unpacking(call, self.source)?;
        let count = call.arguments.args.len();
        let mut bound = [count >= 1, count >= 2];
        let mut problem = None;
        for kw in &call.arguments.keywords {
            let key = kw.arg.as_deref().expect("expanded keywords were rejected");
            match params.iter().position(|p| p.name == key) {
                None => {
                    problem.get_or_insert_with(|| {
                        self.keyword_error_message(
                            format!(
                                "quote_from_bytes() got an unexpected keyword argument '{key}'"
                            ),
                            key,
                            params.iter().map(|p| unpacking::string(&p.name)).collect(),
                        )
                    });
                }
                Some(i) if std::mem::replace(&mut bound[i], true) => {
                    self.graph.checked_repeated_keyword(self.module, kw.range());
                    problem.get_or_insert_with(|| {
                        unpacking::string(format!(
                            "quote_from_bytes() got multiple values for argument '{key}'"
                        ))
                    });
                }
                _ => {}
            }
        }
        if count > 2 {
            problem.get_or_insert_with(|| {
                unpacking::string(format!("quote_from_bytes() takes from 1 to 2 positional arguments but {count} were given"))
            });
        } else if !bound[0] {
            problem.get_or_insert_with(|| {
                unpacking::string("quote_from_bytes() missing 1 required positional argument: 'bs'")
            });
        }
        if let Some(problem) = problem {
            return self.positional_argument_error(call, &["bs", "safe"], Type::Str, problem);
        }
        let mut args = self.arguments_with(call, &params, None, |checker, _, node| {
            checker.evaluated_argument(node)
        })?;
        let source = args.values[args.order[0]].ty;
        if source != Type::Bytes {
            if matches!(
                source,
                Type::ObjectValue | Type::Union(_) | Type::Optional(_) | Type::Document(_)
            ) {
                return Err(error(
                    self.source,
                    call,
                    "quote_from_bytes requires a concrete bytes operand",
                ));
            }
            return Ok(calls::type_error(
                Type::Str,
                args.values,
                "quote_from_bytes() expected bytes".into(),
            ));
        }
        let safe = args.order[1];
        args.values[safe] = Self::enum_text(args.values[safe].clone());
        if !matches!(
            args.values[safe].ty,
            Type::Str | Type::StrLiteral(_) | Type::Bytes
        ) {
            return Err(error(
                self.source,
                call,
                "unsupported quote_from_bytes safe operand",
            ));
        }
        // Bytes do not take an encoding or an error handler.
        for _ in 0..2 {
            args.order.push(args.values.len());
            args.values.push(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            });
        }
        Ok(self.ordered_builtin(Builtin::Text(TextOperation::UrlQuote), Type::Str, args))
    }

    /// Bind concrete strings and bytes while preserving keyword evaluation order.
    pub(super) fn url_quoting(
        &mut self,
        call: &ast::ExprCall,
        op: Intrinsic,
    ) -> Result<Expr, String> {
        let quote = op != Intrinsic::Unquote;
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let mut params = vec![slot("string", None)];
        if quote {
            let safe = if op == Intrinsic::Quote { "/" } else { "" };
            params.push(slot("safe", Some(unpacking::string(safe))));
        }
        let (encoding, errors) = if quote {
            (none.clone(), none)
        } else {
            (unpacking::string("utf-8"), unpacking::string("replace"))
        };
        params.extend([
            slot("encoding", Some(encoding)),
            slot("errors", Some(errors)),
        ]);
        let args = self.arguments_with(call, &params, None, |checker, _, node| {
            let value = checker.expr(node)?;
            checker.optional_scalar_value(value, Type::Str, node)
        })?;
        for (i, &index) in args.order.iter().enumerate() {
            let ty = args.values[index].ty;
            let valid = if i == 0 || (quote && i == 1) {
                matches!(ty, Type::Str | Type::StrLiteral(_) | Type::Bytes)
            } else {
                matches!(
                    self.types.payload(ty).unwrap_or(ty),
                    Type::Str | Type::StrLiteral(_) | Type::None
                )
            };
            if !valid {
                return Err(error(
                    self.source,
                    call,
                    &format!("unsupported URL quoting type for {}", params[i].name),
                ));
            }
        }
        let source = &args.values[args.order[0]];
        if !quote && call.arguments.len() == 1 && source.ty != Type::Bytes {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    Builtin::Text(TextOperation::Unquote),
                    vec![source.clone()],
                ),
            });
        }
        let operation = match op {
            Intrinsic::Quote => TextOperation::UrlQuote,
            Intrinsic::QuotePlus => TextOperation::UrlQuotePlus,
            _ => TextOperation::UrlUnquote,
        };
        Ok(self.ordered_builtin(Builtin::Text(operation), Type::Str, args))
    }

    pub(super) fn url_split(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
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

    /// A SplitResult and a five-field tuple share the same native record layout.
    pub(super) fn url_unsplit(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let mut args = self.arguments_with(
            call,
            &[slot("components", None)],
            None,
            |checker, _, node| checker.expr(node),
        )?;
        let ty = args.values[0].ty;
        let fields = self.types.fields(ty).ok_or_else(|| {
            error(
                self.source,
                call,
                "urlunsplit requires five str or bytes fields",
            )
        })?;
        let ty = if fields.len() == 5
            && fields
                .iter()
                .all(|ty| matches!(ty, Type::Str | Type::StrLiteral(_) | Type::StrEnum(_)))
        {
            if fields.iter().any(|ty| matches!(ty, Type::StrEnum(_))) {
                let mut scope = calls::ArgumentScope::default();
                let tuple = scope.hold(self, args.values[0].clone());
                let components = fields
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| {
                        Self::enum_text(Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(tuple.clone()), index),
                        })
                    })
                    .collect();
                args.values[0] = scope.finish(Expr {
                    ty: self.types.tuple(vec![Type::Str; 5]),
                    kind: ExprKind::Tuple(components),
                });
            }
            Type::Str
        } else if fields.len() == 5 && fields.iter().all(|ty| *ty == Type::Bytes) {
            Type::Bytes
        } else {
            return Err(error(
                self.source,
                call,
                "urlunsplit requires five str fields or five bytes fields",
            ));
        };
        Ok(self.ordered_builtin(Builtin::Text(TextOperation::UrlGetUrl), ty, args))
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
