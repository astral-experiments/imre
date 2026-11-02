//! Bind timed file handlers without rewriting or reordering their arguments.
use super::calls::parameter;
use super::*;
use ir::TimedOperation as Op;

fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Timed(operation), args),
    }
}

impl Checker<'_> {
    pub(super) fn clock_time(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let integer = |name| parameter(name, Type::Int, Some(Expr::integer(0)));
        let mut fold = integer("fold");
        fold.kind = ParameterKind::KeywordOnly;
        let params = [
            integer("hour"),
            integer("minute"),
            integer("second"),
            integer("microsecond"),
            parameter("tzinfo", Type::None, Some(Expr::none())),
            fold,
        ];
        let args = self.arguments(call, &params, None).map_err(|error| {
            let zone = call
                .arguments
                .args
                .get(4)
                .or_else(|| call.arguments.find_keyword("tzinfo").map(|kw| &kw.value));
            if zone.is_some_and(|e| {
                self.expr(e)
                    .is_ok_and(|v| v.ty == Type::Calendar(ir::CalendarType::Zone))
            }) {
                self.object_builtin_error("fixed-offset datetime.time", call)
            } else {
                self.packed_calendar_error(call, 2, error)
            }
        })?;
        Ok(self.ordered_builtin(Builtin::Timed(Op::Clock), Type::ClockTime, args))
    }

    pub(super) fn timed_handler(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let clock = self.types.optional(Type::ClockTime);
        let off = Expr::boolean(false);
        let params = [
            parameter("filename", Type::Str, None),
            parameter(
                "when",
                Type::Str,
                Some(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str("h".into()),
                }),
            ),
            parameter("interval", Type::Int, Some(Expr::integer(1))),
            parameter("backupCount", Type::Int, Some(Expr::integer(0))),
            parameter("encoding", string, Some(Expr::absent(string))),
            parameter("delay", Type::Bool, Some(off.clone())),
            parameter("utc", Type::Bool, Some(off)),
            parameter("atTime", clock, Some(Expr::absent(clock))),
            parameter("errors", string, Some(Expr::absent(string))),
        ];
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index == 0 {
                checker.path_argument(arg)
            } else {
                if index == 4
                    && !matches!(arg, ast::Expr::NoneLiteral(_))
                    && !matches!(arg, ast::Expr::StringLiteral(s) if matches!(
                        s.value.to_str().to_ascii_lowercase().as_str(), "utf8" | "utf-8" | "utf_8"))
                {
                    return Err(error(
                        checker.source,
                        arg,
                        "timed file handlers currently support UTF-8 encoding",
                    ));
                }
                checker.expr_expected(arg, params[index].ty)
            }
        })?;
        Ok(self.ordered_builtin(Builtin::Timed(Op::New), Type::TimedFileHandler, args))
    }

    pub(super) fn timed_attribute(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        if receiver.ty == Type::ClockTime {
            let index = ["hour", "minute", "second", "microsecond", "fold"]
                .iter()
                .position(|&field| field == name)
                .ok_or_else(|| error(self.source, attr, "unsupported datetime.time attribute"))?;
            return Ok(op(
                Op::ClockPart,
                Type::Int,
                vec![receiver, Expr::integer(index as u8)],
            ));
        }
        let (operation, ty) = match name {
            "stream" => (Op::Stream, self.types.optional(Type::TextIO)),
            "baseFilename" => (Op::Filename, Type::Str),
            "when" => (Op::When, Type::Str),
            "suffix" => (Op::Suffix, Type::Str),
            "interval" => (Op::Interval, Type::Int),
            "backupCount" => (Op::BackupCount, Type::Int),
            "rolloverAt" => (Op::RolloverAt, Type::Int),
            "utc" => (Op::Utc, Type::Bool),
            "delay" => (Op::Delay, Type::Bool),
            "atTime" => (Op::AtTime, self.types.optional(Type::ClockTime)),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "unsupported timed file handler attribute",
                ));
            }
        };
        Ok(op(operation, ty, vec![receiver]))
    }

    pub(super) fn timed_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if attr.attr.as_str() == "shouldRollover" {
            let args = self.arguments_with(
                call,
                &[parameter("record", Type::None, None)],
                Some(receiver),
                |checker, _, arg| checker.expr(arg),
            )?;
            return Ok(self.ordered_builtin(Builtin::Timed(Op::ShouldRollover), Type::Bool, args));
        }
        if attr.attr.as_str() == "computeRollover" {
            let args = self.arguments_with(
                call,
                &[parameter("currentTime", Type::Int, None)],
                Some(receiver),
                |checker, _, arg| checker.expr(arg),
            )?;
            let ty = args.values[1].ty;
            let operation = match ty {
                Type::Int => Op::ComputeInt,
                Type::Float => Op::ComputeFloat,
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "rollover time must be an int or float",
                    ));
                }
            };
            return Ok(self.ordered_builtin(Builtin::Timed(operation), ty, args));
        }
        let (operation, ty) = match (attr.attr.as_str(), call.arguments.is_empty()) {
            ("doRollover", true) => (Op::DoRollover, Type::None),
            ("close", true) => (Op::Close, Type::None),
            ("flush", true) => (Op::Flush, Type::None),
            ("getFilesToDelete", true) => (Op::FilesToDelete, self.types.list(Type::Str)),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "unsupported timed file handler method or arguments",
                ));
            }
        };
        Ok(op(operation, ty, vec![receiver]))
    }
}
