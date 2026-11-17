//! Accelerate authenticated bundled difflib source bodies without replacing their Python frames.

use super::super::{
    Context, ffi,
    value::{Code, Id, NONE, Result, Value},
};

mod matcher;

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
        || ctx.class_attr(*class, "__del__").is_some()
    {
        return None;
    }
    Some(receiver)
}

/// Skipping source statements must not skip a Python observer or change an allocation trace.
fn unobserved_checkpoints(ctx: &Context) -> bool {
    if ctx.heap.allocation_traces.is_tracing()
        || ctx.heap.gc_callback(0).is_some()
        || ctx.heap.has_weak_values
        || ctx.heap.has_unstarted_coroutines()
    {
        return false;
    }
    let count = ctx.heap.tracked_finalizer_count();
    if count == 0 {
        return true;
    }
    if count > 5 {
        return false;
    }
    let Some(system) = ctx.threads.system_module else {
        return false;
    };
    let Value::Module { scope, .. } = *ctx.heap.get(system) else {
        return false;
    };
    let [stdin, stdout, stderr] = ["__stdin__", "__stdout__", "__stderr__"]
        .map(|name| ctx.scope_name(scope, name).unwrap_or(NONE));
    let [input, buffer, raw] =
        super::io::text_descriptor_chain(&ctx.heap, stdin).unwrap_or([NONE; 3]);
    ctx.heap
        .finalizers_within(&[input, buffer, raw, stdout, stderr])
}

fn canonical_builtins(ctx: &Context, root: Id, names: &[&str]) -> bool {
    let Some(builtins) = ctx.source_builtins else {
        return false;
    };
    names.iter().all(|name| {
        ctx.scope_name(root, name).is_none()
            && ctx.dict_named_item(builtins, name) == ctx.builtins.get(*name).copied()
    })
}

