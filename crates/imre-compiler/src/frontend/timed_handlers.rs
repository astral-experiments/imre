//! Bind timed file handlers without rewriting or reordering their arguments.
use super::*;
use ir::TimedOperation as Op;

fn param(name: &str, ty: Type, default: Option<Expr>) -> Parameter {
    Parameter {
        name: name.into(),
        ty,
        kind: ParameterKind::PositionalOrKeyword,
        default,
        sign_only: false,
    }
}

fn absent(ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Absent(Box::new(Expr {
            ty: Type::None,
            kind: ExprKind::None,
        })),
    }
}

fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Timed(operation), args),
    }
}

impl Checker<'_> {
    pub(super) fn clock_time(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let integer = |name| param(name, Type::Int, Some(logging::number(0)));
        let mut fold = integer("fold");
        fold.kind = ParameterKind::KeywordOnly;
        let params = [
            integer("hour"),
            integer("minute"),
            integer("second"),
            integer("microsecond"),
            param(
                "tzinfo",
                Type::None,
                Some(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
            ),
            fold,
        ];
        let args = self.arguments(call, &params, None)?;
        Ok(self.ordered_builtin(Builtin::Timed(Op::Clock), Type::ClockTime, args))
    }

    pub(super) fn timed_handler(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let string = self.types.optional(Type::Str);
        let clock = self.types.optional(Type::ClockTime);
        let off = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        };
        let params = [
            param("filename", Type::Str, None),
            param(
                "when",
                Type::Str,
                Some(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str("h".into()),
                }),
            ),
            param("interval", Type::Int, Some(logging::number(1))),
            param("backupCount", Type::Int, Some(logging::number(0))),
            param("encoding", string, Some(absent(string))),
            param("delay", Type::Bool, Some(off.clone())),
            param("utc", Type::Bool, Some(off)),
            param("atTime", clock, Some(absent(clock))),
            param("errors", string, Some(absent(string))),
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
                vec![receiver, logging::number(index as u8)],
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
                &[param("record", Type::None, None)],
                Some(receiver),
                |checker, _, arg| checker.expr(arg),
            )?;
            return Ok(self.ordered_builtin(Builtin::Timed(Op::ShouldRollover), Type::Bool, args));
        }
        if attr.attr.as_str() == "computeRollover" {
            let args = self.arguments_with(
                call,
                &[param("currentTime", Type::Int, None)],
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
