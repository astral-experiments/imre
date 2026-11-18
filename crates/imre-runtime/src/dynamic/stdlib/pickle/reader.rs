//! Read a callback-free, framed pickle directly from a byte cursor.

use super::super::super::{
    Context, ffi,
    value::{Code, Id, NONE, Result, Value},
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
/// or a memo index, bits 32..40 hold a short string length, and bits 40..62
/// count subsequent MEMOIZE operations, bit 62 records ASCII, and bit 63 distinguishes memo gets.
#[derive(Clone, Copy)]
struct Item(u64);

impl Item {
    const GET: u64 = 1 << 63;
    const ASCII: u64 = 1 << 62;
    const MEMO: u64 = 1 << 40;
    const MAX_MEMO: u64 = (1 << 22) - 1;

    fn text(offset: usize, length: u8, ascii: bool) -> Option<Self> {
        Some(Self(
            u64::from(u32::try_from(offset).ok()?)
                | (u64::from(length) << 32)
                | if ascii { Self::ASCII } else { 0 },
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
    fn is_ascii(self) -> bool {
        self.0 & Self::ASCII != 0
    }
    fn is_get(self) -> bool {
        self.0 & Self::GET != 0
    }
}

struct Layout {
    items: Vec<Item>,
    memo: usize,
    has_get: bool,
    texts: usize,
    dense_prefix: usize,
    prefix_extra_memo: usize,
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
    // The frame itself encodes offsets and lengths for a run of memoized
    // ASCII text. Keep that prefix in place even if a later item needs the
    // general grammar; only the remaining pushes need packed descriptors.
    let mut dense_prefix = 0usize;
    while cursor.at < end && data[cursor.at] == SHORT_BINUNICODE {
        let Some(&length) = data.get(cursor.at.checked_add(1)?) else {
            break;
        };
        let start = cursor.at.checked_add(2)?;
        let text_end = start.checked_add(usize::from(length))?;
        if text_end >= end || data[text_end] != MEMOIZE || !data[start..text_end].is_ascii() {
            break;
        }
        cursor.at = text_end + 1;
        dense_prefix = dense_prefix.checked_add(1)?;
    }
    let mut items: Vec<Item> = Vec::new();
    items
        .try_reserve((end.saturating_sub(cursor.at) / 8).min(1024))
        .ok()?;
    let mut memo = dense_prefix.checked_add(1)?;
    let mut has_get = false;
    let mut texts = dense_prefix;
    let mut prefix_extra_memo = 0usize;
    loop {
        let op = cursor.byte()?;
        match op {
            SHORT_BINUNICODE => {
                let length = cursor.byte()?;
                let offset = cursor.at;
                let bytes = cursor.take(usize::from(length))?;
                let ascii = bytes.is_ascii();
                if !ascii {
                    std::str::from_utf8(bytes).ok()?;
                }
                items.try_reserve(1).ok()?;
                items.push(Item::text(offset, length, ascii)?);
                texts = texts.checked_add(1)?;
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
                if let Some(item) = items.last_mut() {
                    item.memoize()?;
                } else if dense_prefix != 0 && (prefix_extra_memo as u64) < Item::MAX_MEMO - 1 {
                    prefix_extra_memo += 1;
                } else {
                    return None;
                }
                memo = memo.checked_add(1)?;
            }
            APPENDS => {
                if cursor.byte()? != STOP || cursor.at != end {
                    return None;
                }
                return Some(Layout {
                    items,
                    memo,
                    has_get,
                    texts,
                    dense_prefix,
                    prefix_extra_memo,
                });
            }
            _ => return None,
        }
    }
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
    valid_options(ctx, |name| ctx.scope_name(scope, name))
}

fn valid_options(ctx: &Context, option: impl Fn(&str) -> Option<Id>) -> bool {
    option("fix_imports").is_some_and(|id| matches!(ctx.heap.get(id), Value::Bool(_)))
        && option("buffers") == Some(NONE)
        && ["encoding", "errors"].iter().all(|name| {
            option(name).is_some_and(
                |id| matches!(ctx.heap.get(id), Value::Str(text) if !text.contains('\0')),
            )
        })
}

/// Build from validated input offsets. A memo vector is needed only if the
/// pickle actually retrieves memoized objects.
fn decode(ctx: &mut Context, data: Id, layout: Layout) -> Result<Id> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(layout.dense_prefix + layout.items.len())
        .map_err(|_| crate::Error::memory_error())?;
    let mut memo = Vec::new();
    if layout.has_get {
        memo.try_reserve_exact(layout.memo)
            .map_err(|_| crate::Error::memory_error())?;
        memo.push(NONE); // placeholder for the result list itself
    }
    ctx.heap.reserve_unobserved_texts(layout.texts)?;
    let mark = ctx.heap.temporary_mark();
    let Value::Bytes(bytes) = ctx.heap.get(data) else {
        unreachable!()
    };
    let (pointer, length) = (bytes.as_ptr(), bytes.len());
    // SAFETY: the call operands root this exact immutable bytes object; its Vec
    // backing allocation does not move when heap slots grow. No callback,
    // collection, signal checkpoint, or mutation runs while decoding.
    let input = unsafe { std::slice::from_raw_parts(pointer, length) };
    let mut saw_short = false;
    if layout.dense_prefix != 0 {
        let mut at = 14usize;
        let mut first = 0usize;
        if !layout.has_get {
            // SAFETY: dense preflight validated each exact ASCII entry against this rooted,
            // immutable byte input. Both output IDs and heap slots/operand roots were reserved
            // before the first leaf. The batch stops before a short or longer payload.
            (at, first) = unsafe {
                ctx.heap.append_reserved_dense_ascii_texts(
                    input,
                    at,
                    layout.dense_prefix,
                    &mut values,
                )
            };
        }
        for position in first..layout.dense_prefix {
            // SAFETY: dense preflight validated the complete frame, its short
            // text lengths and ASCII contents. The rooted bytes cannot change.
            let length = usize::from(unsafe { *input.get_unchecked(at + 1) });
            // SAFETY: the same preflight established this exact text range.
            let text = unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                    input.as_ptr().add(at + 2),
                    length,
                ))
            };
            let top = if layout.has_get {
                ctx.heap.unobserved_text(text, true)
            } else {
                ctx.heap
                    .unobserved_text_noting_short(text, true, &mut saw_short)
            };
            values.push(top);
            if layout.has_get {
                memo.push(top);
                if position + 1 == layout.dense_prefix {
                    memo.extend(std::iter::repeat_n(top, layout.prefix_extra_memo));
                }
            }
            at += length + 3;
        }
    }
    for item in layout.items {
        let top = if item.is_get() {
            memo[item.index()]
        } else {
            let start = item.index();
            // SAFETY: preflight checked both the range and UTF-8, and the rooted
            // immutable input has not changed. CompactString copies into its slot.
            let text =
                unsafe { std::str::from_utf8_unchecked(&input[start..start + item.length()]) };
            if layout.has_get {
                ctx.heap.unobserved_text(text, item.is_ascii())
            } else {
                ctx.heap
                    .unobserved_text_noting_short(text, item.is_ascii(), &mut saw_short)
            }
        };
        values.push(top);
        if layout.has_get {
            memo.extend(std::iter::repeat_n(top, item.memo_count()));
        }
    }
    let result = ctx.alloc(Value::List(values));
    if !layout.has_get {
        if layout.texts != 0 && !saw_short {
            ctx.heap.mark_fresh_nonshort_text_list(result);
        } else {
            ctx.heap.mark_fresh_text_list(result);
        }
    }
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
    attempt_data(ctx, data, false, || {}).map(|result| result.map(DirectLoad::value))
}

