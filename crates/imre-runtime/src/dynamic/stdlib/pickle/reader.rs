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
    let mut items: Vec<Item> = Vec::new();
    items
        .try_reserve((end.saturating_sub(14) / 8).min(1024))
        .ok()?;
    let mut memo = 1usize;
    let mut has_get = false;
    let mut texts = 0usize;
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
                items.last_mut()?.memoize()?;
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
    ctx.heap.reserve_unobserved_texts(layout.texts)?;
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
            ctx.heap.unobserved_text(text, item.is_ascii())
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
