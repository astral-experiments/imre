//! A decoder that carries a pending CR and newline history across delegate calls.

use super::{Context, Error, Id, MemoryStream, NONE, Result, Value};
use crate::dynamic::builtins::boolean;

pub(in crate::dynamic) const CLASS: &str = "_io.IncrementalNewlineDecoder";

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
    for (i, &key) in names.iter().enumerate().skip(args.len()) {
        if let Some((_, value)) = kwargs.iter().find(|(name, _)| name == key) {
            values[i] = Some(*value);
        } else if i < required {
            return Err(type_error(format!(
                "{name}() missing required argument '{key}' (pos {})",
                i + 1
            )));
        }
    }
    for (key, _) in kwargs {
        match names.iter().position(|&param| param == key) {
            Some(i) if i < args.len() => {
                return Err(type_error(format!(
                    "argument for {name}() given by name ('{key}') and position ({})",
                    i + 1
                )));
            }
            None => {
                return Err(type_error(format!(
                    "{name}() got an unexpected keyword argument '{key}'"
                )));
            }
            _ => {}
        }
    }
    Ok(values)
}

/// Attribute lookup can reinitialize the wrapper and release its delegate; only the
/// resulting callable owns the receiver, if applicable, when its Python body runs.
fn invoke(ctx: &mut Context, delegate: Id, name: &str, args: Vec<Id>) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(delegate);
    let callable = ctx.lookup_attr(delegate, name);
    ctx.heap.release_temporaries_since(mark);
    ctx.heap.weak_edge_removed(delegate);
    match callable {
        Ok(callable) => {
            ctx.heap.keep(callable);
            ctx.call(callable, args, Vec::new())
        }
        Err(error) => {
            for value in error.roots() {
                ctx.heap.keep(value);
            }
            Err(error)
        }
    }
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
    ctx.heap.keep(arg);
    let (_, number) = ctx
        .index_result(*flag)?
        .ok_or_else(|| type_error(message))?;
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
    let Some(storage) = ctx.string_storage(output) else {
        return Err(type_error(format!(
            "decoder should return a string result, not '{:.200}'",
            ctx.type_name(output)
        )));
    };
    // A delegate may have reinitialized this decoder. Read the live state after the call.
    let current = *state(ctx, receiver)?;
    // CPython's LF-only fast path can return the actual str subclass. Its byte
    // scan also sees 0x0d embedded in a wide code point such as U+0D00.
    if !current.pending && (!current.translate || current.seen & (1 | 4) == 0) {
        let cr_byte = |p: u32| p.to_ne_bytes().contains(&13);
        let only_lf = match ctx.heap.get(storage) {
            Value::Str(s)
                if !s.contains('\r')
                    && (!current.translate
                        || s.is_ascii()
                        || !s.chars().any(|c| cr_byte(c as u32))) =>
            {
                Some(s.contains('\n'))
            }
            Value::SurrogateStr(s)
                if !s.contains(&13) && (!current.translate || !s.iter().copied().any(cr_byte)) =>
            {
                Some(s.contains(&10))
            }
            _ => None,
        };
        if let Some(has_lf) = only_lf {
            if has_lf {
                state(ctx, receiver)?.seen |= 2;
            }
            return Ok(output);
        }
    }
    let points = ctx.string_codepoints(output)?;
    if points.is_empty() && (!current.pending || !final_) {
        return Ok(output);
    }
    let mut pending = false;
    let mut seen = current.seen;
    let mut changed = current.pending || current.translate;
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
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, args)) = args.split_first() else {
        return Err(type_error(if name == "__init__" {
            format!("descriptor '__init__' of '{CLASS}' object needs an argument")
        } else {
            format!("unbound method IncrementalNewlineDecoder.{name}() needs an argument")
        }));
    };
    if !ctx
        .heap
        .get(receiver)
        .instance_class()
        .is_some_and(|class| ctx.direct_subclass(class, owner))
    {
        let actual = ctx.type_name(receiver);
        return Err(type_error(if name == "__init__" {
            format!("descriptor '__init__' requires a '{CLASS}' object but received a '{actual}'")
        } else {
            format!(
                "descriptor '{name}' for '{CLASS}' objects doesn't apply to a '{actual}' object"
            )
        }));
    }
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
        if let Some(MemoryStream::Decoder(previous)) =
            memory_stream.replace(MemoryStream::Decoder(state))
        {
            ctx.heap.weak_edge_removed(previous.delegate);
            ctx.heap.weak_edge_removed(previous.errors);
        }
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
