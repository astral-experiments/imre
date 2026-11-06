//! Boxed types.SimpleNamespace representation from CPython's Objects/namespaceobject.c.

use super::{Context, Error, Id, Result, Value, builtins};

pub(super) fn representation(ctx: &mut Context, object: Id) -> Result<Id> {
    let Value::Instance { class, .. } = *ctx.heap.get(object) else {
        unreachable!("namespace repr receiver was checked")
    };
    let name = if class == ctx.classes["types.SimpleNamespace"] {
        "namespace"
    } else {
        ctx.class_name(class)
    };
    if ctx.repr_ancestors.contains(&object) {
        return Ok(ctx.text(format!("{name}(...)")));
    }

    let mut output = name.chars().map(u32::from).collect::<Vec<_>>();
    output.push(u32::from('('));
    ctx.repr_ancestors
        .try_reserve(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let roots = ctx.heap.temporary_mark();
    ctx.heap.keep(object);
    ctx.repr_ancestors.push(object);
    let result = fields(ctx, object, class, &mut output);
    ctx.repr_ancestors.pop();
    ctx.heap.release_temporaries_since(roots);
    if let Err(error) = result {
        for root in error.roots() {
            ctx.heap.keep(root);
        }
        return Err(error);
    }
    output.push(u32::from(')'));
    Ok(ctx.text_codepoints(output))
}

fn fields(ctx: &mut Context, object: Id, class: Id, output: &mut Vec<u32>) -> Result<()> {
    let dictionary = ctx.instance_dictionary(object, class);
    ctx.heap.keep(dictionary);
    // A value's __repr__ may remove or replace a later item, or insert new keys.
    let entries = ctx.stored_entries(dictionary);
    let mut keys = Vec::new();
    keys.try_reserve_exact(entries.len().max(entries.order_len()))
        .map_err(|_| Error::new("MemoryError", ""))?;
    if entries.order_len() == 0 {
        keys.extend(entries.iter().map(|&(key, _)| key));
    } else {
        keys.extend(entries.ordered_keys());
    }
    ctx.heap.reserve_temporaries(keys.len())?;
    for &key in &keys {
        ctx.heap.keep(key);
    }

    let step = ctx.heap.temporary_mark();
    let mut first = true;
    for key in keys {
        if ctx.string_storage(key).is_none() {
            continue;
        }
        let name = ctx.string_codepoints(key)?.into_owned();
        if name.is_empty() {
            continue;
        }
        let (_, position) = ctx.find_dictionary_key(dictionary, key, None)?;
        if let Some(position) = position {
            let value = ctx.stored_entries(dictionary)[position].1;
            ctx.heap.keep(value);
            let rendered = builtins::call(ctx, "repr", &[value], &[])?;
            let text = ctx.string_codepoints(rendered)?;
            output
                .try_reserve(name.len().saturating_add(text.len()).saturating_add(4))
                .map_err(|_| Error::new("MemoryError", ""))?;
            if !first {
                output.extend([u32::from(','), u32::from(' ')]);
            }
            first = false;
            output.extend(name);
            output.push(u32::from('='));
            output.extend_from_slice(&text);
        }
        ctx.heap.release_temporaries_since(step);
        ctx.heap.reserve_slot_or_request_collection();
        ctx.collect_garbage();
    }
    Ok(())
}
