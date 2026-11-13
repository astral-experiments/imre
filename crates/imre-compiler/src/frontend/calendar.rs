//! Bind fixed-offset calendar operations in Python evaluation order.
use super::calls::parameter;
use super::*;
use ir::{CalendarOp as Op, CalendarType as C};
pub(super) enum ParserBinding<'a> {
    Local(&'a [ast::Stmt]),
    Global(DefinitionId),
}
pub(super) fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Calendar(operation), args),
    }
}
/// Calendar values and optional calendar values use the same handle or zero for None.
pub(super) fn equality(types: &Types, values: &mut Vec<Expr>, ops: &[ast::CmpOp]) -> Option<Expr> {
    let ([left, right], [cmp @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)]) = (values.as_slice(), ops)
    else {
        return None;
    };
    let operands = [left.ty, right.ty].map(|ty| types.payload(ty).unwrap_or(ty));
    if !operands
        .iter()
        .all(|ty| matches!(ty, Type::None | Type::Calendar(_)))
        || !operands.iter().any(|ty| matches!(ty, Type::Calendar(_)))
    {
        return None;
    }
    let equal = op(Op::NullableEqual, Type::Bool, std::mem::take(values));
    Some(if *cmp == ast::CmpOp::NotEq {
        Expr::not(equal)
    } else {
        equal
    })
}

pub(super) fn format_builtin(name: &str, args: Vec<Expr>) -> Expr {
    let text = if name == "repr" {
        Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Repr, args),
        }
    } else if args[0].ty == Type::Calendar(C::Zone) {
        op(Op::ZoneName, Type::Str, args)
    } else {
        op(Op::Text { iso: false }, Type::Str, args)
    };
    if name == "print" {
        Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::Print, vec![text]),
        }
    } else {
        text
    }
}

pub(super) fn binary(
    a: C,
    b: C,
    left: Expr,
    right: Expr,
    node: &ast::ExprBinOp,
    source: &str,
) -> std::result::Result<Expr, String> {
    use ast::Operator::{Add, Sub};
    let (kind, reverse, subtract) = match (a, b, node.op) {
        (kind @ (C::Date | C::DateTime | C::Delta), C::Delta, Add | Sub) => {
            (kind, false, node.op == Sub)
        }
        (C::Delta, date @ (C::Date | C::DateTime), Add) => (date, true, false),
        _ => {
            return Err(diagnostic_message(
                source,
                node,
                "unsupported arithmetic between these datetime values",
            ));
        }
    };
    Ok(op(
        Op::Shift { reverse, subtract },
        Type::Calendar(kind),
        vec![left, right],
    ))
}
pub(super) fn utc() -> Expr {
    op(Op::Utc, Type::Calendar(C::Zone), vec![])
}

fn fixed_zone_method(name: &str) -> bool {
    matches!(name, "utcoffset" | "tzname" | "dst")
}

pub(super) fn boxed_comparison(receiver: Type, name: &str) -> bool {
    matches!(receiver, Type::Calendar(C::Date | C::DateTime))
        && matches!(
            name,
            "__eq__" | "__ne__" | "__lt__" | "__le__" | "__gt__" | "__ge__"
        )
}

pub(super) fn boxed_date_arithmetic(receiver: Type, name: &str) -> bool {
    receiver == Type::Calendar(C::Date)
        && matches!(name, "__add__" | "__radd__" | "__sub__" | "__rsub__")
}

pub(super) fn date_isoformat(receiver: Type, name: &str) -> bool {
    receiver == Type::Calendar(C::Date) && name == "isoformat"
}

