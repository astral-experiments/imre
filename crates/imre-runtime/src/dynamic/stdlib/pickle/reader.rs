//! Read a callback-free, framed pickle directly from a byte cursor.

use super::super::super::{
    Context, ffi,
    value::{self, Code, Id, NONE, Result, Value},
};

const PROTO: u8 = 0x80;
const FRAME: u8 = 0x95;
const EMPTY_LIST: u8 = b']';
const MEMOIZE: u8 = 0x94;
const MARK: u8 = b'(';
const SHORT_BINUNICODE: u8 = 0x8c;
const BINGET: u8 = b'h';
const LONG_BINGET: u8 = b'j';
const APPENDS: u8 = b'e';
const STOP: u8 = b'.';

/// Enter through the original bundled function after normal signature binding.
///
/// # Safety
/// The compiler supplies the entered source frame and its original fallback body.
#[unsafe(no_mangle)]
pub(in crate::dynamic) unsafe extern "C" fn imre_dyn_pickle_loads(
    raw: *mut Context,
    scope: Id,
    fallback: Code,
) -> Id {
    // SAFETY: the authenticated compiler wrapper entered this source frame first.
    let ctx = unsafe { &mut *raw };
    match attempt(ctx, scope) {
        Ok(Some(value)) => ffi::finish(ctx, Ok(value)),
        Ok(None) => {
            // SAFETY: this is the original body for the same live frame and scope.
            unsafe { fallback(raw, scope) }
        }
        Err(error) => ffi::finish(ctx, Err(error)),
    }
}

/// One validated stack push. The low 32 bits are either an input-byte offset
/// or a memo index, bits 32..40 hold a short string length, and bits 40..63
/// count subsequent MEMOIZE operations. The top bit distinguishes memo gets.
#[derive(Clone, Copy)]
struct Item(u64);

impl Item {
    const GET: u64 = 1 << 63;
    const MEMO: u64 = 1 << 40;
    const MAX_MEMO: u64 = (1 << 23) - 1;

    fn text(offset: usize, length: u8) -> Option<Self> {
        Some(Self(
            u64::from(u32::try_from(offset).ok()?) | (u64::from(length) << 32),
        ))
    }

    fn get(index: usize) -> Option<Self> {
        Some(Self(Self::GET | u64::from(u32::try_from(index).ok()?)))
    }

    fn memoize(&mut self) -> Option<()> {
        if self.memo_count() as u64 == Self::MAX_MEMO {
            return None;
        }
        self.0 += Self::MEMO;
        Some(())
    }

    fn memo_count(self) -> usize {
        ((self.0 >> 40) & Self::MAX_MEMO) as usize
    }

    fn index(self) -> usize {
        self.0 as u32 as usize
    }
    fn length(self) -> usize {
        ((self.0 >> 32) & 0xff) as usize
    }
    fn is_get(self) -> bool {
        self.0 & Self::GET != 0
    }
}

struct Layout {
    items: Vec<Item>,
    memo: usize,
    used: u16,
    has_get: bool,
}

const HANDLERS: &[(u8, u32)] = &[
    (PROTO, value::PICKLE_PROTO_BODY),
    (FRAME, value::PICKLE_FRAME_BODY),
    (EMPTY_LIST, value::PICKLE_EMPTY_LIST_BODY),
    (MEMOIZE, value::PICKLE_MEMOIZE_BODY),
    (MARK, value::PICKLE_MARK_BODY),
    (SHORT_BINUNICODE, value::PICKLE_SHORT_UNICODE_BODY),
    (APPENDS, value::PICKLE_APPENDS_BODY),
    (STOP, value::PICKLE_STOP_BODY),
    (BINGET, value::PICKLE_BINGET_BODY),
    (LONG_BINGET, value::PICKLE_LONG_BINGET_BODY),
];

