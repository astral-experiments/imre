//! Convert local civil time and preserve the nine sequence fields of struct_time.

use super::*;
use crate::types::TupleOrigin;

impl Checker<'_> {
    pub(super) fn clock_parts(&mut self, call: &ast::ExprCall, utc: bool) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let stamp = match call.arguments.args.as_ref() {
            [] => Expr {
                ty: Type::Float,
                kind: ExprKind::Builtin(Builtin::WallTime, vec![]),
            },
            [arg] => {
                let val = self.expr(arg)?;
                match val.ty {
                    Type::Float => val,
                    Type::Int | Type::Bool => Expr {
                        ty: Type::Float,
                        kind: ExprKind::Builtin(Builtin::Float, vec![val]),
                    },
                    Type::None => self.evaluated_receiver(
                        Some(val),
                        Expr {
                            ty: Type::Float,
                            kind: ExprKind::Builtin(Builtin::WallTime, vec![]),
                        },
                    ),
                    _ => {
                        return Err(error(
                            self.source,
                            arg,
                            "a clock timestamp must be an integer or float",
                        ));
                    }
                }
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "a clock conversion takes at most one timestamp",
                ));
            }
        };
        Ok(Expr {
            ty: self.types.struct_time(),
            kind: ExprKind::Builtin(Builtin::ClockParts(utc), vec![stamp]),
        })
    }
}

fn field(record: Expr, index: usize, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Field(Box::new(record), index),
    }
}

fn op(builtin: Builtin, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(builtin, args),
    }
}

