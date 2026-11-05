use super::*;
use crate::dynamic::complex::Complex;

/// Parse the string payload, then let a float subclass construct its result.
pub(super) fn fromhex(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let (&class, args) = args
        .split_first()
        .ok_or_else(|| type_error("descriptor 'fromhex' of 'float' object needs an argument"))?;
    super::super::stdlib::datetime::validate_classmethod_receiver(ctx, "float", "fromhex", class)?;
    let name = format!("{}.fromhex", ctx.class_qualname(class));
    let argument = one_positional_arg(&name, args, kwargs)?;
    let storage = ctx.scalar_value(argument);
    let number = match ctx.heap.get(storage) {
        Value::Str(text) => crate::floats::parse_hex(text)?,
        Value::SurrogateStr(_) => {
            ctx.encode_utf8_codepoints(storage, false, "strict")?;
            return Err(type_error("expected a Unicode string"));
        }
        _ => return Err(type_error("bad argument type for built-in operation")),
    };
    let result = ctx.alloc(Value::Float(number));
    if matches!(ctx.heap.get(class), Value::Native(name) if name == "float") {
        Ok(result)
    } else {
        ctx.call(class, vec![result], Vec::new())
    }
}

/// Report the representation of the native floating-point type named by Python.
pub(super) fn getformat(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some((&owner, args)) = args.split_first() else {
        return Err(type_error(
            "unbound method float.__getformat__() needs an argument",
        ));
    };
    let class = ctx
        .own_class_attr(owner, "__qualname__")
        .and_then(|id| ctx.str_value(id).ok())
        .unwrap_or_else(|| ctx.class_name(owner));
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "{class}.__getformat__() takes no keyword arguments"
        )));
    }
    let [kind] = args else {
        return Err(type_error(format!(
            "{class}.__getformat__() takes exactly one argument ({} given)",
            args.len()
        )));
    };
    let storage = ctx.scalar_value(*kind);
    let invalid = || {
        Error::new(
            "ValueError",
            "__getformat__() argument 1 must be 'double' or 'float'",
        )
    };
    let format = match ctx.heap.get(storage) {
        Value::Str(s) if s == "float" => {
            native_format(16_711_938.0_f32.to_ne_bytes(), [0x4b, 0x7f, 0x01, 0x02])
        }
        Value::Str(s) if s == "double" => native_format(
            9_006_104_071_832_581.0_f64.to_ne_bytes(),
            [0x43, 0x3f, 0xff, 0x01, 0x02, 0x03, 0x04, 0x05],
        ),
        Value::Str(s) if s.contains('\0') => {
            return Err(Error::new("ValueError", "embedded null character"));
        }
        Value::Str(_) => return Err(invalid()),
        Value::SurrogateStr(_) => {
            ctx.encode_utf8_codepoints(*kind, false, "strict")?;
            return Err(invalid());
        }
        _ => {
            return Err(type_error(format!(
                "__getformat__() argument must be str, not {}",
                ctx.type_name(*kind)
            )));
        }
    };
    Ok(ctx.text(format))
}

// Compare the same finite values and byte patterns as CPython's native layout probe.
fn native_format<const N: usize>(bytes: [u8; N], mut ieee_big: [u8; N]) -> &'static str {
    if bytes == ieee_big {
        return "IEEE, big-endian";
    }
    ieee_big.reverse();
    if bytes == ieee_big {
        "IEEE, little-endian"
    } else {
        "unknown"
    }
}

/// Read a complex component without invoking a float subclass's conversion hooks.
pub(super) fn component(
    ctx: &mut Context,
    field: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = noargs_method_receiver(ctx, "float", field, args, kwargs)?;
    let storage = ctx.scalar_value(receiver);
    let Value::Float(value) = *ctx.heap.get(storage) else {
        unreachable!("validated float receiver")
    };
    match field {
        "real" if receiver == storage => Ok(receiver),
        "real" => Ok(ctx.alloc(Value::Float(value))),
        "imag" => Ok(ctx.alloc(Value::Float(0.0))),
        _ => unreachable!("registered float component"),
    }
}

/// Return the reduced integer ratio of the receiver's stored binary64 value.
pub(super) fn as_integer_ratio(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let receiver = noargs_method_receiver(ctx, "float", "as_integer_ratio", args, kwargs)?;
    let Value::Float(value) = *ctx.heap.get(ctx.scalar_value(receiver)) else {
        unreachable!("validated float receiver")
    };
    if value.is_infinite() {
        return Err(Error::new(
            "OverflowError",
            "cannot convert Infinity to integer ratio",
        ));
    }
    if value.is_nan() {
        return Err(Error::new(
            "ValueError",
            "cannot convert NaN to integer ratio",
        ));
    }
    let (numerator, denominator) = if value.fract() == 0.0 {
        let integer = crate::Int::new(crate::floats::float_to_integer(value)?);
        (ctx.alloc(Value::Int(integer)), ctx.int(1))
    } else {
        // A non-integral binary64 has an odd numerator and a power-of-two denominator.
        let (mantissa, exponent, sign) = num_traits::Float::integer_decode(value);
        let zeros = mantissa.trailing_zeros();
        let numerator = i64::from(sign) * ((mantissa >> zeros) as i64);
        let power = -(exponent + zeros as i16);
        let denominator = crate::Int::new(num_bigint::BigInt::from(1) << power);
        (ctx.int(numerator), ctx.alloc(Value::Int(denominator)))
    };
    Ok(ctx.alloc(Value::Tuple(vec![numerator, denominator])))
}