fn flag(op: u8) -> Option<u16> {
    Some(match op {
        PROTO => 1 << 0,
        FRAME => 1 << 1,
        EMPTY_LIST => 1 << 2,
        MEMOIZE => 1 << 3,
        MARK => 1 << 4,
        SHORT_BINUNICODE => 1 << 5,
        APPENDS => 1 << 6,
        STOP => 1 << 7,
        BINGET => 1 << 8,
        LONG_BINGET => 1 << 9,
        _ => return None,
    })
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        if end > self.end {
            return None;
        }
        let value = self.data.get(self.at..end)?;
        self.at = end;
        Some(value)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn get_index(&mut self, op: u8) -> Option<usize> {
        match op {
            BINGET => Some(usize::from(self.byte()?)),
            LONG_BINGET => {
                let bytes: [u8; 4] = self.take(4)?.try_into().ok()?;
                usize::try_from(u32::from_le_bytes(bytes)).ok()
            }
            _ => None,
        }
    }
}

/// Validate the accepted grammar and UTF-8 before publishing Python objects;
/// retain only packed positions into the immutable input, not copies of text.
fn preflight(data: &[u8]) -> Option<Layout> {
    if data.get(..3)? != [PROTO, 4, FRAME] && data.get(..3)? != [PROTO, 5, FRAME] {
        return None;
    }
    let size = u64::from_le_bytes(data.get(3..11)?.try_into().ok()?);
    let end = 11usize.checked_add(usize::try_from(size).ok()?)?;
    if end > data.len() || end > u32::MAX as usize {
        return None;
    }
    let mut cursor = Cursor { data, at: 11, end };
    if cursor.byte()? != EMPTY_LIST || cursor.byte()? != MEMOIZE || cursor.byte()? != MARK {
        return None;
    }
    let mut used = flag(PROTO)? | flag(FRAME)? | flag(EMPTY_LIST)? | flag(MEMOIZE)? | flag(MARK)?;
    let mut items: Vec<Item> = Vec::new();
    items
        .try_reserve((end.saturating_sub(14) / 8).min(1024))
        .ok()?;
    let mut memo = 1usize;
    let mut has_get = false;
    loop {
        let op = cursor.byte()?;
        used |= flag(op)?;
        match op {
            SHORT_BINUNICODE => {
                let length = cursor.byte()?;
                let offset = cursor.at;
                std::str::from_utf8(cursor.take(usize::from(length))?).ok()?;
                items.try_reserve(1).ok()?;
                items.push(Item::text(offset, length)?);
            }
            BINGET | LONG_BINGET => {
                let index = cursor.get_index(op)?;
                if index >= memo {
                    return None;
                }
                items.try_reserve(1).ok()?;
                items.push(Item::get(index)?);
                has_get = true;
            }
            MEMOIZE => {
                items.last_mut()?.memoize()?;
                memo = memo.checked_add(1)?;
            }
            APPENDS => {
                if cursor.byte()? != STOP || cursor.at != end {
                    return None;
                }
                used |= flag(STOP)?;
                return Some(Layout {
                    items,
                    memo,
                    used,
                    has_get,
                });
            }
            _ => return None,
        }
    }
}

fn source_function(ctx: &Context, function: Id, root: Id, tag: u32) -> bool {
    matches!(ctx.heap.get(function), Value::Function { code, scope, .. }
        if *scope == root && ctx.code_object(*code).definition.provider_body() == tag as u8)
}

fn source_class(ctx: &Context, root: Id, class: Id, expected: &str) -> bool {
    let Value::Class {
        metaclass,
        name,
        scope,
        ancestors,
        ..
    } = ctx.heap.get(class)
    else {
        return false;
    };
    name == expected
        && metaclass.is_none()
        && ctx.scope_data(*scope).parent == Some(root)
        && ancestors.as_slice()
            == ctx
                .builtins
                .get("object")
                .map(std::slice::from_ref)
                .unwrap_or(&[])
        && [
            "__new__",
            "__getattr__",
            "__getattribute__",
            "__setattr__",
            "__bool__",
            "__len__",
            "__del__",
        ]
        .iter()
        .all(|name| ctx.own_class_attr(class, name).is_none())
}