pub(super) fn boxed_member(receiver: Type, name: &str) -> bool {
    match name {
        "replace" | "__replace__" | "strptime" if receiver == Type::Calendar(C::Date) => true,
        "utctimetuple" => receiver == Type::Calendar(C::DateTime),
        "min" | "max" | "resolution" | "__reduce__" | "__reduce_ex__" => {
            matches!(
                receiver,
                Type::Calendar(C::Date | C::DateTime | C::Delta) | Type::ClockTime
            )
        }
        "strftime" | "__format__" => matches!(
            receiver,
            Type::Calendar(C::Date | C::DateTime) | Type::ClockTime
        ),
        "__str__" | "__repr__" => receiver == Type::Calendar(C::Zone),
        "__mod__" | "__rmod__" | "__divmod__" | "__rdivmod__" => {
            receiver == Type::Calendar(C::Delta)
        }
        "fromisoformat" | "strptime" => {
            matches!(
                receiver,
                Type::Calendar(C::Date | C::DateTime) | Type::ClockTime
            )
        }
        "fromtimestamp" => receiver == Type::Calendar(C::Date),
        "ctime" | "fromisocalendar" | "fromordinal" | "isocalendar" | "isoweekday" | "today" => {
            matches!(receiver, Type::Calendar(C::Date | C::DateTime))
        }
        _ => boxed_comparison(receiver, name) || boxed_date_arithmetic(receiver, name),
    }
}