/// Identify the immediate decoder that produced this invocation's result.
/// Only the text reader without memo GETs can certify fresh, distinct leaves.
pub(in crate::dynamic) enum DirectLoad {
    FreshText(Id),
    Other(Id),
}

impl DirectLoad {
    fn value(self) -> Id {
        match self {
            Self::FreshText(value) | Self::Other(value) => value,
        }
    }
}

/// Try a positional call to the authenticated bundled `pickle.loads` body
/// before allocating and binding an otherwise unobservable Python frame.
pub(in crate::dynamic) fn try_direct_loads(
    ctx: &mut Context,
    scope: Id,
    kwdefaults: Id,
    data: Id,
) -> Result<Option<Id>> {
    try_classified_direct_loads(ctx, scope, kwdefaults, data, || {})
        .map(|result| result.map(DirectLoad::value))
}

/// Invalidate a caller's ownership receipt before decoding a memo alias or VM stream.
pub(in crate::dynamic) fn try_classified_direct_loads(
    ctx: &mut Context,
    scope: Id,
    kwdefaults: Id,
    data: Id,
    before_nonfresh: impl FnOnce(),
) -> Result<Option<DirectLoad>> {
    let root = ctx.root_scope(scope);
    let globals = ctx.scope_data(root);
    if globals.module != "pickle"
        || globals.exec_globals.is_some()
        || !valid_options(ctx, |name| ctx.dict_named_item(kwdefaults, name))
    {
        return Ok(None);
    }
    attempt_data(ctx, data, true, before_nonfresh)
}