fn methods(ctx: &Context, root: Id, class: Id, expected: &[(&str, u32)]) -> bool {
    expected.iter().all(|&(name, tag)| {
        ctx.own_class_attr(class, name)
            .is_some_and(|function| source_function(ctx, function, root, tag))
    })
}

fn plain_fields(ctx: &Context, class: Id, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|name| ctx.own_class_attr(class, name).is_none())
}

fn builtins(ctx: &Context, root: Id) -> bool {
    let Some(table) = ctx.source_builtins else {
        return false;
    };
    [
        "bool",
        "bytes",
        "isinstance",
        "str",
        "len",
        "getattr",
        "type",
        "memoryview",
        "min",
        "hasattr",
    ]
    .iter()
    .all(|name| {
        ctx.scalar_scope_name(root, name) == Some(None)
            && ctx.scalar_dict_name(table, name) == Some(ctx.builtins.get(*name).copied())
    })
}

fn io_method(ctx: &Context, class: Id, name: &str) -> bool {
    let Some(mut member) = ctx.own_class_attr(class, name) else {
        return false;
    };
    if let Value::Method { callable, .. } = ctx.heap.get(member) {
        member = *callable;
    }
    matches!(ctx.heap.get(member), Value::IoMethod { owner, name: current, .. }
        if *owner == class && *current == name)
}

fn environment(ctx: &Context, root: Id, used: u16) -> bool {
    // _append_items evaluates this default even though exact lists always have extend.
    if !builtins(ctx, root) || !matches!(ctx.scalar_scope_name(root, "_NoValue"), Some(Some(_))) {
        return false;
    }
    let Some(Some(loads)) = ctx.scalar_scope_name(root, "loads") else {
        return false;
    };
    if !source_function(ctx, loads, root, value::PICKLE_LOADS_BODY) {
        return false;
    }
    let Some(Some(reader)) = ctx.scalar_scope_name(root, "_loads_reader") else {
        return false;
    };
    if !source_class(ctx, root, reader, "_Unpickler")
        || !plain_fields(
            ctx,
            reader,
            &[
                "_buffers",
                "_file_readline",
                "_file_read",
                "memo",
                "encoding",
                "errors",
                "proto",
                "fix_imports",
                "_unframer",
                "read",
                "readline",
                "metastack",
                "stack",
                "append",
            ],
        )
        || !methods(
            ctx,
            root,
            reader,
            &[
                ("__init__", value::PICKLE_READER_INIT_BODY),
                ("load", value::PICKLE_READER_LOAD_BODY),
                ("_require_stack", value::PICKLE_REQUIRE_STACK_BODY),
                ("pop_mark", value::PICKLE_POP_MARK_BODY),
                ("_append_items", value::PICKLE_APPEND_ITEMS_BODY),
            ],
        )
    {
        return false;
    }
    let Some(dispatch) = ctx.own_class_attr(reader, "dispatch") else {
        return false;
    };
    let Value::Dict(entries) = ctx.heap.get(dispatch) else {
        return false;
    };
    for (index, &(op, tag)) in HANDLERS.iter().enumerate() {
        if used & (1 << index) != 0 {
            let Some(Some(handler)) = entries.lookup_ordinal(ctx, char::from(op)).continue_value()
            else {
                return false;
            };
            if !source_function(ctx, handler, root, tag) {
                return false;
            }
        }
    }
    let Some(Some(framer)) = ctx.scalar_scope_name(root, "_Unframer") else {
        return false;
    };
    if !source_class(ctx, root, framer, "_Unframer")
        || !plain_fields(
            ctx,
            framer,
            &["file_read", "file_readline", "current_frame"],
        )
        || !methods(
            ctx,
            root,
            framer,
            &[
                ("__init__", value::PICKLE_UNFRAMER_INIT_BODY),
                ("read", value::PICKLE_UNFRAMER_READ_BODY),
                ("read_exact", value::PICKLE_UNFRAMER_EXACT_BODY),
                ("readline", value::PICKLE_UNFRAMER_LINE_BODY),
                ("load_frame", value::PICKLE_UNFRAMER_FRAME_BODY),
            ],
        )
    {
        return false;
    }
    let Some(Some(frame)) = ctx.scalar_scope_name(root, "_BytesFrame") else {
        return false;
    };
    if !source_class(ctx, root, frame, "_BytesFrame")
        || !plain_fields(ctx, frame, &["view", "pos"])
        || !methods(
            ctx,
            root,
            frame,
            &[
                ("__init__", value::PICKLE_BYTES_FRAME_INIT_BODY),
                ("read", value::PICKLE_BYTES_FRAME_READ_BODY),
            ],
        )
    {
        return false;
    }
    let Some(Some(io)) = ctx.scalar_scope_name(root, "io") else {
        return false;
    };
    let Value::Module {
        scope: io_scope,
        constructor: None,
        ..
    } = ctx.heap.get(io)
    else {
        return false;
    };
    let Some(&bytes_io) = ctx.builtins.get("_io.BytesIO") else {
        return false;
    };
    if ctx.scalar_scope_name(*io_scope, "BytesIO") != Some(Some(bytes_io))
        || !["__new__", "__init__", "read", "readline"]
            .iter()
            .all(|name| io_method(ctx, bytes_io, name))
    {
        return false;
    }
    let Some(Some(unpack)) = ctx.scalar_scope_name(root, "unpack") else {
        return false;
    };
    if !matches!(ctx.heap.get(unpack), Value::Native(name) if name == "struct.unpack") {
        return false;
    }
    let Some(Some(protocol)) = ctx.scalar_scope_name(root, "HIGHEST_PROTOCOL") else {
        return false;
    };
    if !matches!(ctx.heap.get(protocol), Value::Int(n) if n.as_small() == Some(5)) {
        return false;
    }
    let Some(Some(system)) = ctx.scalar_scope_name(root, "sys") else {
        return false;
    };
    if Some(system) != ctx.threads.system_module {
        return false;
    }
    let Value::Module {
        scope: system_scope,
        constructor: None,
        ..
    } = ctx.heap.get(system)
    else {
        return false;
    };
    let Some(Some(maxsize)) = ctx.scalar_scope_name(*system_scope, "maxsize") else {
        return false;
    };
    matches!(ctx.heap.get(maxsize), Value::Int(n) if n.to_i64() == Some(isize::MAX as i64))
}