/// Construct a boxed binary64 using Python's numeric slots or decimal text grammar.
pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("float() takes no keyword arguments"));
    }
    let value = match args {
        [] => return Ok(ctx.alloc(Value::Float(0.0))),
        [value] => *value,
        _ => {
            return Err(type_error(format!(
                "float expected at most 1 argument, got {}",
                args.len()
            )));
        }
    };
    if let Some(number) = from_numeric_slots(ctx, value)? {
        return Ok(number);
    }
    let number = parse_input(ctx, value)?;
    Ok(ctx.alloc(Value::Float(number)))
}

/// Read native strings directly, then request a contiguous buffer from other inputs.
fn parse_input(ctx: &mut Context, value: Id) -> Result<f64> {
    let parsed = match ctx.heap.get(ctx.scalar_value(value)) {
        Value::Str(text) => crate::floats::parse(text),
        Value::SurrogateStr(_) => None,
        Value::Bytes(bytes) | Value::ByteArray(bytes) => crate::floats::parse_bytes(bytes),
        _ => {
            let invalid_input = |ctx: &Context| {
                let name = ctx.type_name(value);
                type_error(format!(
                    "float() argument must be a string or a real number, not '{}'",
                    &name[..name.floor_char_boundary(200)]
                ))
            };
            if let Some(method) = ctx.buffer_slot(value, "__buffer__").flatten() {
                // Only acquisition errors are replaced. Format invalid input while the
                // export is live, so exceptions raised by __repr__ survive release.
                let view = ctx
                    .acquire_python_buffer(value, method, 0)
                    .and_then(|view| ctx.view_bytes(view).map(|_| view))
                    .map_err(|_| invalid_input(ctx))?;
                return ctx.with_python_buffer_export(value, view, |ctx| {
                    let parsed = crate::floats::parse_bytes(ctx.view_bytes(view)?);
                    parsed_number(ctx, value, parsed)
                });
            }
            let bytes = ctx
                .native_buffer_bytes(value)
                .map_err(|_| invalid_input(ctx))?;
            let parsed = crate::floats::parse_bytes(bytes);
            return ctx.with_native_bytes_export(value, |ctx, _| parsed_number(ctx, value, parsed));
        }
    };
    parsed_number(ctx, value, parsed)
}

fn parsed_number(ctx: &mut Context, value: Id, parsed: Option<f64>) -> Result<f64> {
    if let Some(number) = parsed {
        return Ok(number);
    }
    let repr = super::call(ctx, "repr", &[value], &[])?;
    let prefix = ctx.text("could not convert string to float: ");
    let message = ctx.concat_strings(prefix, repr)?;
    Err(ctx.exception("ValueError", &[message]))
}

/// Read the stored double of any float before consulting other numeric objects' slots.
pub(in crate::dynamic) fn from_numeric(ctx: &mut Context, value: Id) -> Result<Option<Id>> {
    let scalar = ctx.scalar_value(value);
    if matches!(ctx.heap.get(scalar), Value::Float(_)) {
        return Ok(Some(scalar));
    }
    from_numeric_slots(ctx, value)
}

/// float() and explicit complex components honor a float subclass's __float__ override.
pub(in crate::dynamic) fn from_numeric_slots(ctx: &mut Context, value: Id) -> Result<Option<Id>> {
    if matches!(ctx.heap.get(value), Value::Float(_)) {
        return Ok(Some(value));
    }
    if let Some(method) = ctx.bound_slot(value, "__float__")? {
        let result = ctx.call(method, Vec::new(), Vec::new())?;
        let scalar = ctx.scalar_value(result);
        if let Value::Float(number) = *ctx.heap.get(scalar) {
            if result != scalar {
                ctx.heap.reserve_temporaries(1)?;
                ctx.heap.keep(result);
                let owner = ctx.type_name(value);
                let returned = ctx.type_name(result);
                ctx.emit_builtin_warning(
                    crate::objects::exceptions::Kind::DeprecationWarning,
                    &format!(
                        "{}.__float__ returned non-float (type {}).  The ability to return an instance of a strict subclass of float is deprecated, and may be removed in a future version of Python.",
                        &owner[..owner.floor_char_boundary(50)],
                        &returned[..returned.floor_char_boundary(50)],
                    ),
                    1,
                )?;
                return Ok(Some(ctx.alloc(Value::Float(number))));
            }
            return Ok(Some(result));
        }
        return Err(type_error(format!(
            "{}.__float__ returned non-float (type {})",
            ctx.type_name(value),
            ctx.type_name(result),
        )));
    }
    let scalar = ctx.scalar_value(value);
    if matches!(ctx.heap.get(scalar), Value::Float(_)) {
        return Ok(Some(scalar));
    }
    if let Some(integer) = ctx.index_value(value)? {
        let number = crate::floats::int_to_float(&integer)?;
        return Ok(Some(ctx.alloc(Value::Float(number))));
    }
    Ok(None)
}

