//! A decoder that carries a pending CR and newline history across delegate calls.

use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::dynamic::builtins::boolean;

pub(super) const METHODS: &[&str] = &[
    "__init__", "decode", "getstate", "setstate", "reset", "newlines",
];

#[derive(Clone, Copy)]
pub(in crate::dynamic) struct Decoder {
    pub(in crate::dynamic) delegate: Id,
    pub(in crate::dynamic) errors: Id,
    translate: bool,
    pending: bool,
    seen: u8,
}

fn state(ctx: &mut Context, receiver: Id) -> Result<&mut Decoder> {
    if let Value::Instance {
        memory_stream: Some(MemoryStream::Decoder(state)),
        ..
    } = ctx.heap.get_mut(receiver)
    {
        return Ok(state);
    }
    Err(Error::new(
        "ValueError",
        "IncrementalNewlineDecoder.__init__() not called",
    ))
}

fn type_error(message: impl Into<String>) -> Error {
    Error::new("TypeError", message)
}

/// Bind the positional-or-keyword clinic parameters, preserving None as an argument.
fn parameters<const N: usize>(
    name: &str,
    names: [&str; N],
    required: usize,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<[Option<Id>; N]> {
    let count = args.len() + kwargs.len();
    if count > N {
        return Err(type_error(format!(
            "{name}() takes at most {N} arguments ({count} given)"
        )));
    }
    let mut values = [None; N];
    for (i, &value) in args.iter().enumerate() {
        values[i] = Some(value);
    }
    for (key, value) in kwargs {
        let Some(i) = names.iter().position(|&n| n == key) else {
            return Err(type_error(format!(
                "{name}() got an unexpected keyword argument '{key}'"
            )));
        };
        if values[i].replace(*value).is_some() {
            return Err(type_error(format!(
                "argument for {name}() given by name ('{key}') and position ({})",
                i + 1
            )));
        }
    }
    for (i, key) in names.iter().take(required).enumerate() {
        if values[i].is_none() {
            return Err(type_error(format!(
                "{name}() missing required argument '{key}' (pos {})",
                i + 1
            )));
        }
    }
    Ok(values)
}

fn invoke(ctx: &mut Context, delegate: Id, method: &str, args: Vec<Id>) -> Result<Id> {
    let callable = ctx.lookup_attr(delegate, method)?;
    ctx.call(callable, args, Vec::new())
}

/// The "K" state format converts with __index__ and masks to an unsigned long long.
fn unpack_state(ctx: &mut Context, arg: Id, setting: bool) -> Result<(Id, u64)> {
    let message = if setting {
        "setstate(): illegal state argument"
    } else {
        "illegal decoder state"
    };
    let Some(items) = ctx.tuple_items(arg)? else {
        return Err(type_error(if setting {
            "state argument must be a tuple"
        } else {
            message
        }));
    };
    let [buffer, flag] = items.as_slice() else {
        return Err(type_error(message));
    };
    ctx.heap.keep(*buffer);
    let number = ctx.index_integer(*flag).map_err(|e| {
        if e.kind == "TypeError" {
            type_error(message)
        } else {
            e
        }
    })?;
    let bigint = number.big();
    let magnitude = bigint.magnitude().iter_u64_digits().next().unwrap_or(0);
    let bits = if bigint.sign() == num_bigint::Sign::Minus {
        magnitude.wrapping_neg()
    } else {
        magnitude
    };
    Ok((*buffer, bits))
}

fn packed_state(ctx: &mut Context, buf: Id, bits: u64) -> Id {
    let flag = ctx.alloc(Value::Int(crate::Int::new(bits.into())));
    ctx.alloc(Value::Tuple(vec![buf, flag]))
}

fn decode(ctx: &mut Context, receiver: Id, text: Id, final_: bool) -> Result<Id> {
    let delegate = state(ctx, receiver)?.delegate;
    let output = if delegate == NONE {
        text
    } else {
        invoke(ctx, delegate, "decode", vec![text, boolean(final_)])?
    };
    if ctx.string_storage(output).is_none() {
        return Err(type_error(format!(
            "decoder should return a string result, not '{}'",
            ctx.type_name(output)
        )));
    }
    // A delegate may have reinitialized this decoder. Take its current state after the call.
    let current = *state(ctx, receiver)?;
    let points = ctx.string_codepoints(output)?;
    if points.is_empty() && (!current.pending || !final_) {
        return Ok(output);
    }
    let mut pending = false;
    let mut seen = current.seen;
    let mut changed = current.pending;
    let mut iter = current
        .pending
        .then_some(13)
        .into_iter()
        .chain(points.iter().copied())
        .peekable();
    let mut result = Vec::new();
    result
        .try_reserve_exact(
            points
                .len()
                .checked_add(usize::from(current.pending))
                .ok_or_else(crate::Error::allocation)?,
        )
        .map_err(|_| crate::Error::allocation())?;
    while let Some(c) = iter.next() {
        if c == 13 {
            match iter.peek() {
                None if !final_ => {
                    pending = true;
                    changed = true;
                    break;
                }
                Some(10) => {
                    iter.next();
                    seen |= 4;
                    if !current.translate {
                        result.push(13);
                    }
                    result.push(10);
                }
                _ => {
                    seen |= 1;
                    result.push(if current.translate { 10 } else { 13 });
                }
            }
            changed |= current.translate;
        } else {
            if c == 10 {
                seen |= 2;
            }
            result.push(c);
        }
    }
    let decoder = state(ctx, receiver)?;
    decoder.seen = seen;
    decoder.pending = pending;
    Ok(if changed {
        ctx.text_codepoints(result)
    } else {
        output
    })
}

pub(super) fn call(
    ctx: &mut Context,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "__init__" {
        let [delegate, translate, errors] = parameters(
            "IncrementalNewlineDecoder",
            ["decoder", "translate", "errors"],
            2,
            args,
            kwargs,
        )?;
        let translate = ctx.truth(translate.unwrap())?;
        let state = Decoder {
            delegate: delegate.unwrap(),
            errors: errors.unwrap_or_else(|| ctx.text("strict")),
            translate,
            pending: false,
            seen: 0,
        };
        let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
            unreachable!("validated decoder instance")
        };
        *memory_stream = Some(MemoryStream::Decoder(state));
        return Ok(NONE);
    }
    if name == "decode" {
        let [input, final_] = parameters("decode", ["input", "final"], 1, args, kwargs)?;
        let final_ = if let Some(value) = final_ {
            ctx.truth(value)?
        } else {
            false
        };
        return decode(ctx, receiver, input.unwrap(), final_);
    }
    if !kwargs.is_empty() {
        return Err(type_error(format!(
            "IncrementalNewlineDecoder.{name}() takes no keyword arguments"
        )));
    }
    let expected = usize::from(name == "setstate");
    if args.len() != expected {
        return Err(type_error(if expected == 0 {
            format!(
                "IncrementalNewlineDecoder.{name}() takes no arguments ({} given)",
                args.len()
            )
        } else {
            format!(
                "IncrementalNewlineDecoder.{name}() takes exactly one argument ({} given)",
                args.len()
            )
        }));
    }
    let current = *state(ctx, receiver)?;
    match name {
        "newlines" => Ok(super::newline_history(ctx, current.seen)),
        "getstate" => {
            let (buffer, flag) = if current.delegate == NONE {
                (ctx.alloc(Value::Bytes(Vec::new())), 0)
            } else {
                let pair = invoke(ctx, current.delegate, "getstate", vec![])?;
                unpack_state(ctx, pair, false)?
            };
            let pending = state(ctx, receiver)?.pending;
            Ok(packed_state(ctx, buffer, flag << 1 | u64::from(pending)))
        }
        "setstate" => {
            let (buffer, flag) = unpack_state(ctx, args[0], true)?;
            let decoder = state(ctx, receiver)?;
            decoder.pending = flag & 1 != 0;
            let delegate = decoder.delegate;
            if delegate == NONE {
                return Ok(NONE);
            }
            let value = packed_state(ctx, buffer, flag >> 1);
            invoke(ctx, delegate, "setstate", vec![value])
        }
        "reset" => {
            let decoder = state(ctx, receiver)?;
            decoder.pending = false;
            decoder.seen = 0;
            if current.delegate == NONE {
                Ok(NONE)
            } else {
                invoke(ctx, current.delegate, "reset", vec![])
            }
        }
        _ => unreachable!("registered decoder method"),
    }
}