fn unobserved(ctx: &Context) -> bool {
    #[cfg(unix)]
    {
        if crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        let default = ctx.builtins.get("_signal.SIG_DFL").copied();
        let ignore = ctx.builtins.get("_signal.SIG_IGN").copied();
        let interrupt = ctx.builtins.get("_signal.default_int_handler").copied();
        if !ctx
            .signal_handlers
            .iter()
            .enumerate()
            .all(|(number, &handler)| {
                handler == NONE
                    || Some(handler) == default
                    || Some(handler) == ignore
                    || (number == libc::SIGINT as usize && Some(handler) == interrupt)
            })
        {
            return false;
        }
    }
    if ctx.heap.allocation_traces.is_tracing()
        || ctx.heap.gc_callback(0).is_some()
        || ctx.heap.has_weak_values
        || ctx.heap.has_unstarted_coroutines()
        || ctx.heap.full_collection_active()
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
        super::super::io::text_descriptor_chain(&ctx.heap, stdin).unwrap_or([NONE; 3]);
    ctx.heap
        .finalizers_within(&[input, buffer, raw, stdout, stderr])
}

fn options(ctx: &Context, scope: Id) -> bool {
    let Some(fix_imports) = ctx.scope_name(scope, "fix_imports") else {
        return false;
    };
    if !matches!(ctx.heap.get(fix_imports), Value::Bool(_))
        || ctx.scope_name(scope, "buffers") != Some(NONE)
    {
        return false;
    }
    ["encoding", "errors"].iter().all(|name| {
        ctx.scope_name(scope, name)
            .is_some_and(|id| matches!(ctx.heap.get(id), Value::Str(text) if !text.contains('\0')))
    })
}

