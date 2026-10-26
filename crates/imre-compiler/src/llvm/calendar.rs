//! Lower exact immutable calendar objects using one conventional object handle.
use super::*;
use crate::ir::CalendarOp as Op;
impl Emitter<'_, '_> {
    pub(super) fn calendar_builtin(&mut self, op: Op, returns: Type, args: &[Operand]) -> Value {
        let get = |i: usize| &args[i].value;
        let flag = |yes| Value {
            ty: Type::Bool,
            operand: if yes { "true".into() } else { "false".into() },
            small_integer: false,
        };
        match op {
            Op::NullableEqual => {
                let a = self.word(get(0));
                let b = self.word(get(1));
                Value {
                    ty: Type::Bool,
                    operand: self.instruction(format!(
                        "call i1 @imre_calendar_equal_optional(i64 {a}, i64 {b})"
                    )),
                    small_integer: false,
                }
            }
            Op::Delta => {
                let mask = args.iter().enumerate().fold(0, |v, (i, a)| {
                    v | (i32::from(a.value.ty == Type::Float) << i)
                });
                let fields = args
                    .iter()
                    .map(|a| format!("i64 {}", self.word(&a.value)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let operand = self.checked_instruction(format!(
                    "call ptr @imre_calendar_delta(i32 {mask}, {fields})"
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
            Op::Shift { reverse, subtract } => {
                let order = usize::from(reverse);
                self.call(
                    "imre_calendar_shift",
                    returns,
                    &[get(order), get(1 - order), &flag(subtract)],
                )
            }
            Op::Text { iso } => self.call("imre_calendar_text", Type::Str, &[get(0), &flag(iso)]),
            Op::ZoneOffset => self.call("imre_calendar_zoneoffset", returns, &[get(0)]),
            Op::ZoneName => self.call("imre_calendar_zonename", returns, &[get(0)]),
            _ => {
                let name = match op {
                    Op::DateTime => "imre_calendar_datetime",
                    Op::Zone => "imre_calendar_zone",
                    Op::Utc => "imre_calendar_utc",
                    Op::ZoneLimit => "imre_calendar_zone_limit",
                    Op::Now => "imre_calendar_now",
                    Op::Negate => "imre_calendar_negate",
                    Op::Field => "imre_calendar_field",
                    Op::TzInfo => "imre_calendar_tzinfo",
                    Op::Offset => "imre_calendar_offset",
                    Op::TotalSeconds => "imre_calendar_seconds",
                    _ => unreachable!(),
                };
                self.call(
                    name,
                    returns,
                    &args.iter().map(|a| &a.value).collect::<Vec<_>>(),
                )
            }
        }
    }
}
