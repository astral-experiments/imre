//! Integer rounding functions used as first-class callables by runtime-loaded modules.

use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;

/// Dispatch the original operand's rounding method before converting a numeric fallback.
pub(super) fn round(
    ctx: &mut Context,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("math.{operation}() takes no keyword arguments"),
        ));
    }
    let &[value] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "math.{operation}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let slot = match operation {
        "trunc" => "__trunc__",
        "floor" => "__floor__",
        "ceil" => "__ceil__",
        _ => unreachable!(),
    };
    if let Some(method) = ctx.bound_numeric_slot(value, slot)? {
        return ctx.call(method, Vec::new(), Vec::new());
    }
    let scalar = ctx.scalar_value(value);
    let number = match *ctx.heap.get(scalar) {
        Value::Int(_) if scalar == value => return Ok(value),
        Value::Int(ref integer) => return Ok(ctx.alloc(Value::Int(integer.clone()))),
        Value::Bool(boolean) => return Ok(ctx.int(i64::from(boolean))),
        Value::Float(number) => number,
        _ if operation == "trunc" => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type {:.100} doesn't define __trunc__ method",
                    ctx.type_name(value)
                ),
            ));
        }
        _ => {
            let converted = builtins::from_numeric(ctx, value)?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!("must be real number, not {}", ctx.type_name(value)),
                )
            })?;
            let Value::Float(number) = *ctx.heap.get(converted) else {
                unreachable!()
            };
            number
        }
    };
    let rounded = match operation {
        "floor" => number.floor(),
        "ceil" => number.ceil(),
        _ => number,
    };
    Ok(ctx.alloc(Value::Int(crate::Int::new(
        crate::floats::float_to_integer(rounded)?,
    ))))
}
