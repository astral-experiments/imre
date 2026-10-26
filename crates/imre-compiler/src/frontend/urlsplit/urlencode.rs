//! Build ordered form fields from dictionaries and sequences of pairs.
use super::*;

struct FormEncoder {
    output: usize,
    safe: Expr,
    encoding: Expr,
    errors: Expr,
    doseq: Expr,
    quoting: TextOperation,
}

fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

impl Checker<'_> {
    /// Evaluate call arguments before iterating the query, including keyword arguments in source order.
    pub(in super::super) fn url_encode(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = [
            slot("query", None),
            slot("doseq", Some(boolean(false))),
            slot("safe", Some(unpacking::string(""))),
            slot("encoding", Some(none())),
            slot("errors", Some(none())),
            slot("quote_via", Some(boolean(true))),
        ];
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            let value = match index {
                0 if matches!(arg, ast::Expr::Dict(d) if d.items.is_empty()) => {
                    checker.expr_expected(arg, checker.types.dict(Type::Str, Type::Str))?
                }
                0 if matches!(arg, ast::Expr::List(l) if l.elts.is_empty()) => {
                    let pair = checker.types.tuple(vec![Type::Str, Type::Str]);
                    checker.expr_expected(arg, checker.types.list(pair))?
                }
                2..=4 => {
                    let value = checker.expr(arg)?;
                    checker.optional_scalar_value(value, Type::Str, arg)?
                }
                5 => match checker.resolve(arg) {
                    Some(Binding::Intrinsic(Intrinsic::QuotePlus)) => boolean(true),
                    Some(Binding::Intrinsic(Intrinsic::Quote)) => boolean(false),
                    _ => {
                        return Err(error(
                            checker.source,
                            arg,
                            "urlencode supports quote or quote_plus for quote_via",
                        ));
                    }
                },
                _ => checker.expr(arg)?,
            };
            Ok(value)
        })?;
        let mut scope = calls::ArgumentScope::default();
        let plus = matches!(args.values[args.order[5]].kind, ExprKind::Bool(true));
        let values = args
            .values
            .into_iter()
            .map(|value| match value.kind {
                ExprKind::Bool(_) | ExprKind::None => value,
                _ => scope.hold(self, value),
            })
            .collect::<Vec<_>>();
        let arg = |i: usize| values[args.order[i]].clone();
        let query = arg(0);
        let safe = arg(2);
        if !matches!(safe.ty, Type::Str | Type::StrLiteral(_) | Type::Bytes) {
            return Err(error(
                self.source,
                call,
                "urlencode safe requires str or bytes",
            ));
        }
        let (encoding, errors) = (arg(3), arg(4));
        for option in [&encoding, &errors] {
            if !matches!(
                self.types.payload(option.ty).unwrap_or(option.ty),
                Type::Str | Type::StrLiteral(_) | Type::None
            ) {
                return Err(error(
                    self.source,
                    call,
                    "urlencode encoding and errors require str or None",
                ));
            }
        }
        let items = self.types.mapping(query.ty).is_some();
        if !items
            && self.types.element(query.ty).is_none()
            && self.types.fields(query.ty).is_none()
            && self.types.var_tuple_element(query.ty).is_none()
        {
            return Err(error(
                self.source,
                call,
                "urlencode requires a mapping or a sequence of pairs",
            ));
        }
        let doseq = self.truth_value(arg(1));
        let doseq = if matches!(doseq.kind, ExprKind::Bool(_)) {
            doseq
        } else {
            scope.hold(self, doseq)
        };
        let pairs = self.types.list(Type::Str);
        let output = self.hidden(pairs);
        let encoder = FormEncoder {
            output,
            safe,
            encoding,
            errors,
            doseq,
            quoting: if plus {
                TextOperation::UrlQuotePlus
            } else {
                TextOperation::UrlQuote
            },
        };
        let mut body = vec![Statement::Assign(
            output,
            Expr {
                ty: pairs,
                kind: ExprKind::List(vec![]),
            },
        )];
        let input = self.iteration_value(query, items, call)?;
        body.extend(self.expand_items(input, call, |checker, pair| {
            encoder.pair(checker, pair, call)
        })?);
        Ok(scope.finish(Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(
                        Builtin::StringJoin,
                        vec![unpacking::string("&"), self.local(output)],
                    ),
                }),
                clear: vec![output],
            },
        }))
    }
}

impl FormEncoder {
    fn component(
        &self,
        checker: &Checker<'_>,
        value: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let value = Checker::enum_integer(Checker::enum_text(value));
        let binary = value.ty == Type::Bytes;
        let value = match value.ty {
            Type::Bytes | Type::Str | Type::StrLiteral(_) => value,
            ty if checker.has_builtin_repr(ty) && checker.types.payload(ty).is_none() => Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
            },
            _ => {
                return Err(error(
                    checker.source,
                    call,
                    "urlencode requires a known string conversion for each field",
                ));
            }
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::Text(self.quoting),
                vec![
                    value,
                    self.safe.clone(),
                    if binary {
                        none()
                    } else {
                        self.encoding.clone()
                    },
                    if binary { none() } else { self.errors.clone() },
                ],
            ),
        })
    }

    fn pair(
        &self,
        checker: &mut Checker<'_>,
        pair: Expr,
        call: &ast::ExprCall,
    ) -> Result<Vec<Statement>, String> {
        let fields = checker.types.fields(pair.ty).unwrap_or_default();
        let [key_ty, value_ty] = fields.as_slice() else {
            return Err(error(
                checker.source,
                call,
                "urlencode requires two-element key/value pairs",
            ));
        };
        let key = checker.hidden(Type::Str);
        let key_field = Expr {
            ty: *key_ty,
            kind: ExprKind::Field(Box::new(pair.clone()), 0),
        };
        let mut body = vec![Statement::Assign(
            key,
            self.component(checker, key_field, call)?,
        )];
        let value = Expr {
            ty: *value_ty,
            kind: ExprKind::Field(Box::new(pair), 1),
        };
        let append = |checker: &mut Checker<'_>, value: Expr| -> Result<Vec<Statement>, String> {
            let field = unpacking::concat(
                unpacking::concat(checker.local(key), unpacking::string("=")),
                self.component(checker, value, call)?,
            );
            Ok(vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::Append, vec![checker.local(self.output), field]),
            })])
        };
        let sequence = checker.types.element(*value_ty).is_some()
            || checker.types.fields(*value_ty).is_some()
            || checker.types.var_tuple_element(*value_ty).is_some()
            || checker.types.mapping(*value_ty).is_some();
        if sequence && !matches!(self.doseq.kind, ExprKind::Bool(false)) {
            let iter = checker.iteration_value(value.clone(), false, call)?;
            let many = checker.expand_items(iter, call, append)?;
            if matches!(self.doseq.kind, ExprKind::Bool(true)) {
                body.extend(many);
            } else {
                body.push(Statement::If(
                    self.doseq.clone(),
                    many,
                    append(checker, value)?,
                ));
            }
        } else {
            body.extend(append(checker, value)?);
        }
        Ok(vec![Statement::Scope {
            body,
            clear: vec![key],
        }])
    }
}
