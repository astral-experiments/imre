//! Bind logging records without forcing heterogeneous field values into a homogeneous dictionary.
use super::*;
use ir::LogRecordOperation as Op;

pub(super) fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::LogRecord(operation), args),
    }
}

fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

fn text(value: &str) -> Expr {
    Expr {
        ty: Type::Str,
        kind: ExprKind::Str(value.into()),
    }
}

impl Checker<'_> {
    /// Reject fields whose Python representation would call an unbound resource hook.
    pub(super) fn check_record_value(
        &self,
        value: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        self.check_record_value_type(value.ty, node)?;
        Ok(value)
    }

    fn check_record_value_type(&self, ty: Type, node: &impl Ranged) -> Result<(), String> {
        if ty != Type::ObjectValue && !dataclass_values::can_format(ty, self.types, self.classes) {
            return Err(error(
                self.source,
                node,
                "log record value needs a native Python representation",
            ));
        }
        Ok(())
    }

    fn empty_record(&self) -> Expr {
        let args = Expr {
            ty: self.types.tuple(vec![]),
            kind: ExprKind::Tuple(vec![]),
        };
        op(
            Op::New,
            Type::LogRecord,
            vec![
                none(),
                none(),
                text(""),
                logging::number(0),
                text(""),
                args,
                none(),
                none(),
                none(),
            ],
        )
    }

    pub(super) fn log_record(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params: Vec<_> = [
            "name", "level", "pathname", "lineno", "msg", "args", "exc_info", "func", "sinfo",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, name)| Parameter {
            name: name.into(),
            ty: Type::None,
            kind: ParameterKind::PositionalOrKeyword,
            default: (index >= 7).then(none),
            sign_only: false,
        })
        .collect();
        let args = self.arguments_with(call, &params, None, |checker, _, arg| {
            let value = checker.expr(arg)?;
            checker.check_record_value(value, arg)
        })?;
        Ok(self.ordered_builtin(Builtin::LogRecord(Op::New), Type::LogRecord, args))
    }

    pub(super) fn make_log_record(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let mapping = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [arg]) if arg.arg.as_ref().is_some_and(|k| k.as_str() == "dict") => &arg.value,
            _ => return Err(error(self.source, call, "makeLogRecord takes one mapping")),
        };
        let mut body = Vec::new();
        let mut clear = Vec::new();
        let mut pairs = Vec::new();
        let mut dynamic = None;
        if let ast::Expr::Dict(dict) = mapping {
            for item in &dict.items {
                let key = item.key.as_ref().ok_or_else(|| {
                    error(
                        self.source,
                        dict,
                        "record field expansion requires a string-key mapping",
                    )
                })?;
                let key = self.expr_expected(key, Type::Str)?;
                let value = self.expr(&item.value)?;
                let value = self.check_record_value(value, &item.value)?;
                let k = self.hidden(key.ty);
                let v = self.hidden(value.ty);
                body.extend([Statement::Assign(k, key), Statement::Assign(v, value)]);
                clear.extend([k, v]);
                pairs.push((k, v));
            }
        } else {
            let value = self.expr(mapping)?;
            if !matches!(
                self.types.mapping(value.ty),
                Some((Type::Str, Type::ObjectValue))
            ) {
                self.check_record_value_type(value.ty, mapping)?;
            }
            if !matches!(self.types.mapping(value.ty), Some((Type::Str, _))) {
                return Err(error(
                    self.source,
                    mapping,
                    "makeLogRecord needs string field names",
                ));
            }
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            dynamic = Some(slot);
        }
        // Evaluate the mapping, including all side effects, before constructing the record.
        let record = self.hidden(Type::LogRecord);
        body.push(Statement::Assign(record, self.empty_record()));
        clear.push(record);
        for (key, value) in pairs {
            body.push(Statement::Expr(op(
                Op::Put,
                Type::None,
                vec![self.local(record), self.local(key), self.local(value)],
            )));
        }
        if let Some(mapping) = dynamic {
            body.push(Statement::Expr(op(
                Op::Update,
                Type::None,
                vec![self.local(record), self.local(mapping)],
            )));
        }
        Ok(Expr {
            ty: Type::LogRecord,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(self.local(record)),
            },
        })
    }

    /// Bind Logger.makeRecord in call order and apply extra only after the new record exists.
    pub(super) fn logger_make_record(
        &mut self,
        logger: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let params: Vec<_> = [
            "name", "level", "fn", "lno", "msg", "args", "exc_info", "func", "extra", "sinfo",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, name)| Parameter {
            name: name.into(),
            ty: Type::None,
            kind: ParameterKind::PositionalOrKeyword,
            default: (i >= 7).then(none),
            sign_only: false,
        })
        .collect();
        let args = self.arguments_with(call, &params, Some(logger), |checker, i, arg| {
            let value = checker.expr(arg)?;
            if i == 8 {
                let concrete = checker.types.payload(value.ty).unwrap_or(value.ty);
                if concrete != Type::None
                    && !matches!(checker.types.mapping(concrete), Some((Type::Str, _)))
                {
                    return Err(error(
                        checker.source,
                        arg,
                        "logging extra needs a string-key mapping",
                    ));
                }
                if let Some((_, field)) = checker.types.mapping(concrete) {
                    checker.check_record_value_type(field, arg)?;
                }
                return Ok(value);
            }
            checker.check_record_value(value, arg)
        })?;
        let mut scope = calls::ArgumentScope::default();
        let held: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let bound: Vec<_> = args.order.iter().map(|&slot| held[slot].clone()).collect();
        let new = op(
            Op::New,
            Type::LogRecord,
            [1, 2, 3, 4, 5, 6, 7, 8, 10]
                .map(|i| bound[i].clone())
                .into(),
        );
        let record = scope.hold(self, new);
        if bound[9].ty != Type::None {
            scope.hold(
                self,
                op(
                    Op::AddExtra,
                    Type::None,
                    vec![record.clone(), bound[9].clone()],
                ),
            );
        }
        Ok(scope.finish(record))
    }

    pub(super) fn log_record_field(
        &self,
        record: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        if name == "__dict__" {
            return Ok(op(
                Op::Dictionary,
                self.types.dict(Type::Str, Type::ObjectValue),
                vec![record],
            ));
        }
        let ty = match name {
            "name" | "funcName" | "exc_text" | "stack_info" | "processName" | "threadName"
            | "taskName" => self.types.optional(Type::Str),
            "levelno" | "thread" | "process" => self.types.optional(Type::Int),
            "msg" | "message" | "pathname" | "filename" | "module" | "levelname" | "asctime" => {
                Type::Str
            }
            "lineno" => Type::Int,
            "created" | "relativeCreated" | "msecs" => Type::Float,
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "record field has no statically known representation",
                ));
            }
        };
        Ok(op(Op::Read, ty, vec![record, text(name)]))
    }

    pub(super) fn log_record_method(
        &mut self,
        record: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if call.arguments.args.is_empty() && call.arguments.keywords.is_empty() {
            let kind = match attr.attr.as_str() {
                "getMessage" => Op::Message,
                "__str__" | "__repr__" => Op::Repr,
                _ => return Err(error(self.source, attr, "unsupported LogRecord method")),
            };
            return Ok(op(kind, Type::Str, vec![record]));
        }
        Err(error(self.source, call, "record method takes no arguments"))
    }
}
