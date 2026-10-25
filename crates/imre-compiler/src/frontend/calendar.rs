//! Bind fixed-offset calendar operations in Python evaluation order.
use super::*;
use ir::{CalendarOp as Op, CalendarType as C};
pub(super) fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Calendar(operation), args),
    }
}
fn par(name: &str, ty: Type, default: Option<Expr>) -> Parameter {
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
fn boolean(v: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(v),
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
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Not(Box::new(equal)),
        }
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
) -> Result<Expr, String> {
    use ast::Operator::{Add, Sub};
    let (reverse, subtract) = match (a, b, node.op) {
        (C::DateTime, C::Delta, Add) => (false, false),
        (C::DateTime, C::Delta, Sub) => (false, true),
        (C::Delta, C::DateTime, Add) => (true, false),
        _ => {
            return Err(error(
                source,
                node,
                "unsupported arithmetic between these datetime values",
            ));
        }
    };
    Ok(op(
        Op::Shift { reverse, subtract },
        Type::Calendar(C::DateTime),
        vec![left, right],
    ))
}
pub(super) fn utc() -> Expr {
    op(Op::Utc, Type::Calendar(C::Zone), vec![])
}

impl Checker<'_> {
    pub(super) fn calendar_class_attribute(&self, a: &ast::ExprAttribute) -> Option<Expr> {
        if self.resolve(&a.value) != Some(Binding::Intrinsic(Intrinsic::Calendar(C::Zone))) {
            return None;
        }
        match a.attr.as_str() {
            "utc" => Some(utc()),
            "min" | "max" => Some(op(
                Op::ZoneLimit,
                Type::Calendar(C::Zone),
                vec![boolean(a.attr.as_str() == "max")],
            )),
            _ => None,
        }
    }
    pub(super) fn calendar_constructor(
        &mut self,
        kind: C,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let int =
            |name: &str, optional: bool| par(name, Type::Int, optional.then(|| logging::number(0)));
        let (builtin, args) = match kind {
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
                    let v = powers::integer(c.expr(e)?);
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
                    par("tzinfo", optzone, Some(absent(optzone))),
                    fold,
                ];
                let args = self.arguments_with(call, &pars, None, |c, i, e| {
                    if i == 7 {
                        return c.expr_expected(e, optzone);
                    }
                    let v = powers::integer(c.expr(e)?);
                    c.expect(Type::Int, v.ty, e)?;
                    Ok(v)
                })?;
                (Op::DateTime, args)
            }
            C::Zone => {
                let optname = self.types.optional(Type::Str);
                let pars = [
                    par("offset", Type::Calendar(C::Delta), None),
                    par("name", optname, Some(absent(optname))),
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
    pub(super) fn calendar_now(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let args = self.arguments(call, &[par("tz", Type::Calendar(C::Zone), None)], None)?;
        Ok(self.ordered_builtin(
            Builtin::Calendar(Op::Now),
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
        if kind == C::DateTime && name == "tzinfo" {
            let ty = self.types.optional(Type::Calendar(C::Zone));
            return Ok(op(Op::TzInfo, ty, vec![recv]));
        }
        let names: &[&str] = match kind {
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
        Ok(op(
            Op::Field,
            Type::Int,
            vec![recv, logging::number(i as u8)],
        ))
    }
    pub(super) fn calendar_method(
        &mut self,
        kind: C,
        recv: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        if kind == C::Zone && matches!(name, "utcoffset" | "tzname") {
            let [e] = call.arguments.args.as_ref() else {
                return Err(error(
                    self.source,
                    call,
                    "timezone method requires one positional datetime or None",
                ));
            };
            if !call.arguments.keywords.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "timezone method takes a positional argument",
                ));
            }
            let arg = self.expr(e)?;
            if !matches!(arg.ty, Type::None | Type::Calendar(C::DateTime)) {
                return Err(error(
                    self.source,
                    e,
                    "timezone method requires a datetime or None",
                ));
            }
            let (operation, ty) = if name == "utcoffset" {
                (Op::ZoneOffset, Type::Calendar(C::Delta))
            } else {
                (Op::ZoneName, Type::Str)
            };
            return Ok(op(operation, ty, vec![recv, arg]));
        }
        if !call.arguments.is_empty() {
            return Err(error(
                self.source,
                call,
                "this datetime method currently takes no arguments",
            ));
        }
        let (operation, ty) = match (kind, name) {
            (C::DateTime, "isoformat") => (Op::Text { iso: true }, Type::Str),
            (C::DateTime, "utcoffset") => {
                (Op::Offset, self.types.optional(Type::Calendar(C::Delta)))
            }
            (C::Delta, "total_seconds") => (Op::TotalSeconds, Type::Float),
            _ => return Err(error(self.source, attr, "unsupported datetime method")),
        };
        Ok(op(operation, ty, vec![recv]))
    }
}
