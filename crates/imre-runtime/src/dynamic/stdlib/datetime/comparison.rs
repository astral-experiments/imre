//! Clock comparisons whose offsets must be obtained from a Python tzinfo.
use super::*;

pub(super) fn offset(ctx: &mut Context, zone: Id, arg: Id) -> Result<Option<i128>> {
    let result = call_tzinfo(ctx, zone, "utcoffset", arg)?;
    Ok(calendar(ctx, result, "timedelta").map(|delta| delta.delta_microseconds()))
}

struct Clock {
    receiver: Id,
    value: Calendar,
    zone: Id,
}

impl Clock {
    fn get(ctx: &Context, receiver: Id) -> Option<Self> {
        match ctx.heap.get(ctx.scalar_value(receiver)) {
            Value::Calendar { value, tzinfo, .. }
                if matches!(value.class(), "datetime" | "time") =>
            {
                Some(Self {
                    receiver,
                    value: value.clone(),
                    zone: *tzinfo,
                })
            }
            _ => None,
        }
    }

    fn offset(&self, ctx: &mut Context) -> Result<Option<i128>> {
        offset(
            ctx,
            self.zone,
            if self.value.class() == "time" {
                NONE
            } else {
                self.receiver
            },
        )
    }

    // Internal copies use the original type without invoking its Python constructor.
    fn copy(&self, ctx: &mut Context, value: Calendar) -> Result<Id> {
        let copy = store(ctx, value, self.zone);
        if ctx.scalar_value(self.receiver) == self.receiver {
            return Ok(copy);
        }
        let class = ctx.exact_type(self.receiver)?;
        let instance = ctx.instance(class, None);
        if let Value::Instance { scalar, .. } = ctx.heap.get_mut(instance) {
            *scalar = Some(copy);
        }
        Ok(instance)
    }

    fn flipped_offset(&self, ctx: &mut Context) -> Result<Option<i128>> {
        let subtype = ctx.scalar_value(self.receiver) != self.receiver;
        if !subtype && !custom(ctx, self.zone) {
            return self.offset(ctx);
        }
        let mark = ctx.heap.temporary_mark();
        let result = (|| {
            let zone = calendar(ctx, self.zone, "timezone");
            let arg = if self.value.class() == "time" {
                // time passes None, but destruction of the fold-zero subtype can be observed.
                self.copy(
                    ctx,
                    Calendar::time(self.value.wall_microseconds(), 0, zone.as_ref())?,
                )?;
                NONE
            } else {
                let mut fields = self.value.fields().map(|v| v as i32);
                fields[7] ^= 1;
                self.copy(ctx, Calendar::datetime(fields, zone.as_ref())?)?
            };
            offset(ctx, self.zone, arg)
        })();
        ctx.heap.release_temporaries_since(mark);
        if let Err(error) = &result {
            for id in error.roots() {
                ctx.heap.keep(id);
            }
        }
        if subtype && ctx.heap.observes_released_roots() {
            ctx.collect_unreachable();
        }
        result
    }
}

pub(in crate::dynamic) fn custom(ctx: &Context, zone: Id) -> bool {
    zone != NONE && !matches!(ctx.heap.get(zone), Value::Calendar { .. })
}

fn pair(ctx: &mut Context, a: Id, b: Id) -> Option<(Clock, Clock)> {
    let left = Clock::get(ctx, a)?;
    let right = Clock::get(ctx, b)?;
    if left.value.class() != right.value.class()
        || !(custom(ctx, left.zone) || custom(ctx, right.zone))
    {
        return None;
    }
    ctx.heap.keep(a);
    ctx.heap.keep(b);
    Some((left, right))
}

fn difference(ctx: &mut Context, a: &Clock, b: &Clock) -> Result<i128> {
    let wall = a.value.wall_microseconds() - b.value.wall_microseconds();
    if a.zone == b.zone {
        return Ok(wall);
    }
    match (a.offset(ctx)?, b.offset(ctx)?) {
        (Some(left), Some(right)) => Ok(wall - left + right),
        (None, None) => Ok(wall),
        _ => Err(Error::new(
            "TypeError",
            "can't subtract offset-naive and offset-aware datetimes",
        )),
    }
}

pub(super) fn subtract(ctx: &mut Context, a: Id, b: Id) -> Result<Option<Calendar>> {
    let Some((left, right)) = pair(ctx, a, b) else {
        return Ok(None);
    };
    let micros = difference(ctx, &left, &right)?;
    Ok(Some(Calendar::from_delta_microseconds(&crate::Int::new(
        micros.into(),
    ))?))
}