/// Build the public b2j dictionary and junk/popular sets from flat ASCII character positions.
fn chain_b(ctx: &mut Context, scope: Id) -> Result<Option<Id>> {
    let Some(receiver) = receiver(ctx, scope) else {
        return Ok(None);
    };
    let Some(isjunk) = ctx.plain_instance_field(receiver, "isjunk") else {
        return Ok(None);
    };
    if isjunk != NONE {
        return Ok(None);
    }
    let Some(b) = ctx.plain_instance_field(receiver, "b") else {
        return Ok(None);
    };
    if !matches!(ctx.heap.get(b), Value::Str(text) if text.is_ascii()) {
        return Ok(None);
    }
    let Some(autojunk) = ctx.plain_instance_field(receiver, "autojunk") else {
        return Ok(None);
    };
    let autojunk = match ctx.heap.get(autojunk) {
        Value::Bool(value) => *value,
        _ => return Ok(None),
    };
    let Value::Instance { class, .. } = ctx.heap.get(receiver) else {
        unreachable!()
    };
    if ["b2j", "bjunk", "bpopular"].iter().any(|name| {
        ctx.raw_instance_field(receiver, name).is_some() || ctx.class_attr(*class, name).is_some()
    }) {
        // Replacing prior public values can run finalizers at each source assignment.
        return Ok(None);
    }
    let root = ctx.root_scope(scope);
    if !canonical_builtins(ctx, root, &["enumerate", "set", "len"]) || !unobserved_checkpoints(ctx)
    {
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
        debug_assert!(bytes.is_ascii());
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

/// Search the current public `b2j`/`bjunk` state from the original bound source method.
///
/// # Safety
/// The compiler supplies the entered source frame and the original linked entry for fallback.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_difflib_find_longest(
    raw: *mut Context,
    scope: Id,
    fallback: Code,
) -> Id {
    // SAFETY: the compiler enters this authenticated source frame before the wrapper runs.
    let ctx = unsafe { &mut *raw };
    match find_longest(ctx, scope) {
        Ok(Some(value)) => ffi::finish(ctx, Ok(value)),
        Ok(None) => {
            // SAFETY: fallback is the original source entry for this same frame and scope.
            unsafe { fallback(raw, scope) }
        }
        Err(error) => ffi::finish(ctx, Err(error)),
    }
}

fn exact_index(ctx: &Context, value: Id) -> Option<usize> {
    let Value::Int(value) = ctx.heap.get(value) else {
        return None;
    };
    usize::try_from(value.to_i64()?).ok()
}

fn bound(ctx: &Context, scope: Id, name: &str, default: Option<usize>) -> Option<usize> {
    let value = ctx.scope_name(scope, name)?;
    if value == NONE {
        default
    } else {
        exact_index(ctx, value)
    }
}

fn single_char(ctx: &Context, value: Id) -> Option<char> {
    let Value::Str(value) = ctx.heap.get(value) else {
        return None;
    };
    let mut chars = value.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

struct AsciiPositions {
    offsets: [usize; 257],
    positions: Vec<usize>,
    junk: [bool; 256],
}

/// Visit only positions the Python loop would reach before its first upper-bound break.
fn bounded_positions(
    ctx: &Context,
    list: Id,
    blo: usize,
    bhi: usize,
    mut visit: impl FnMut(usize),
) -> Option<()> {
    let Value::List(items) = ctx.heap.get(list) else {
        return None;
    };
    for &item in items {
        let index = exact_index(ctx, item)?;
        if index < blo {
            continue;
        }
        if index >= bhi {
            break;
        }
        visit(index);
    }
    Some(())
}

fn ascii_positions(
    ctx: &Context,
    b2j: Id,
    bjunk: Id,
    active: &[bool; 256],
    blo: usize,
    bhi: usize,
) -> Result<Option<AsciiPositions>> {
    let Value::Dict(entries) = ctx.heap.get(b2j) else {
        return Ok(None);
    };
    let Value::Set {
        entries: junk_entries,
        frozen: false,
    } = ctx.heap.get(bjunk)
    else {
        return Ok(None);
    };
    let mut counts = [0usize; 256];
    let mut lists = [NONE; 256];
    for &(key, value) in entries.iter() {
        let Some(key) = single_char(ctx, key) else {
            return Ok(None);
        };
        if !key.is_ascii() || !active[key as usize] {
            continue;
        }
        let mut count = 0;
        if bounded_positions(ctx, value, blo, bhi, |_| count += 1).is_none() {
            return Ok(None);
        }
        counts[key as usize] = count;
        lists[key as usize] = value;
    }
    let mut offsets = [0usize; 257];
    for i in 0..256 {
        offsets[i + 1] = offsets[i]
            .checked_add(counts[i])
            .ok_or_else(crate::Error::memory_error)?;
    }
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(offsets[256])
        .map_err(|_| crate::Error::memory_error())?;
    for list in lists {
        if list == NONE {
            continue;
        }
        bounded_positions(ctx, list, blo, bhi, |index| positions.push(index))
            .expect("validated positions do not change during native search");
    }
    let mut junk = [false; 256];
    for &(key, _) in junk_entries.iter() {
        let Some(key) = single_char(ctx, key) else {
            return Ok(None);
        };
        if key.is_ascii() {
            junk[key as usize] = true;
        }
    }
    Ok(Some(AsciiPositions {
        offsets,
        positions,
        junk,
    }))
}

struct UnicodePositions {
    keys: Vec<char>,
    offsets: Vec<usize>,
    positions: Vec<usize>,
    junk: Vec<char>,
}

fn unicode_positions(
    ctx: &Context,
    b2j: Id,
    bjunk: Id,
    active: &[char],
    blo: usize,
    bhi: usize,
) -> Result<Option<UnicodePositions>> {
    let Value::Dict(entries) = ctx.heap.get(b2j) else {
        return Ok(None);
    };
    let Value::Set {
        entries: junk_entries,
        frozen: false,
    } = ctx.heap.get(bjunk)
    else {
        return Ok(None);
    };
    let mut groups = Vec::new();
    groups
        .try_reserve_exact(active.len().min(entries.len()))
        .map_err(|_| crate::Error::memory_error())?;
    for &(key, value) in entries.iter() {
        let Some(key) = single_char(ctx, key) else {
            return Ok(None);
        };
        if active.binary_search(&key).is_ok() {
            groups.push((key, value));
        }
    }
    groups.sort_unstable_by_key(|&(key, _)| key);
    let mut keys = Vec::new();
    let mut offsets = Vec::new();
    let mut positions = Vec::new();
    keys.try_reserve_exact(groups.len())
        .map_err(|_| crate::Error::memory_error())?;
    offsets
        .try_reserve_exact(groups.len().saturating_add(1))
        .map_err(|_| crate::Error::memory_error())?;
    offsets.push(0);
    for (key, value) in groups {
        keys.push(key);
        let mut count = 0;
        if bounded_positions(ctx, value, blo, bhi, |_| count += 1).is_none() {
            return Ok(None);
        }
        positions
            .try_reserve(count)
            .map_err(|_| crate::Error::memory_error())?;
        bounded_positions(ctx, value, blo, bhi, |index| positions.push(index))
            .expect("validated positions do not change during native search");
        offsets.push(positions.len());
    }
    let mut junk = Vec::new();
    junk.try_reserve_exact(junk_entries.len())
        .map_err(|_| crate::Error::memory_error())?;
    for &(key, _) in junk_entries.iter() {
        let Some(key) = single_char(ctx, key) else {
            return Ok(None);
        };
        junk.push(key);
    }
    junk.sort_unstable();
    Ok(Some(UnicodePositions {
        keys,
        offsets,
        positions,
        junk,
    }))
}

/// Permit only a named-tuple constructor whose complete call path cannot enter Python.
/// A custom constructor could inspect this source frame's locals and line number.
fn plain_match_constructor(ctx: &Context, root: Id) -> Option<Id> {
    let constructor = ctx.scope_name(root, "Match")?;
    let Value::Class {
        metaclass: None,
        ancestors,
        namedtuple_fields: Some(fields),
        ..
    } = ctx.heap.get(constructor)
    else {
        return None;
    };
    if fields.as_ref() != ["a", "b", "size"]
        || ancestors.as_slice() != [*ctx.builtins.get("tuple")?, *ctx.builtins.get("object")?]
        || ctx.own_class_attr(constructor, "__init__").is_some()
    {
        return None;
    }
    let new = ctx.own_class_attr(constructor, "__new__")?;
    let Value::StaticMethod { callable, .. } = ctx.heap.get(new) else {
        return None;
    };
    let Value::Function { code, scope, .. } = ctx.heap.get(*callable) else {
        return None;
    };
    let code = ctx.code_object(*code);
    let entry = code.native_entry()?;
    if !std::ptr::fn_addr_eq(entry, ffi::namedtuple_constructor as Code) {
        return None;
    }
    let Value::Tuple(varnames) = ctx.heap.get(code.varnames) else {
        return None;
    };
    if varnames.len() != 4
        || varnames
            .iter()
            .zip(["_cls", "a", "b", "size"])
            .any(|(&name, expected)| ctx.str_value(name).ok() != Some(expected))
    {
        return None;
    }
    let globals = ctx.scope_data(*scope).exec_globals.as_ref()?.dict;
    let Value::Dict(entries) = ctx.heap.get(globals) else {
        return None;
    };
    if entries
        .iter()
        .any(|&(key, _)| !matches!(ctx.heap.get(key), Value::Str(_)))
        || ctx.dict_named_item(globals, "_tuple_new") != ctx.builtins.get("tuple.__new__").copied()
    {
        return None;
    }
    Some(constructor)
}

fn find_longest(ctx: &mut Context, scope: Id) -> Result<Option<Id>> {
    let Some(receiver) = receiver(ctx, scope) else {
        return Ok(None);
    };
    let root = ctx.root_scope(scope);
    if !canonical_builtins(ctx, root, &["len", "range"]) {
        return Ok(None);
    }
    let Some(constructor) = plain_match_constructor(ctx, root) else {
        return Ok(None);
    };
    let Some(a) = ctx.plain_instance_field(receiver, "a") else {
        return Ok(None);
    };
    let Some(b) = ctx.plain_instance_field(receiver, "b") else {
        return Ok(None);
    };
    let Some(b2j) = ctx.plain_instance_field(receiver, "b2j") else {
        return Ok(None);
    };
    let Some(bjunk) = ctx.plain_instance_field(receiver, "bjunk") else {
        return Ok(None);
    };
    let (a_len, b_len, ascii) = match (ctx.heap.get(a), ctx.heap.get(b)) {
        (Value::Str(a), Value::Str(b)) => {
            let ascii = a.is_ascii() && b.is_ascii();
            if !ascii && b.len() > a.len().saturating_mul(64).max(4096) {
                // The source loop touches only left-hand anchors. Avoid walking,
                // counting, then copying a severely asymmetric Unicode right side.
                return Ok(None);
            }
            (
                if ascii { a.len() } else { a.chars().count() },
                if ascii { b.len() } else { b.chars().count() },
                ascii,
            )
        }
        _ => return Ok(None),
    };
    let Some(alo) = bound(ctx, scope, "alo", None) else {
        return Ok(None);
    };
    let Some(ahi) = bound(ctx, scope, "ahi", Some(a_len)) else {
        return Ok(None);
    };
    let Some(blo) = bound(ctx, scope, "blo", None) else {
        return Ok(None);
    };
    let Some(bhi) = bound(ctx, scope, "bhi", Some(b_len)) else {
        return Ok(None);
    };
    if alo > ahi || ahi > a_len || blo > bhi || bhi > b_len {
        return Ok(None);
    }
    if !unobserved_checkpoints(ctx) {
        return Ok(None);
    }
    let best = if ascii {
        let (Value::Str(a), Value::Str(b)) = (ctx.heap.get(a), ctx.heap.get(b)) else {
            unreachable!()
        };
        let mut active = [false; 256];
        for &byte in &a.as_bytes()[alo..ahi] {
            active[byte as usize] = true;
        }
        let Some(index) = ascii_positions(ctx, b2j, bjunk, &active, blo, bhi)? else {
            return Ok(None);
        };
        matcher::longest_ascii(
            a.as_bytes(),
            b.as_bytes(),
            (alo, ahi, blo, bhi),
            &index.offsets,
            &index.positions,
            &index.junk,
        )?
    } else {
        let (Value::Str(a), Value::Str(b)) = (ctx.heap.get(a), ctx.heap.get(b)) else {
            unreachable!()
        };
        let mut left = Vec::new();
        let mut right = Vec::new();
        left.try_reserve_exact(a_len)
            .map_err(|_| crate::Error::memory_error())?;
        right
            .try_reserve_exact(b_len)
            .map_err(|_| crate::Error::memory_error())?;
        left.extend(a.chars());
        right.extend(b.chars());
        let mut active = Vec::new();
        active
            .try_reserve_exact(ahi - alo)
            .map_err(|_| crate::Error::memory_error())?;
        active.extend_from_slice(&left[alo..ahi]);
        active.sort_unstable();
        active.dedup();
        let Some(index) = unicode_positions(ctx, b2j, bjunk, &active, blo, bhi)? else {
            return Ok(None);
        };
        matcher::longest_unicode(
            &left,
            &right,
            (alo, ahi, blo, bhi),
            &index.keys,
            &index.offsets,
            &index.positions,
            &index.junk,
        )?
    };
    ctx.heap.reserve_temporaries(3)?;
    let left = ctx.int(best.0 as i64);
    ctx.heap.keep(left);
    let right = ctx.int(best.1 as i64);
    ctx.heap.keep(right);
    let size = ctx.int(best.2 as i64);
    ctx.heap.keep(size);
    ctx.call(constructor, vec![left, right, size], Vec::new())
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::{Context, NONE, Value, ffi, plain_match_constructor};

    #[test]
    fn match_constructor_requires_a_callback_free_namedtuple_path() {
        let mut ctx: Context = ffi::new_context(
            serde_json::json!({"paths": [], "python_version": "3.14"}),
            ["imre"],
        );
        let root = ctx.scope("difflib", None);
        let scope = ctx.scope("difflib", None);
        let tuple = ctx.builtin("tuple");
        let class = ctx
            .allocate_class("Match".into(), scope, vec![tuple])
            .unwrap();
        let Value::Class {
            namedtuple_fields, ..
        } = ctx.heap.get_mut(class)
        else {
            unreachable!()
        };
        *namedtuple_fields = Some(vec!["a".into(), "b".into(), "size".into()].into());
        let name = ctx.text("<lambda>");
        let parameters =
            ctx.text(r#"{"positional":["_cls","a","b","size"],"scope_complete":true}"#);
        let filename = ctx.text("<string>");
        let code = ctx.prelinked_code(
            super::super::super::value::Body::Native(ffi::namedtuple_constructor, NONE),
            root,
            name,
            parameters,
            (name, NONE),
            (filename, 1),
        );
        let varnames = ["_cls", "a", "b", "size"].map(|name| ctx.text(name));
        let varnames = ctx.alloc(Value::Tuple(varnames.to_vec()));
        let Value::Code(metadata) = ctx.heap.get_mut(code) else {
            unreachable!()
        };
        metadata.varnames = varnames;
        let allocate = ctx.text("_tuple_new");
        let tuple_new = ctx.builtin("tuple.__new__");
        let globals = ctx.dict(vec![(allocate, tuple_new)]);
        let function = ctx.construct_function(&[code, globals], &[]).unwrap();
        let descriptor =
            super::super::super::method_decorator::static_method(&mut ctx, function).unwrap();
        ctx.bind(scope, "__new__", descriptor);
        ctx.bind(root, "Match", class);
        assert_eq!(plain_match_constructor(&ctx, root), Some(class));

        let replacement = ctx.builtin("len");
        ctx.set_attr(class, "__new__", replacement).unwrap();
        assert_eq!(plain_match_constructor(&ctx, root), None);
    }
}