/// Sum a boxed iterable, retaining low-order bits independently on each numerical axis.
pub(super) fn sum(ctx: &mut Context, source: Id, start: Option<Id>) -> Result<Id> {
    let iterator = ctx.iterator(source)?;
    let mut total = start.unwrap_or_else(|| ctx.int(0));
    let unsuitable = match ctx.heap.get(ctx.scalar_value(total)) {
        Value::Str(_) => Some(("strings", "''")),
        Value::Bytes(_) => Some(("bytes", "b''")),
        Value::ByteArray(_) => Some(("bytearray", "b''")),
        _ => None,
    };
    if let Some((kind, empty)) = unsuitable {
        return Err(type_error(format!(
            "sum() can't sum {kind} [use {empty}.join(seq) instead]"
        )));
    }
    ctx.heap.keep(iterator);
    let mark = ctx.heap.temporary_mark();
    let next_item = |ctx: &mut Context| {
        ctx.heap.reserve_slot_or_request_collection();
        ctx.collect_garbage();
        ctx.next_item(iterator)
    };
    let Some(mut item) = next_item(ctx)? else {
        return Ok(total);
    };

    if let Value::Int(initial) = ctx.heap.get(total)
        && let Some(mut integer) = initial.to_i64()
    {
        loop {
            let value = match ctx.heap.get(item) {
                Value::Int(value) => value.to_i64(),
                Value::Bool(value) => Some(i64::from(*value)),
                _ => None,
            };
            let Some(next) = value.and_then(|value| integer.checked_add(value)) else {
                let accumulated = ctx.int(integer);
                total = ctx.binary("+", accumulated, item)?;
                break;
            };
            integer = next;
            ctx.heap.release_temporaries_since(mark);
            let Some(next) = next_item(ctx)? else {
                return Ok(ctx.int(integer));
            };
            item = next;
        }
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(total);
        let Some(next) = next_item(ctx)? else {
            return Ok(total);
        };
        item = next;
    }

    if let Value::Float(initial) = *ctx.heap.get(total) {
        let mut real = CompensatedSum::new(initial);
        loop {
            let value = match ctx.heap.get(item) {
                Value::Float(value) => Some(*value),
                _ => match ctx.heap.get(ctx.scalar_value(item)) {
                    Value::Int(integer) => Some(crate::floats::int_to_float(integer)?),
                    Value::Bool(value) => Some(f64::from(u8::from(*value))),
                    _ => None,
                },
            };
            let Some(value) = value else {
                let rounded = ctx.alloc(Value::Float(real.value()));
                total = ctx.binary("+", rounded, item)?;
                break;
            };
            real.add(value);
            ctx.heap.release_temporaries_since(mark);
            let Some(next) = next_item(ctx)? else {
                return Ok(ctx.alloc(Value::Float(real.value())));
            };
            item = next;
        }
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(total);
        let Some(next) = next_item(ctx)? else {
            return Ok(total);
        };
        item = next;
    }

    if let Value::Complex(initial) = *ctx.heap.get(total) {
        let mut real = CompensatedSum::new(initial.real);
        let mut imag = CompensatedSum::new(initial.imag);
        loop {
            let component = match ctx.heap.get(item) {
                Value::Complex(value) => {
                    imag.add(value.imag);
                    Some(value.real)
                }
                _ => ctx.real_component(ctx.scalar_value(item))?,
            };
            let Some(component) = component else {
                total = ctx.alloc(complex_total(real, imag));
                break;
            };
            real.add(component);
            ctx.heap.release_temporaries_since(mark);
            let Some(next) = next_item(ctx)? else {
                return Ok(ctx.alloc(complex_total(real, imag)));
            };
            item = next;
        }
    }

    loop {
        total = ctx.binary("+", total, item)?;
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.keep(total);
        let Some(next) = next_item(ctx)? else {
            return Ok(total);
        };
        item = next;
    }
}

/// Neumaier summation keeps the low-order contribution lost by binary64 addition.
struct CompensatedSum {
    high: f64,
    low: f64,
}

impl CompensatedSum {
    fn new(initial: f64) -> Self {
        Self {
            high: initial,
            low: 0.0,
        }
    }

    fn add(&mut self, value: f64) {
        let rounded = self.high + value;
        self.low += if self.high.abs() >= value.abs() {
            (self.high - rounded) + value
        } else {
            (value - rounded) + self.high
        };
        self.high = rounded;
    }

    /// Preserve signed zero and avoid turning an infinite sum into a NaN.
    fn value(self) -> f64 {
        if self.low != 0.0 && self.low.is_finite() {
            self.high + self.low
        } else {
            self.high
        }
    }
}

fn complex_total(real: CompensatedSum, imag: CompensatedSum) -> Value {
    Value::Complex(Complex {
        real: real.value(),
        imag: imag.value(),
    })
}