/// Build from validated input offsets. A memo vector is needed only if the
/// pickle actually retrieves memoized objects.
fn decode(ctx: &mut Context, data: Id, layout: Layout) -> Result<Id> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(layout.items.len())
        .map_err(|_| crate::Error::memory_error())?;
    let mut memo = Vec::new();
    if layout.has_get {
        memo.try_reserve_exact(layout.memo)
            .map_err(|_| crate::Error::memory_error())?;
        memo.push(NONE); // placeholder for the result list itself
    }
    ctx.heap
        .reserve_temporaries(layout.items.len().saturating_add(1))?;
    let mark = ctx.heap.temporary_mark();
    let Value::Bytes(bytes) = ctx.heap.get(data) else {
        unreachable!()
    };
    let (pointer, length) = (bytes.as_ptr(), bytes.len());
    // SAFETY: the source frame roots this exact immutable bytes object; its Vec
    // backing allocation does not move when heap slots grow. No callback,
    // collection, signal checkpoint, or mutation runs while decoding.
    let input = unsafe { std::slice::from_raw_parts(pointer, length) };
    for item in layout.items {
        let top = if item.is_get() {
            memo[item.index()]
        } else {
            let start = item.index();
            // SAFETY: preflight checked both the range and UTF-8, and the rooted
            // immutable input has not changed. CompactString copies into its slot.
            let text =
                unsafe { std::str::from_utf8_unchecked(&input[start..start + item.length()]) };
            ctx.text(text)
        };
        values.push(top);
        if layout.has_get {
            memo.extend(std::iter::repeat_n(top, item.memo_count()));
        }
    }
    let result = ctx.alloc(Value::List(values));
    if layout.has_get
        && let Value::List(values) = ctx.heap.get_mut(result)
    {
        for item in values {
            if *item == NONE {
                *item = result;
            }
        }
    }
    ctx.heap.keep_call_result(mark, result);
    Ok(result)
}

fn attempt(ctx: &mut Context, scope: Id) -> Result<Option<Id>> {
    let root = ctx.root_scope(scope);
    let globals = ctx.scope_data(root);
    if globals.module != "pickle" || globals.exec_globals.is_some() || !options(ctx, scope) {
        return Ok(None);
    }
    let Some(data) = ctx.scope_name(scope, "data") else {
        return Ok(None);
    };
    let Value::Bytes(bytes) = ctx.heap.get(data) else {
        return Ok(None);
    };
    // Reject observers before scanning or allocating a descriptor vector. No
    // Python callback can alter the environment while preflight runs.
    if !unobserved(ctx) {
        return Ok(None);
    }
    let Some(layout) = preflight(bytes) else {
        return Ok(None);
    };
    #[cfg(unix)]
    if crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(None);
    }
    if !environment(ctx, root, layout.used) {
        return Ok(None);
    }
    decode(ctx, data, layout).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_accepts_dense_memo_and_rejects_sparse_or_incomplete_streams() {
        let good = [
            PROTO,
            5,
            FRAME,
            11,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            EMPTY_LIST,
            MEMOIZE,
            MARK,
            SHORT_BINUNICODE,
            1,
            b'x',
            MEMOIZE,
            BINGET,
            1,
            APPENDS,
            STOP,
        ];
        let layout = preflight(&good).unwrap();
        assert_eq!((layout.items.len(), layout.memo), (2, 2));
        assert!(layout.has_get);
        assert!(layout.items[1].is_get());
        assert!(preflight(&good[..good.len() - 1]).is_none());
        let mut sparse = good;
        sparse[14] = b'q'; // BINPUT is intentionally unsupported.
        assert!(preflight(&sparse).is_none());
    }
}
