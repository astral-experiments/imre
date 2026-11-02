use num_traits::{Signed, ToPrimitive};

use super::*;

fn c_int(ctx: &mut Context, value: Option<Id>, default: i32) -> Result<i32> {
    let Some(id) = value else { return Ok(default) };
    ctx.index_integer(id)?
        .big()
        .to_i32()
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn unsigned(ctx: &mut Context, value: Option<Id>, ty: &str) -> Result<u64> {
    let Some(id) = value else { return Ok(0) };
    let number = ctx.index_integer(id)?;
    let n = number.big();
    if n.is_negative() {
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    n.to_u64().ok_or_else(|| {
        Error::new(
            "OverflowError",
            format!("Python int too large for C unsigned {ty}"),
        )
    })
}

fn byte_parameter(ctx: &Context, value: Option<Id>) -> Result<&[u8]> {
    value
        .map(|id| ctx.bytes_like(id))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Keep each buffer exported across all subsequent user-defined conversions.
fn with_buffers<T>(
    ctx: &mut Context,
    buffers: &[Option<Id>],
    f: impl FnOnce(&mut Context) -> Result<T>,
) -> Result<T> {
    match buffers.split_first() {
        Some((Some(id), tail)) => {
            ctx.bytes_like(*id)?;
            ctx.with_buffer_export(*id, |ctx| with_buffers(ctx, tail, f))
        }
        Some((None, tail)) => with_buffers(ctx, tail, f),
        None => f(ctx),
    }
}

pub(super) fn new(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [
        data,
        size,
        key,
        salt,
        person,
        fanout,
        depth,
        leaf,
        offset,
        node_depth,
        inner,
        last,
        security,
        string,
    ] = builtins::bind_method_arguments(
        name,
        args,
        kwargs,
        [
            "data",
            "digest_size",
            "key",
            "salt",
            "person",
            "fanout",
            "depth",
            "leaf_size",
            "node_offset",
            "node_depth",
            "inner_size",
            "last_node",
            "usedforsecurity",
            "string",
        ],
        1,
    )?;
    let large = name == "blake2b";
    let (label, max, salt_size) = if large {
        ("Blake2b", 64, 16)
    } else {
        ("Blake2s", 32, 8)
    };
    let size = c_int(ctx, size, max)?;
    with_buffers(ctx, &[key, salt, person], |ctx| {
        let fanout = c_int(ctx, fanout, 1)?;
        let depth = c_int(ctx, depth, 1)?;
        let leaf = unsigned(ctx, leaf, "long")?;
        let offset = unsigned(ctx, offset, "long long")?;
        let node_depth = c_int(ctx, node_depth, 0)?;
        let inner = c_int(ctx, inner, 0)?;
        let last = ctx.optional_truth(last, false)?;
        ctx.optional_truth(security, true)?;
        let input = data_argument(data, string)?;
        if !(1..=max).contains(&size) {
            return Err(Error::new(
                "ValueError",
                format!(
                    "digest_size for {label} must be between 1 and {max} bytes, here it is {size}"
                ),
            ));
        }
        let salt = byte_parameter(ctx, salt)?;
        let person = byte_parameter(ctx, person)?;
        for (field, bytes) in [("salt", salt), ("person", person)] {
            if bytes.len() > salt_size {
                return Err(Error::new(
                    "ValueError",
                    format!("maximum {field} length is {salt_size} bytes"),
                ));
            }
        }
        let fanout = u8::try_from(fanout)
            .map_err(|_| Error::new("ValueError", "fanout must be between 0 and 255"))?;
        if !(1..=255).contains(&depth) {
            return Err(Error::new("ValueError", "depth must be between 1 and 255"));
        }
        let leaf = u32::try_from(leaf)
            .map_err(|_| Error::new("OverflowError", "leaf_size is too large"))?;
        if !large && offset >= 1 << 48 {
            return Err(Error::new("OverflowError", "node_offset is too large"));
        }
        let node_depth = u8::try_from(node_depth)
            .map_err(|_| Error::new("ValueError", "node_depth must be between 0 and 255"))?;
        if !(0..=max).contains(&inner) {
            return Err(Error::new(
                "ValueError",
                format!("inner_size must be between 0 and is {max}"),
            ));
        }
        let key = byte_parameter(ctx, key)?;
        if key.len() > max as usize {
            return Err(Error::new(
                "ValueError",
                format!("maximum key length is {max} bytes"),
            ));
        }
        let input = input
            .map(|id| buffer(ctx, id))
            .transpose()?
            .unwrap_or_default();
        macro_rules! initialize {
            ($provider:ident, $variant:ident) => {{
                let mut state = $provider::Params::new()
                    .hash_length(size as usize)
                    .key(key)
                    .salt(salt)
                    .personal(person)
                    .fanout(fanout)
                    .max_depth(depth as u8)
                    .max_leaf_length(leaf)
                    .node_offset(offset)
                    .node_depth(node_depth)
                    .inner_hash_length(inner as usize)
                    .last_node(last)
                    .to_state();
                state.update(input);
                HashState::$variant(state, size as usize)
            }};
        }
        let state = if large {
            initialize!(blake2b_simd, blake2b)
        } else {
            initialize!(blake2s_simd, blake2s)
        };
        Ok(ctx.alloc(Value::Hash(SharedHash::new(state))))
    })
}