pub(in crate::dynamic) fn compare(
    ctx: &mut Context,
    a: Id,
    b: Id,
    op: &str,
) -> Result<Option<bool>> {
    let Some((a, b)) = pair(ctx, a, b) else {
        return Ok(None);
    };
    let mut diff = a.value.wall_microseconds() - b.value.wall_microseconds();
    if a.zone != b.zone {
        let (left, right) = (a.offset(ctx)?, b.offset(ctx)?);
        let time = a.value.class() == "time";
        if left != right {
            match (left, right) {
                // The accelerator compares time offsets in whole seconds, including for negative deltas.
                (Some(l), Some(r)) if time => {
                    diff += (r.div_euclid(1_000_000) - l.div_euclid(1_000_000)) * 1_000_000
                }
                (Some(_), Some(_)) => diff = difference(ctx, &a, &b)?,
                _ if op == "==" => return Ok(Some(false)),
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "can't compare offset-naive and offset-aware {}s",
                            a.value.class()
                        ),
                    ));
                }
            }
        }
        if !time
            && diff == 0
            && op == "=="
            && (a.flipped_offset(ctx)? != left || b.flipped_offset(ctx)? != right)
        {
            return Ok(Some(false));
        }
    }
    Ok(Some(match op {
        "==" => diff == 0,
        "<" => diff < 0,
        "<=" => diff <= 0,
        ">" => diff > 0,
        ">=" => diff >= 0,
        _ => unreachable!(),
    }))
}

/// Direct comparison descriptors return NotImplemented for an unrelated operand.
pub(super) fn rich_method(
    ctx: &mut Context,
    kind: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&a, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{name}' of 'datetime.{kind}' object needs an argument"),
        ));
    };
    let left = calendar(ctx, a, kind).ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' requires a 'datetime.{kind}' object but received a '{}'",
                type_name(ctx, a)
            ),
        )
    })?;
    if kind == "date" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("wrapper {name}() takes no keyword arguments"),
            ));
        }
        let &[b] = rest else {
            return Err(Error::new(
                "TypeError",
                format!("expected 1 argument, got {}", rest.len()),
            ));
        };
        // A date descriptor accepts a datetime receiver, but declines a datetime on the right.
        let Some(right) = calendar(ctx, b, "date").filter(|right| right.class() == "date") else {
            return Ok(crate::dynamic::value::NOT_IMPLEMENTED);
        };
        let order = left.ordinal().cmp(&right.ordinal());
        return Ok(crate::dynamic::builtins::boolean(match name {
            "__eq__" => order.is_eq(),
            "__ne__" => order.is_ne(),
            "__lt__" => order.is_lt(),
            "__le__" => order.is_le(),
            "__gt__" => order.is_gt(),
            "__ge__" => order.is_ge(),
            _ => unreachable!(),
        }));
    }
    let b = crate::dynamic::builtins::one_positional_arg(name, rest, kwargs)?;
    let Some(right) = calendar(ctx, b, kind) else {
        return Ok(crate::dynamic::value::NOT_IMPLEMENTED);
    };
    let op = match name {
        "__eq__" | "__ne__" => "==",
        "__lt__" => "<",
        "__le__" => "<=",
        "__gt__" => ">",
        "__ge__" => ">=",
        _ => unreachable!(),
    };
    let result = if let Some(result) = compare(ctx, a, b, op)? {
        result
    } else if op == "==" {
        left.equal(&right)
    } else {
        let ordering = left.order(&right, op)?;
        match op {
            "<" => ordering.is_lt(),
            "<=" => ordering.is_le(),
            ">" => ordering.is_gt(),
            ">=" => ordering.is_ge(),
            _ => unreachable!(),
        }
    };
    Ok(crate::dynamic::builtins::boolean(
        result != (name == "__ne__"),
    ))
}

pub(in crate::dynamic) fn hash_offset(ctx: &mut Context, receiver: Id) -> Result<Option<i128>> {
    let clock = Clock::get(ctx, receiver).expect("clock");
    ctx.heap.keep(receiver);
    let flipped = if clock.value.class() == "datetime" {
        clock.value.fields()[7] == 1
    } else {
        ctx.scalar_value(receiver) != receiver && clock.value.clock_parts()[4] == 1
    };
    if flipped {
        clock.flipped_offset(ctx)
    } else {
        clock.offset(ctx)
    }
}