impl Checker<'_> {
    pub(super) fn mktime(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [arg] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "mktime takes one positional argument",
            ));
        };
        let value = self.expr(arg)?;
        let Some(fields) = self.types.fields(value.ty) else {
            return Ok(expandtabs::type_error_after(
                Type::Float,
                vec![value],
                "Tuple or struct_time argument required".into(),
            ));
        };
        if fields.len() != 9 {
            return Ok(expandtabs::type_error_after(
                Type::Float,
                vec![value],
                "mktime(): illegal time tuple argument".into(),
            ));
        }
        let ints = fields.iter().all(|&ty| ty == Type::Int);
        let offset = self.types.time_attributes(value.ty).and_then(|[_, ty]| {
            let payload = self.types.payload(ty)?;
            matches!(payload, Type::Class(_) | Type::IntEnum(_)).then_some((ty, payload))
        });
        if ints && offset.is_none() {
            return Ok(op(Builtin::Mktime, Type::Float, vec![value]));
        }
        let mut scope = calls::ArgumentScope::default();
        let record = scope.hold(self, value);
        let civil = if ints {
            record.clone()
        } else {
            let mut words = Vec::new();
            for (i, ty) in fields.into_iter().enumerate() {
                let input = field(record.clone(), i, ty);
                let word = self.index_word(input, arg, false)?;
                words.push(op(Builtin::TimeComponent, Type::Int, vec![word]));
            }
            let tuple = Expr {
                ty: self.types.tuple(vec![Type::Int; 9]),
                kind: ExprKind::Tuple(words),
            };
            scope.hold(self, tuple)
        };
        let mut args = vec![civil, record];
        if let Some((ty, payload)) = offset {
            let offset = scope.hold(self, op(Builtin::MktimeOffset, ty, args.clone()));
            let absent = op(Builtin::IsNone, Type::Bool, vec![offset.clone()]);
            let present = Expr {
                ty: payload,
                kind: ExprKind::Unwrap(Box::new(offset)),
            };
            let indexed = self.index_word(present, arg, false)?;
            args.push(Expr {
                ty: Type::Int,
                kind: ExprKind::Conditional(
                    Box::new(absent),
                    Box::new(pipelines::integer(0)),
                    Box::new(indexed),
                ),
            });
        }
        Ok(scope.finish(op(Builtin::Mktime, Type::Float, args)))
    }

    pub(super) fn struct_time_new(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [arg] = call.arguments.args.as_ref() else {
            return Err(error(self.source, call, "struct_time takes a sequence"));
        };
        let mut scope = calls::ArgumentScope::default();
        let fields = if let ast::Expr::List(list) = arg {
            let mut fields = Vec::new();
            for item in &list.elts {
                let value = self.expr(item)?;
                fields.push(scope.hold(self, value));
            }
            fields
        } else {
            let value = self.expr(arg)?;
            if let Some(element) = self.types.element(value.ty) {
                let source = scope.hold(self, value);
                let length = op(Builtin::Len, Type::Int, vec![source.clone()]);
                let checked = op(Builtin::TimeSequenceLength, Type::Int, vec![length]);
                let count = scope.hold(self, checked);
                let item = |n| {
                    op(
                        Builtin::ListGet,
                        element,
                        vec![source.clone(), pipelines::integer(n)],
                    )
                };
                let extra_ty = self.types.optional(element);
                let extra = |n| -> Result<Expr, String> {
                    let present = self.coerce(item(n), extra_ty, arg)?;
                    let absent = self.coerce(
                        Expr {
                            ty: Type::None,
                            kind: ExprKind::None,
                        },
                        extra_ty,
                        arg,
                    )?;
                    let test =
                        pipelines::compare(Compare::Greater, count.clone(), pipelines::integer(n));
                    Ok(Expr {
                        ty: extra_ty,
                        kind: ExprKind::Conditional(
                            Box::new(test),
                            Box::new(present),
                            Box::new(absent),
                        ),
                    })
                };
                let ty = self.types.namedtuple_layout(
                    TupleOrigin::StructTime(extra_ty, extra_ty),
                    vec![element; 9],
                );
                let value = Expr {
                    ty,
                    kind: ExprKind::Tuple((0..9).map(item).collect()),
                };
                let zone = extra(9)?;
                let offset = extra(10)?;
                let result = scope.hold(self, value);
                let attach = op(
                    Builtin::TimeExtras,
                    Type::None,
                    vec![result.clone(), zone, offset],
                );
                return Ok(scope.finish(self.evaluated_receiver(Some(attach), result)));
            }
            if !matches!(value.ty, Type::Tuple(_)) {
                if matches!(value.ty, Type::Int | Type::Bool | Type::Float | Type::None) {
                    return Ok(expandtabs::type_error_after(
                        self.types.struct_time(),
                        vec![value],
                        "constructor requires a sequence".into(),
                    ));
                }
                return Err(error(
                    self.source,
                    arg,
                    "struct_time needs a concrete sequence",
                ));
            }
            let types = self.types.fields(value.ty).unwrap();
            let source = scope.hold(self, value);
            types
                .into_iter()
                .enumerate()
                .map(|(i, ty)| field(source.clone(), i, ty))
                .collect()
        };
        let length = fields.len();
        if !(9..=11).contains(&length) {
            let bound = if length < 9 { "least 9" } else { "most 11" };
            let message = format!(
                "time.struct_time() takes an at {bound}-sequence ({length}-sequence given)"
            );
            return Ok(scope.finish(expandtabs::type_error_after(
                self.types.struct_time(),
                vec![],
                message,
            )));
        }
        let regular = self
            .types
            .time_attributes(self.types.struct_time())
            .unwrap();
        let extra_type = |index: usize| match fields.get(index + 9) {
            Some(value) if value.ty != Type::None => self.types.optional(value.ty),
            _ => regular[index],
        };
        let attrs = [extra_type(0), extra_type(1)];
        let ty = self.types.namedtuple_layout(
            TupleOrigin::StructTime(attrs[0], attrs[1]),
            fields[..9].iter().map(|e| e.ty).collect(),
        );
        let result = Expr {
            ty,
            kind: ExprKind::Tuple(fields[..9].to_vec()),
        };
        if fields.len() == 9 {
            return Ok(scope.finish(result));
        }
        let empty = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let zone = self.coerce(fields[9].clone(), attrs[0], arg)?;
        let offset = self.coerce(fields.get(10).cloned().unwrap_or(empty), attrs[1], arg)?;
        let result = scope.hold(self, result);
        let attach = op(
            Builtin::TimeExtras,
            Type::None,
            vec![result.clone(), zone, offset],
        );
        Ok(scope.finish(self.evaluated_receiver(Some(attach), result)))
    }

    pub(super) fn struct_time_field(
        &self,
        value: Expr,
        node: &ast::ExprAttribute,
        declaration: &crate::types::NamedTuple,
    ) -> Result<Expr, String> {
        match node.attr.as_str() {
            "tm_zone" | "tm_gmtoff" => {
                let zone = node.attr.as_str() == "tm_zone";
                Ok(op(
                    Builtin::TimeExtra(zone),
                    self.types.time_attributes(value.ty).unwrap()[usize::from(!zone)],
                    vec![value],
                ))
            }
            "n_fields" | "n_sequence_fields" | "n_unnamed_fields" => {
                let n = self.struct_time_class_field(node)?;
                Ok(self.evaluated_receiver(Some(value), n))
            }
            _ => self.namedtuple_instance_attribute(node, value, declaration),
        }
    }

    pub(super) fn struct_time_class_field(
        &self,
        node: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let n = match node.attr.as_str() {
            "n_fields" => 11,
            "n_sequence_fields" => 9,
            "n_unnamed_fields" => 0,
            "__name__" => return Ok(unpacking::string("struct_time")),
            "__module__" => return Ok(unpacking::string("time")),
            "__match_args__" => {
                return self.namedtuple_class_attribute(
                    node,
                    &self.types.tuple_metadata(self.types.struct_time()).unwrap(),
                );
            }
            _ => {
                return Err(error(
                    self.source,
                    node,
                    "unsupported time.struct_time class attribute",
                ));
            }
        };
        Ok(pipelines::integer(n))
    }
}