impl Checker<'_> {
    pub(super) fn month_grid_call(
        &mut self,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let (op, names, returns): (_, &[&str], _) = match name {
            "imre._calendar.month_grid" => (
                Op::InstanceMonthGrid,
                &["year", "month", "firstweekday"],
                self.types.list(self.types.list(Type::Int)),
            ),
            "calendar.monthcalendar" => (
                Op::MonthGrid,
                &["year", "month"],
                self.types.list(self.types.list(Type::Int)),
            ),
            "calendar.setfirstweekday" => (Op::SetFirstWeekday, &["firstweekday"], Type::None),
            _ => (Op::FirstWeekday, &[], Type::Int),
        };
        let params = names
            .iter()
            .map(|name| parameter(name, Type::Int, None))
            .collect::<Vec<_>>();
        let args = self
            .arguments_with(call, &params, None, |checker, _, source| {
                let value = checker.expr(source)?;
                if value.ty != Type::Int {
                    return Err(checker.object_runtime_export(name, source));
                }
                Ok(value)
            })
            .map_err(|_| self.object_runtime_export(name, call))?;
        Ok(self.ordered_builtin(Builtin::Calendar(op), returns, args))
    }

    pub(super) fn calendar_class_attribute(
        &self,
        a: &ast::ExprAttribute,
        base: Option<Binding>,
    ) -> Result<Option<Expr>, String> {
        let kind = match base {
            Some(Binding::Intrinsic(Intrinsic::Calendar(kind))) => Type::Calendar(kind),
            Some(Binding::Intrinsic(Intrinsic::ClockTime)) => Type::ClockTime,
            _ => return Ok(None),
        };
        if matches!(a.attr.as_str(), "__module__" | "__name__" | "__qualname__")
            || (kind == Type::Calendar(C::Zone) && fixed_zone_method(a.attr.as_str()))
            || (kind == Type::Calendar(C::DateTime) && a.attr.as_str() == "astimezone")
            || date_isoformat(kind, a.attr.as_str())
            || boxed_member(kind, a.attr.as_str())
        {
            return Err(self.object_runtime_export("datetime class member", a));
        }
        if base != Some(Binding::Intrinsic(Intrinsic::Calendar(C::Zone))) {
            return Ok(None);
        }
        Ok(match a.attr.as_str() {
            "utc" => Some(utc()),
            "min" | "max" => Some(op(
                Op::ZoneLimit,
                Type::Calendar(C::Zone),
                vec![Expr::boolean(a.attr.as_str() == "max")],
            )),
            _ => None,
        })
    }
    /// A packed calendar state is the first positional argument to an alternate constructor.
    pub(super) fn packed_calendar_error(
        &mut self,
        call: &ast::ExprCall,
        max_positional: usize,
        error: Failure,
    ) -> Failure {
        if let [state, ..] = call.arguments.args.as_ref()
            && call.arguments.args.len() <= max_positional
            && self.expr(state).is_ok_and(|value| {
                matches!(value.ty, Type::Bytes | Type::Str | Type::StrLiteral(_))
            })
        {
            self.object_builtin_error("packed datetime constructor", call)
        } else {
            error
        }
    }

    pub(super) fn calendar_constructor(
        &mut self,
        kind: C,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let int = |name: &str, optional: bool| {
            parameter(name, Type::Int, optional.then(|| Expr::integer(0)))
        };
        let (builtin, args) = match kind {
            C::Date => {
                let pars = ["year", "month", "day"].map(|name| int(name, false));
                let args = self
                    .arguments_with(call, &pars, None, |c, _, e| c.integer_index(e))
                    .map_err(|error| self.packed_calendar_error(call, 1, error))?;
                (Op::Date, args)
            }
            C::Delta => {
                let pars = [
                    "days",
                    "seconds",
                    "microseconds",
                    "milliseconds",
                    "minutes",
                    "hours",
                    "weeks",
                ]
                .map(|n| int(n, true));
                let args = self.arguments_with(call, &pars, None, |c, _, e| {
                    let v = powers::integer(Self::enum_integer(c.expr(e)?));
                    if matches!(v.ty, Type::Int | Type::Float) {
                        Ok(v)
                    } else {
                        Err(error(
                            c.source,
                            e,
                            "timedelta components currently require int, bool, or float",
                        ))
                    }
                })?;
                (Op::Delta, args)
            }
            C::DateTime => {
                let optzone = self.types.optional(Type::Calendar(C::Zone));
                let mut fold = int("fold", true);
                fold.kind = ParameterKind::KeywordOnly;
                let pars = [
                    int("year", false),
                    int("month", false),
                    int("day", false),
                    int("hour", true),
                    int("minute", true),
                    int("second", true),
                    int("microsecond", true),
                    parameter("tzinfo", optzone, Some(Expr::absent(optzone))),
                    fold,
                ];
                let args = self
                    .arguments_with(call, &pars, None, |c, i, e| {
                        if i == 7 {
                            return c.expr_expected(e, optzone);
                        }
                        c.integer_index(e)
                    })
                    .map_err(|error| self.packed_calendar_error(call, 2, error))?;
                (Op::DateTime, args)
            }
            C::Zone => {
                let optname = self.types.optional(Type::Str);
                let pars = [
                    parameter("offset", Type::Calendar(C::Delta), None),
                    parameter("name", optname, Some(Expr::absent(optname))),
                ];
                let args = self.arguments_with(call, &pars, None, |c, i, e| {
                    if i == 1 {
                        let string = c.expr(e)?;
                        c.expect(Type::Str, string.ty, e)?;
                        Ok(Expr {
                            ty: optname,
                            kind: ExprKind::Present(Box::new(string)),
                        })
                    } else {
                        c.expr_expected(e, Type::Calendar(C::Delta))
                    }
                })?;
                (Op::Zone, args)
            }
        };
        Ok(self.ordered_builtin(Builtin::Calendar(builtin), Type::Calendar(kind), args))
    }
    /// Bind the public mail-date functions to the same fixed-offset calendar types as datetime.
    pub(super) fn email_date(
        &mut self,
        which: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let dt = Type::Calendar(C::DateTime);
        let flag = |name| parameter(name, Type::Bool, Some(Expr::boolean(false)));
        let (op, ty, args) = match which {
            Intrinsic::EmailParseDate => (
                Op::EmailParseDate,
                dt,
                self.arguments(call, &[parameter("data", Type::Str, None)], None)?,
            ),
            Intrinsic::EmailFormatDateTime => (
                Op::EmailFormatDateTime,
                Type::Str,
                self.arguments(call, &[parameter("dt", dt, None), flag("usegmt")], None)?,
            ),
            Intrinsic::EmailFormatDate => {
                let params = [
                    parameter("timeval", Type::None, Some(Expr::none())),
                    flag("localtime"),
                    flag("usegmt"),
                ];
                let args = self.arguments_with(call, &params, None, |c, i, node| {
                    if i != 0 {
                        return c.expr_expected(node, Type::Bool);
                    }
                    let value = c.expr(node)?;
                    let value = c.optional_scalar_value(value, Type::Int, node)?;
                    if !matches!(
                        c.types.payload(value.ty).unwrap_or(value.ty),
                        Type::None | Type::Int | Type::Float
                    ) {
                        return Err(error(
                            c.source,
                            node,
                            "formatdate requires an integer, float, or None timestamp",
                        ));
                    }
                    Ok(value)
                })?;
                (Op::EmailFormatDate, Type::Str, args)
            }
            _ => unreachable!(),
        };
        Ok(self.ordered_builtin(Builtin::Calendar(op), ty, args))
    }
    /// Recognize a saved built-in parser only when every reference calls it directly.
    pub(super) fn saved_datetime_parser<'a>(
        &mut self,
        statement: &'a ast::Stmt,
        binding: ParserBinding<'_>,
    ) -> Option<(&'a ast::Expr, Expr)> {
        let assignment = statement.as_assign_stmt()?;
        let [target @ ast::Expr::Name(name)] = assignment.targets.as_slice() else {
            return None;
        };
        let attr = assignment.value.as_attribute_expr()?;
        if attr.attr.as_str() != "fromisoformat"
            || self.resolve(&attr.value)
                != Some(Binding::Intrinsic(Intrinsic::Calendar(C::DateTime)))
        {
            return None;
        }
        let positional = |call: &ast::ExprCall| {
            call.arguments.keywords.is_empty()
                && matches!(call.arguments.args.as_slice(), [arg] if !arg.is_starred_expr())
        };
        let observed = match binding {
            ParserBinding::Local(suite) => local_functions::reads_function_value(
                suite,
                |expr| matches!(expr, ast::Expr::Name(n) if n.id == name.id),
                Some(target.range()),
                positional,
            ),
            ParserBinding::Global(definition) => {
                self.graph
                    .modules
                    .iter()
                    .enumerate()
                    .any(|(module, source)| {
                        local_functions::reads_function_value(
                            &source.parsed.syntax().body,
                            |expr| {
                                matches!(expr, ast::Expr::Name(_) | ast::Expr::Attribute(_))
                                    && self.graph.resolve(module, expr)
                                        == Some(Binding::Value(definition))
                            },
                            (module == definition.module).then_some(target.range()),
                            positional,
                        )
                    })
            }
        };
        if observed {
            return None;
        }
        self.graph.needs_object_runtime.set(true);
        let parser = self.builtin_callable(
            attr,
            None,
            Builtin::Calendar(Op::FromIsoFormat),
            vec![Type::Str],
            Type::Calendar(C::DateTime),
        );
        Some((target, parser))
    }
    pub(super) fn datetime_fromisoformat(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        // A later operation on the result may need the full datetime protocol.
        self.graph.needs_object_runtime.set(true);
        if let [arg] = call.arguments.args.as_slice()
            && !arg.is_starred_expr()
            && call.arguments.keywords.is_empty()
        {
            let text = self.expr(arg)?;
            if matches!(text.ty, Type::Str | Type::StrLiteral(_)) {
                return Ok(op(
                    Op::FromIsoFormat,
                    Type::Calendar(C::DateTime),
                    vec![text],
                ));
            }
        }
        Err(self.object_runtime_export("datetime.fromisoformat", call))
    }

    pub(super) fn calendar_now(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let zone = self.types.optional(Type::Calendar(C::Zone));
        let args = self.arguments(
            call,
            &[parameter("tz", zone, Some(Expr::absent(zone)))],
            None,
        )?;
        Ok(self.ordered_builtin(
            Builtin::Calendar(Op::Now),
            Type::Calendar(C::DateTime),
            args,
        ))
    }

    pub(super) fn calendar_combine(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let tz = self.types.optional(Type::Calendar(C::Zone));
        let pars = [
            parameter("date", Type::Calendar(C::Date), None),
            parameter("time", Type::ClockTime, None),
            parameter("tzinfo", tz, Some(Expr::absent(tz))),
        ];
        let args = self.arguments(call, &pars, None)?;
        Ok(self.ordered_builtin(
            Builtin::Calendar(Op::Combine),
            Type::Calendar(C::DateTime),
            args,
        ))
    }
    pub(super) fn calendar_attribute(
        &mut self,
        kind: C,
        recv: Expr,
        a: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let name = a.attr.as_str();
        if (kind == C::Zone && (fixed_zone_method(name) || matches!(name, "min" | "max" | "utc")))
            || (kind == C::DateTime && name == "astimezone")
        {
            return Err(self.object_runtime_export("timezone member", a));
        }
        if kind == C::DateTime && name == "tzinfo" {
            let ty = self.types.optional(Type::Calendar(C::Zone));
            return Ok(op(Op::TzInfo, ty, vec![recv]));
        }
        let names: &[&str] = match kind {
            C::Date => &["year", "month", "day"],
            C::DateTime => &[
                "year",
                "month",
                "day",
                "hour",
                "minute",
                "second",
                "microsecond",
                "fold",
            ],
            C::Delta => &["days", "seconds", "microseconds"],
            C::Zone => &[],
        };
        let i = names
            .iter()
            .position(|&field| field == name)
            .ok_or_else(|| error(self.source, a, "unsupported datetime value attribute"))?;
        Ok(op(Op::Field, Type::Int, vec![recv, Expr::integer(i as u8)]))
    }
    pub(super) fn calendar_method(
        &mut self,
        kind: C,
        recv: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        if kind == C::DateTime && name == "replace" {
            let integer = self.types.optional(Type::Int);
            let zone = self.types.optional(Type::Calendar(C::Zone));
            let mut params = [
                "year",
                "month",
                "day",
                "hour",
                "minute",
                "second",
                "microsecond",
                "tzinfo",
                "fold",
            ]
            .map(|name| {
                let ty = if name == "tzinfo" { zone } else { integer };
                parameter(name, ty, Some(Expr::absent(ty)))
            });
            params[8].kind = ParameterKind::KeywordOnly;
            let mut timezone_supplied = false;
            let args = self.arguments_with(call, &params, Some(recv), |c, i, e| {
                if i == 7 {
                    timezone_supplied = true;
                    return c.expr_expected(e, zone);
                }
                c.integer_index(e)
            })?;
            return Ok(self.ordered_builtin(
                Builtin::Calendar(Op::Replace { timezone_supplied }),
                Type::Calendar(C::DateTime),
                args,
            ));
        }
        if kind == C::DateTime && name == "astimezone" {
            if call.arguments.is_empty() {
                return Err(self.object_runtime_export("system local timezone", call));
            }
            let zone = Type::Calendar(C::Zone);
            let args = self.arguments_with(
                call,
                &[parameter("tz", zone, None)],
                Some(recv),
                |c, _, e| {
                    let value = c.expr(e)?;
                    if value.ty == zone {
                        Ok(value)
                    } else {
                        Err(c.object_runtime_export("datetime.astimezone timezone argument", e))
                    }
                },
            )?;
            return Ok(self.ordered_builtin(
                Builtin::Calendar(Op::Astimezone),
                Type::Calendar(C::DateTime),
                args,
            ));
        }
        if kind == C::Zone && fixed_zone_method(name) {
            if name != "dst"
                && let [value] = call.arguments.args.as_ref()
                && call.arguments.keywords.is_empty()
            {
                let arg = self.expr(value)?;
                if matches!(arg.ty, Type::None | Type::Calendar(C::DateTime)) {
                    let (operation, ty) = if name == "utcoffset" {
                        (Op::ZoneOffset, Type::Calendar(C::Delta))
                    } else {
                        (Op::ZoneName, Type::Str)
                    };
                    return Ok(op(operation, ty, vec![recv, arg]));
                }
            }
            return Err(self.object_runtime_export("timezone method call", call));
        }
        if !call.arguments.is_empty() {
            if date_isoformat(Type::Calendar(kind), name) {
                return Err(self.object_runtime_export("date.isoformat call", call));
            }
            return Err(error(
                self.source,
                call,
                "this datetime method currently takes no arguments",
            ));
        }
        let (operation, ty) = match (kind, name) {
            (C::Date | C::DateTime, "isoformat") => (Op::Text { iso: true }, Type::Str),
            (C::DateTime, "utcoffset") => {
                (Op::Offset, self.types.optional(Type::Calendar(C::Delta)))
            }
            (C::Delta, "total_seconds") => (Op::TotalSeconds, Type::Float),
            _ => return Err(error(self.source, attr, "unsupported datetime method")),
        };
        Ok(op(operation, ty, vec![recv]))
    }
}
