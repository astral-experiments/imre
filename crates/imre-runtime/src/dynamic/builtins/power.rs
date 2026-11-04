//! Three-argument pow dispatches the numeric slots of all three operand types.

use super::{Context, Id, NOT_IMPLEMENTED, Result, Value, type_error};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Slot {
    Integer,
    Float,
    Complex,
    Python,
}

impl Slot {
    fn native(ctx: &Context, operand: Id) -> Option<Self> {
        match ctx.heap.get(ctx.scalar_value(operand)) {
            Value::Int(_) | Value::Bool(_) => Some(Self::Integer),
            Value::Float(_) => Some(Self::Float),
            Value::Complex(_) => Some(Self::Complex),
            _ => None,
        }
    }

    fn of(ctx: &Context, operand: Id) -> Option<Self> {
        if ["__pow__", "__rpow__"]
            .into_iter()
            .any(|name| user_method(ctx, operand, name).is_some())
        {
            Some(Self::Python)
        } else {
            Self::native(ctx, operand)
        }
    }

    fn call(self, ctx: &mut Context, base: Id, exp: Id, modulus: Id) -> Result<Id> {
        match self {
            Self::Integer => {
                let (Ok(x), Ok(y), Ok(m)) =
                    (ctx.as_int(base), ctx.as_int(exp), ctx.as_int(modulus))
                else {
                    return Ok(NOT_IMPLEMENTED);
                };
                Ok(ctx.alloc(Value::Int(crate::integer_power::modular_power(&x, &y, &m)?)))
            }
            Self::Float => Err(type_error(
                "pow() 3rd argument not allowed unless all arguments are integers",
            )),
            Self::Complex => Ok(ctx
                .complex_power(base, exp, Some(modulus))?
                .unwrap_or(NOT_IMPLEMENTED)),
            Self::Python => python_power(ctx, base, exp, modulus),
        }
    }
}

/// A matching inherited wrapper retains the builtin slot. Any override of either
/// special method installs the shared Python power slot for both directions.
fn user_method(ctx: &Context, operand: Id, name: &str) -> Option<Id> {
    let class = ctx.heap.get(operand).instance_class()?;
    let method = ctx.find_in_mro(class, |base| match ctx.heap.get(base) {
        Value::Native(kind) if matches!(kind.as_str(), "int" | "bool" | "float" | "complex") => {
            Some(None)
        }
        _ => ctx.own_class_attr(base, name).map(Some),
    })??;
    if let Value::Native(symbol) = ctx.heap.get(method)
        && Slot::native(ctx, operand).is_some()
        && symbol.rsplit_once('.') == Some((ctx.type_name(ctx.scalar_value(operand)), name))
    {
        return None;
    }
    Some(method)
}

fn call_method(ctx: &mut Context, base: Id, exp: Id, modulus: Id, reflected: bool) -> Result<Id> {
    let (receiver, name, other) = if reflected {
        (exp, "__rpow__", base)
    } else {
        (base, "__pow__", exp)
    };
    if let Some(method) = user_method(ctx, receiver, name) {
        let class = ctx.exact_type(receiver)?;
        let bound = match ctx.bind_attribute(method, receiver, class) {
            Ok(bound) => bound,
            Err(error) if ctx.is_attribute_error(&error) => return Ok(NOT_IMPLEMENTED),
            Err(error) => return Err(error),
        };
        ctx.call(bound, vec![other, modulus], Vec::new())
    } else if let Some(native) = Slot::native(ctx, receiver) {
        native.call(ctx, base, exp, modulus)
    } else {
        Ok(NOT_IMPLEMENTED)
    }
}

/// Even when selected through the modulus, Python's slot only invokes methods
/// on the base or exponent. The modulus is always the final argument.
fn python_power(ctx: &mut Context, base: Id, exp: Id, modulus: Id) -> Result<Id> {
    let left = ctx.exact_type(base)?;
    let right = ctx.exact_type(exp)?;
    let mut reflect = left != right && Slot::of(ctx, exp) == Some(Slot::Python);
    if Slot::of(ctx, base) == Some(Slot::Python) {
        if reflect
            && ctx.direct_subclass(right, left)
            && ctx.reflected_slot_overridden(left, right, "__rpow__")?
        {
            let result = call_method(ctx, base, exp, modulus, true)?;
            if result != NOT_IMPLEMENTED {
                return Ok(result);
            }
            reflect = false;
        }
        let result = call_method(ctx, base, exp, modulus, false)?;
        if result != NOT_IMPLEMENTED {
            return Ok(result);
        }
    }
    if reflect {
        call_method(ctx, base, exp, modulus, true)
    } else {
        Ok(NOT_IMPLEMENTED)
    }
}

pub(super) fn ternary(ctx: &mut Context, base: Id, exp: Id, modulus: Id) -> Result<Id> {
    let left = Slot::of(ctx, base);
    let mut right = Slot::of(ctx, exp).filter(|&slot| Some(slot) != left);
    if left.is_some()
        && let Some(slot) = right
    {
        let base_type = ctx.exact_type(base)?;
        let exp_type = ctx.exact_type(exp)?;
        if ctx.direct_subclass(exp_type, base_type) {
            let result = slot.call(ctx, base, exp, modulus)?;
            if result != NOT_IMPLEMENTED {
                return Ok(result);
            }
            right = None;
        }
    }
    for slot in [left, right].into_iter().flatten() {
        let result = slot.call(ctx, base, exp, modulus)?;
        if result != NOT_IMPLEMENTED {
            return Ok(result);
        }
    }
    if let Some(slot) = Slot::of(ctx, modulus)
        && Some(slot) != left
        && Some(slot) != right
    {
        let result = slot.call(ctx, base, exp, modulus)?;
        if result != NOT_IMPLEMENTED {
            return Ok(result);
        }
    }
    Err(type_error(format!(
        "unsupported operand type(s) for ** or pow(): '{}', '{}', '{}'",
        ctx.type_name(base),
        ctx.type_name(exp),
        ctx.type_name(modulus)
    )))
}
