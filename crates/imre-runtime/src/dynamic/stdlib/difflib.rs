//! Accelerate authenticated bundled difflib source bodies without replacing their Python frames.

use super::super::{
    Context, ffi,
    value::{Code, Id, NONE, Result, Value},
};

/// Run the canonical string constructor path after Python method lookup, binding, and frame entry.
///
/// # Safety
/// The compiler passes the live context, entered scope, and the original source entry for fallback.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_difflib_chain_b(
    raw: *mut Context,
    scope: Id,
    fallback: Code,
) -> Id {
    // SAFETY: the compiler enters this exact Python frame before calling its authenticated wrapper.
    let ctx = unsafe { &mut *raw };
    match chain_b(ctx, scope) {
        Ok(Some(value)) => ffi::finish(ctx, Ok(value)),
        Ok(None) => {
            // SAFETY: this is the original linked source body for the same live frame and scope.
            unsafe { fallback(raw, scope) }
        }
        Err(error) => ffi::finish(ctx, Err(error)),
    }
}

/// Recognize an ordinary exact SequenceMatcher instance whose source globals and attributes need no hooks.
fn receiver(ctx: &Context, scope: Id) -> Option<Id> {
    let root = ctx.root_scope(scope);
    let globals = ctx.scope_data(root);
    if globals.module != "difflib" || globals.exec_globals.is_some() {
        return None;
    }
    let receiver = ctx.scope_name(scope, "self")?;
    let Value::Instance { class, .. } = ctx.heap.get(receiver) else {
        return None;
    };
    if ctx.scope_name(root, "SequenceMatcher") != Some(*class)
        || ctx.class_attr(*class, "__setattr__").is_some()
    {
        return None;
    }
    Some(receiver)
}

/// Build the public b2j dictionary and junk/popular sets from flat ASCII character positions.
fn chain_b(ctx: &mut Context, scope: Id) -> Result<Option<Id>> {
    let Some(receiver) = receiver(ctx, scope) else {
        return Ok(None);
    };
    let root = ctx.root_scope(scope);
    let Some(builtins) = ctx.source_builtins else {
        return Ok(None);
    };
    if ["enumerate", "set", "len"].iter().any(|name| {
        ctx.scope_name(root, name).is_some()
            || ctx.dict_named_item(builtins, name) != ctx.builtins.get(*name).copied()
    }) {
        return Ok(None);
    }
    let Some(b) = ctx.plain_instance_field(receiver, "b") else {
        return Ok(None);
    };
    let Some(isjunk) = ctx.plain_instance_field(receiver, "isjunk") else {
        return Ok(None);
    };
    let Some(autojunk) = ctx.plain_instance_field(receiver, "autojunk") else {
        return Ok(None);
    };
    let autojunk = match ctx.heap.get(autojunk) {
        Value::Bool(value) => *value,
        _ => return Ok(None),
    };
    if isjunk != NONE {
        return Ok(None);
    }
    let Value::Instance { class, .. } = ctx.heap.get(receiver) else {
        unreachable!()
    };
    if ["b2j", "bjunk", "bpopular"].iter().any(|name| {
        ctx.raw_instance_field(receiver, name).is_some() || ctx.class_attr(*class, name).is_some()
    }) {
        // Replacing prior public values can run finalizers at each source assignment.
        return Ok(None);
    }

    let mut counts = [0usize; 256];
    let mut offsets = [0usize; 257];
    let mut order = [0u8; 256];
    let mut order_len = 0usize;
    let mut positions = Vec::new();
    let n = {
        let Value::Str(b) = ctx.heap.get(b) else {
            return Ok(None);
        };
        let bytes = b.as_bytes();
        if !bytes.is_ascii() {
            return Ok(None);
        }
        for &byte in bytes {
            if counts[byte as usize] == 0 {
                order[order_len] = byte;
                order_len += 1;
            }
            counts[byte as usize] += 1;
        }
        for i in 0..256 {
            offsets[i + 1] = offsets[i] + counts[i];
        }
        positions
            .try_reserve_exact(bytes.len())
            .map_err(|_| crate::Error::memory_error())?;
        positions.resize(bytes.len(), 0usize);
        let mut next = offsets;
        for (index, &byte) in bytes.iter().enumerate() {
            positions[next[byte as usize]] = index;
            next[byte as usize] += 1;
        }
        bytes.len()
    };

    ctx.heap
        .reserve_temporaries(order_len.saturating_mul(2).saturating_add(4))?;
    let b2j = ctx.dict(Vec::new());
    ctx.heap.keep(b2j);
    ctx.default_set_attr(receiver, "b2j", b2j)?;
    let threshold = if autojunk && n >= 200 {
        n / 100 + 1
    } else {
        usize::MAX
    };
    for &byte in &order[..order_len] {
        if counts[byte as usize] > threshold {
            continue;
        }
        let key = ctx.text(char::from(byte).to_string());
        ctx.heap.keep(key);
        let mut values = Vec::new();
        values
            .try_reserve_exact(counts[byte as usize])
            .map_err(|_| crate::Error::memory_error())?;
        let list = ctx.alloc(Value::List(values));
        ctx.set_dictionary_key(b2j, key, list, None, false)?;
        for &position in &positions[offsets[byte as usize]..offsets[byte as usize + 1]] {
            let index = ctx.int(position as i64);
            let Value::List(values) = ctx.heap.get_mut(list) else {
                unreachable!()
            };
            values.push(index);
        }
    }
    let bjunk = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    ctx.heap.keep(bjunk);
    ctx.default_set_attr(receiver, "bjunk", bjunk)?;
    let popular = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    ctx.heap.keep(popular);
    ctx.default_set_attr(receiver, "bpopular", popular)?;
    for &byte in &order[..order_len] {
        if counts[byte as usize] <= threshold {
            continue;
        }
        let key = ctx.text(char::from(byte).to_string());
        ctx.heap.keep(key);
        super::super::builtins::set_add(ctx, popular, key)?;
    }
    Ok(Some(NONE))
}