fn attempt_data(
    ctx: &mut Context,
    data: Id,
    direct: bool,
    before_nonfresh: impl FnOnce(),
) -> Result<Option<DirectLoad>> {
    let Value::Bytes(bytes) = ctx.heap.get(data) else {
        return Ok(None);
    };
    // Reject observers before scanning or allocating a descriptor vector. No
    // Python callback can alter the environment while preflight runs.
    if !unobserved(ctx) {
        return Ok(None);
    }
    let layout = preflight(bytes);
    let vm = if layout.is_none() {
        super::vm::preflight(bytes)
    } else {
        None
    };
    #[cfg(unix)]
    if crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(None);
    }
    if direct && (layout.is_some() || vm.is_some()) {
        ctx.check_call_depth(0)?;
    }
    match (layout, vm) {
        (Some(layout), _) if !layout.has_get => decode(ctx, data, layout)
            .map(DirectLoad::FreshText)
            .map(Some),
        (Some(layout), _) => {
            before_nonfresh();
            decode(ctx, data, layout).map(DirectLoad::Other).map(Some)
        }
        (_, Some(layout)) => {
            before_nonfresh();
            super::vm::decode(ctx, data, layout)
                .map(DirectLoad::Other)
                .map(Some)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_receipt_is_minted_only_for_the_immediate_text_reader_without_gets() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        for (bytes, fresh) in [
            (
                &b"\x80\x05\x95\x11\0\0\0\0\0\0\0]\x94(\x8c\x03abc\x94\x8c\x03def\x94e."[..],
                true,
            ),
            (
                &b"\x80\x05\x95\x0d\0\0\0\0\0\0\0]\x94(\x8c\x03abc\x94h\x01e."[..],
                false,
            ),
            (&b"\x80\x05\x95\x06\0\0\0\0\0\0\0]\x94K\x01a."[..], false),
        ] {
            let data = ctx.alloc(Value::Bytes(bytes.to_vec()));
            let invalidated = std::cell::Cell::new(false);
            let result = attempt_data(&mut ctx, data, false, || invalidated.set(true))
                .unwrap()
                .unwrap();
            match result {
                DirectLoad::FreshText(value) => {
                    assert!(fresh);
                    assert!(!invalidated.get());
                    assert!(ctx.heap.is_fresh_text_list(value));
                }
                DirectLoad::Other(value) => {
                    assert!(!fresh);
                    assert!(invalidated.get());
                    assert!(!ctx.heap.is_fresh_text_list(value));
                }
            }
        }
    }

    #[test]
    fn nonshort_text_provenance_excludes_empty_short_and_memo_aliases() {
        fn frame(payload: &[u8]) -> Vec<u8> {
            let mut bytes = vec![PROTO, 5, FRAME];
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            bytes.extend_from_slice(payload);
            bytes
        }
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        for (payload, narrow, fresh) in [
            (&b"]\x94(\x8c\x03abc\x94\x8c\x03abc\x94e."[..], true, true),
            (
                &b"]\x94(\x8c\x03abc\x94\x8c\x02\xc3\xa9\x94e."[..],
                false,
                true,
            ),
            (&b"]\x94(\x8c\x03\xe2\x82\xac\x94e."[..], true, true),
            (&b"]\x94(\x8c\x00\x94e."[..], false, true),
            (&b"]\x94(e."[..], false, true),
            (&b"]\x94(\x8c\x03abc\x94h\x01e."[..], false, false),
        ] {
            let data = ctx.alloc(Value::Bytes(frame(payload)));
            let result = attempt_data(&mut ctx, data, false, || {})
                .unwrap()
                .unwrap()
                .value();
            assert_eq!(ctx.heap.is_fresh_text_list(result), fresh);
            assert_eq!(ctx.heap.is_fresh_nonshort_text_list(result), narrow);
            if narrow && payload == b"]\x94(\x8c\x03abc\x94\x8c\x03abc\x94e." {
                let Value::List(items) = ctx.heap.get(result) else {
                    unreachable!()
                };
                assert_ne!(items[0], items[1]);
            }
        }
    }

    #[test]
    fn dense_batch_flushes_before_final_short_and_preserves_distinct_leaves() {
        let mut payload = vec![EMPTY_LIST, MEMOIZE, MARK];
        for _ in 0..99 {
            payload.extend_from_slice(b"\x8c\x05equal\x94");
        }
        payload.extend_from_slice(b"\x8c\x01x\x94e.");
        let mut bytes = vec![PROTO, 5, FRAME];
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let data = ctx.alloc(Value::Bytes(bytes));
        let result = attempt_data(&mut ctx, data, false, || {})
            .unwrap()
            .unwrap()
            .value();
        assert!(ctx.heap.is_fresh_text_list(result));
        assert!(!ctx.heap.is_fresh_nonshort_text_list(result));
        let Value::List(values) = ctx.heap.get(result) else {
            unreachable!()
        };
        assert_eq!(values.len(), 100);
        assert!(values[..99].windows(2).all(|pair| pair[0] != pair[1]));
        assert!(
            values[..99]
                .iter()
                .all(|&id| matches!(ctx.heap.get(id), Value::Str(s) if s == "equal"))
        );
        let short = values[99];
        assert!(matches!(ctx.heap.get(short), Value::Str(s) if s == "x"));
        assert_eq!(short, ctx.heap.unobserved_text("x", true));
    }

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
        assert_eq!(
            (layout.dense_prefix + layout.items.len(), layout.memo),
            (2, 2)
        );
        assert!(layout.has_get);
        assert!(layout.items[0].is_get());
        assert!(preflight(&good[..good.len() - 1]).is_none());
        let mut sparse = good;
        sparse[14] = b'q'; // BINPUT is intentionally unsupported.
        assert!(preflight(&sparse).is_none());
    }

    #[test]
    fn dense_ascii_scan_validates_the_whole_frame_before_decoding() {
        fn frame(payload: &[u8]) -> Vec<u8> {
            let mut data = vec![PROTO, 5, FRAME];
            data.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            data.extend_from_slice(payload);
            data
        }
        let bytes = frame(b"]\x94(\x8c\x03abc\x94\x8c\x00\x94e.");
        let layout = preflight(&bytes).unwrap();
        assert_eq!(layout.dense_prefix, 2);
        assert_eq!((layout.texts, layout.memo), (2, 3));
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let data = ctx.alloc(Value::Bytes(bytes.clone()));
        let result = decode(&mut ctx, data, layout).unwrap();
        let Value::List(values) = ctx.heap.get(result) else {
            panic!("not a list")
        };
        assert!(matches!(ctx.heap.get(values[0]), Value::Str(value) if value == "abc"));
        assert!(matches!(ctx.heap.get(values[1]), Value::Str(value) if value.is_empty()));

        let unicode = frame(b"]\x94(\x8c\x02\xc3\xa9\x94e.");
        assert_eq!(preflight(&unicode).unwrap().dense_prefix, 0);
        let alias = frame(b"]\x94(\x8c\x03abc\x94h\x01e.");
        let layout = preflight(&alias).unwrap();
        assert_eq!(layout.dense_prefix, 1);
        assert!(layout.has_get);
        let repeated = frame(b"]\x94(\x8c\x03abc\x94\x94h\x02e.");
        let layout = preflight(&repeated).unwrap();
        assert_eq!(layout.prefix_extra_memo, 1);
        let data = ctx.alloc(Value::Bytes(repeated));
        let result = decode(&mut ctx, data, layout).unwrap();
        let Value::List(values) = ctx.heap.get(result) else {
            panic!("not a list")
        };
        assert_eq!(values[0], values[1]);
        assert!(preflight(&bytes[..bytes.len() - 1]).is_none());
        let no_memo = frame(b"]\x94(\x8c\x03abce.");
        assert_eq!(preflight(&no_memo).unwrap().dense_prefix, 0);
    }
}
