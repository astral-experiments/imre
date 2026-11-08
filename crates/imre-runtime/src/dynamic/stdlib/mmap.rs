//! Linux file mappings and their buffer exports.
use super::{Context, Error, Id, NONE, Result, Value, builtins, os};
use crate::dynamic::memoryview::Storage;
use crate::dynamic::operators::{slice_count, slice_positions};
use crate::dynamic::slice;
use std::ops::Range;
use std::rc::Rc;

mod region;
pub(in crate::dynamic) use region::{Region, create, install};

pub(in crate::dynamic) const METHODS: &[&str] = &[
    "__getitem__",
    "__setitem__",
    "close",
    "__enter__",
    "__exit__",
    "size",
    "flush",
    "find",
    "rfind",
    "move",
    "seek",
    "tell",
    "read",
    "write",
];

pub(in crate::dynamic) fn region(obj: &Value) -> Result<&Rc<Region>> {
    match obj {
        Value::Mmap {
            region: Some(region),
            ..
        } => Ok(region),
        Value::Mmap { region: None, .. } => Err(Error::new("ValueError", "mmap closed or invalid")),
        _ => Err(Error::new("TypeError", "expected mmap object")),
    }
}

/// A live file map may be changed outside this process. Only ordinary owned bytes
/// may be lent to a Rust consumer; defer the copy until it requests this range.
pub(in crate::dynamic) fn read(map: &Region, range: Range<usize>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(range.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    out.resize(range.len(), 0);
    map.read_into(range.start, &mut out)?;
    Ok(out)
}

pub(in crate::dynamic) fn owner(ctx: &Context, owner: Id) -> Result<Option<(Id, Range<usize>)>> {
    let id = ctx.view_storage_id(owner);
    if let Value::RawReadWindow(w) = *ctx.heap.get(id) {
        let storage = ctx.view_storage_id(w.backing);
        if matches!(ctx.heap.get(storage), Value::Mmap { .. }) {
            return Ok(Some((
                storage,
                w.checked_range(region(ctx.heap.get(storage))?.len())?,
            )));
        }
    }
    if matches!(ctx.heap.get(id), Value::Mmap { .. }) {
        Ok(Some((id, 0..region(ctx.heap.get(id))?.len())))
    } else {
        Ok(None)
    }
}

/// Resolve a contiguous consumer's current view, without reading or borrowing mapped bytes.
pub(in crate::dynamic) fn mapped_span(
    ctx: &Context,
    value: Id,
) -> Result<Option<(Id, Range<usize>)>> {
    if let Value::MemoryView { owner: source, .. } = *ctx.heap.get(value) {
        let source = Context::memoryview_owner(source)?;
        let Some((storage, range)) = owner(ctx, source)? else {
            return Ok(None);
        };
        ctx.check_view_export(value)?;
        let layout = ctx.view_layout(value)?;
        if !layout.is_contiguous() {
            return Err(Error::new(
                "BufferError",
                "memoryview: underlying buffer is not C-contiguous",
            ));
        }
        let (_, width) = ctx.view_format_width(source, &layout);
        let begin = range.start + layout.byte_start(width);
        Ok(Some((storage, begin..begin + layout.len * width)))
    } else {
        owner(ctx, value)
    }
}

pub(in crate::dynamic) fn unsupported_borrow() -> Error {
    Error::new(
        "NotImplementedError",
        "this buffer operation does not support borrowed mapped memory",
    )
}

fn writable_region(obj: &Value) -> Result<&Region> {
    let mapping = region(obj)?;
    if mapping.readonly() {
        return Err(Error::new(
            "TypeError",
            "mmap can't modify a readonly memory map.",
        ));
    }
    Ok(mapping)
}

fn integer_arg(ctx: &mut Context, value: Id, required: &str, overflow: &str) -> Result<i64> {
    ctx.index_value(value)?
        .ok_or_else(|| Error::new("TypeError", required))?
        .to_i64()
        .ok_or_else(|| {
            Error::new(
                overflow,
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    ctx.type_name(value)
                ),
            )
        })
}

fn index(ctx: &mut Context, len: i64, key: Id) -> Result<usize> {
    let n = integer_arg(ctx, key, "mmap indices must be integers", "IndexError")?;
    let i = if n < 0 { n + len } else { n };
    if !(0..len).contains(&i) {
        return Err(Error::new("IndexError", "mmap index out of range"));
    }
    Ok(i as usize)
}

fn bounds(ctx: &mut Context, len: i64, parts: [Id; 3]) -> Result<[i64; 3]> {
    let components = slice::sequence_components(ctx, parts)?;
    Ok(slice::normalize(components, len))
}

pub(in crate::dynamic) fn get_item(ctx: &mut Context, map: Id, key: Id) -> Result<Id> {
    let len = region(ctx.heap.get(map))?.len() as i64;
    if let Value::Slice(parts) = *ctx.heap.get(key) {
        let [start, stop, step] = bounds(ctx, len, parts)?;
        let mapping = region(ctx.heap.get(map))?;
        let value = if step == 1 {
            read(mapping, start as usize..stop.max(start) as usize)?
        } else {
            let mut result = Vec::new();
            result
                .try_reserve_exact(slice_count(start, stop, step))
                .map_err(|_| Error::new("MemoryError", ""))?;
            for i in slice_positions(start, stop, step) {
                let mut byte = [0];
                mapping.read_into(i, &mut byte)?;
                result.push(byte[0]);
            }
            result
        };
        Ok(ctx.alloc(Value::Bytes(value)))
    } else {
        let i = index(ctx, len, key)?;
        let mut byte = [0];
        region(ctx.heap.get(map))?.read_into(i, &mut byte)?;
        Ok(ctx.int(i64::from(byte[0])))
    }
}

/// Copy a contiguous export while its owner is held. Stepped assignment reads the
/// source in order, so later bytes can observe earlier writes to the same mapping.
fn write_buffer(
    destination: &Region,
    start: i64,
    stop: i64,
    step: i64,
    source: Storage<'_>,
) -> Result<()> {
    match source {
        Storage::Native(bytes) if step == 1 => destination.write_from(start as usize, bytes),
        Storage::Native(bytes) => {
            for (index, byte) in slice_positions(start, stop, step).zip(bytes) {
                destination.write_from(index, std::slice::from_ref(byte))?;
            }
            Ok(())
        }
        Storage::Mapped(region, range) => destination.copy_from_map(start, step, region, range),
    }
}

pub(in crate::dynamic) fn put_item(ctx: &mut Context, map: Id, key: Id, value: Id) -> Result<()> {
    // Closing a map during conversion leaves its logical length available for
    // bounds checks. Reacquire the region only after the value is converted.
    let len = writable_region(ctx.heap.get(map))?.len() as i64;
    if let Value::Slice(parts) = *ctx.heap.get(key) {
        let [start, stop, step] = bounds(ctx, len, parts)?;
        ctx.with_read_buffer(value, |ctx, input| {
            let source = ctx.contiguous_buffer(input)?;
            let count = slice_count(start, stop, step);
            if source.len() != count {
                return Err(Error::new(
                    "IndexError",
                    "mmap slice assignment is wrong size",
                ));
            }
            let mapping = region(ctx.heap.get(map))?;
            write_buffer(mapping, start, stop, step, source)
        })
    } else {
        let i = index(ctx, len, key)?;
        let value = integer_arg(ctx, value, "mmap item value must be an int", "TypeError")?;
        let byte = u8::try_from(value)
            .map_err(|_| Error::new("ValueError", "mmap item value must be in range(0, 256)"))?;
        region(ctx.heap.get(map))?.write_from(i, &[byte])
    }
}

/// Search a live range while the needle's simple buffer export is held.
fn find(ctx: &mut Context, map: Id, args: &[Id], reverse: bool) -> Result<Id> {
    let index = ctx.with_read_buffer(args[0], |ctx, view| {
        region(ctx.heap.get(map))?;
        let Value::Mmap { pos, .. } = *ctx.heap.get(map) else {
            unreachable!()
        };
        // CPython uses the cursor and ignores end when start is omitted or None.
        let (start, end) = match args.get(1).copied().filter(|&v| v != NONE) {
            Some(value) => {
                let start = os::ssize_t(ctx, value)?;
                let end = args
                    .get(2)
                    .copied()
                    .filter(|&v| v != NONE)
                    .map(|v| os::ssize_t(ctx, v))
                    .transpose()?;
                (start, end)
            }
            None => (pos as isize, None),
        };
        // An index callback can close the receiver or mutate the exported needle.
        let mapping = region(ctx.heap.get(map))?;
        let [start, end, _] = slice::normalize([Some(start), end, None], mapping.len() as isize);
        if start > end {
            return Ok(-1);
        }
        let haystack = Storage::Mapped(mapping, start as usize..end as usize);
        let needle = ctx.contiguous_buffer(view)?;
        Ok(haystack
            .find(&needle, reverse)?
            .map_or(-1, |i| start as i64 + i as i64))
    })?;
    Ok(ctx.int(index))
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (map, args) = builtins::method_receiver_args(ctx, "mmap.mmap", name, args, kwargs)?;
    let n = args.len();
    let error = match name {
        "close" | "tell" | "size" | "__enter__" if n != 0 => {
            Some(format!("mmap.{name}() takes no arguments ({n} given)"))
        }
        "__exit__" | "move" if n != 3 => Some(format!("{name} expected 3 arguments, got {n}")),
        "seek" if n == 0 => Some("seek expected at least 1 argument, got 0".to_owned()),
        "seek" | "flush" if n > 2 => Some(format!("{name} expected at most 2 arguments, got {n}")),
        "find" | "rfind" if n == 0 => Some(format!("{name} expected at least 1 argument, got 0")),
        "find" | "rfind" if n > 3 => Some(format!("{name} expected at most 3 arguments, got {n}")),
        "read" if n > 1 => Some(format!("read expected at most 1 argument, got {n}")),
        _ => None,
    };
    if let Some(error) = error {
        return Err(Error::new("TypeError", error));
    }
    match name {
        "__getitem__" => {
            let key = builtins::one_positional_arg("mmap.__getitem__", args, kwargs)?;
            get_item(ctx, map, key)
        }
        "__setitem__" => {
            let [key, value] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!("expected 2 arguments, got {n}"),
                ));
            };
            put_item(ctx, map, *key, *value)?;
            Ok(NONE)
        }
        "close" | "__exit__" => {
            if ctx.buffer_is_exported(map)? {
                return Err(Error::new(
                    "BufferError",
                    "cannot close exported pointers exist",
                ));
            }
            let Value::Mmap { region, .. } = ctx.heap.get_mut(map) else {
                unreachable!()
            };
            *region = None;
            Ok(NONE)
        }
        "__enter__" | "size" | "tell" => {
            let mapping = region(ctx.heap.get(map))?;
            match name {
                "size" => Ok(ctx.int(mapping.size()?)),
                "tell" => {
                    let Value::Mmap { pos, .. } = *ctx.heap.get(map) else {
                        unreachable!()
                    };
                    Ok(ctx.int(pos as i64))
                }
                _ => Ok(map),
            }
        }
        "find" | "rfind" => find(ctx, map, args, name == "rfind"),
        "flush" => {
            let offset = args.first().copied();
            let size = args.get(1).copied();
            let offset = offset
                .map(|n| os::ssize_t(ctx, n))
                .transpose()?
                .unwrap_or(0);
            // Conversion of the offset precedes the closed check; conversion of size follows it.
            let mapping = region(ctx.heap.get(map))?;
            let len = mapping.len();
            let sync = mapping.sync;
            let size = size
                .filter(|&n| n != NONE)
                .map(|n| os::ssize_t(ctx, n))
                .transpose()?;
            let bad_range = || Error::new("ValueError", "flush values out of range");
            let offset = usize::try_from(offset).map_err(|_| bad_range())?;
            let size = usize::try_from(size.unwrap_or(len as isize)).map_err(|_| bad_range())?;
            if offset > len || size > len - offset {
                return Err(bad_range());
            }
            if sync {
                let Value::Mmap { region, .. } = ctx.heap.get(map) else {
                    unreachable!()
                };
                match region {
                    Some(mapping) => mapping.flush(offset, size)?,
                    None => region::flush_after_close(offset, size)?,
                }
            }
            Ok(NONE)
        }
        "move" => {
            let dest = os::ssize_t(ctx, args[0])?;
            let src = os::ssize_t(ctx, args[1])?;
            let count = os::ssize_t(ctx, args[2])?;
            let mapping = writable_region(ctx.heap.get(map))?;
            let len = mapping.len() as isize;
            if dest < 0 || src < 0 || count < 0 || len - dest < count || len - src < count {
                return Err(Error::new(
                    "ValueError",
                    "source, destination, or count out of range",
                ));
            }
            mapping.copy_from_map(
                dest as i64,
                1,
                mapping,
                src as usize..(src + count) as usize,
            )?;
            Ok(NONE)
        }
        "seek" => {
            let whence = args.get(1).copied();
            let offset = os::ssize_t(ctx, args[0])?;
            let whence = whence.map(|n| ctx.index_c_int(n)).transpose()?.unwrap_or(0);
            let len = region(ctx.heap.get(map))?.len();
            let Value::Mmap { pos, .. } = ctx.heap.get_mut(map) else {
                unreachable!()
            };
            let start = match whence {
                0 => 0,
                1 => *pos,
                2 => len,
                _ => return Err(Error::new("ValueError", "unknown seek type")),
            };
            *pos = start
                .checked_add_signed(offset)
                .filter(|&n| n <= len)
                .ok_or_else(|| Error::new("ValueError", "seek out of range"))?;
            let result = *pos as i64;
            Ok(ctx.int(result))
        }
        "read" => {
            let count = args.first().copied();
            let count = count
                .filter(|&id| id != NONE)
                .map(|n| os::ssize_t(ctx, n))
                .transpose()?
                .unwrap_or(-1);
            let Value::Mmap { pos, .. } = *ctx.heap.get(map) else {
                unreachable!()
            };
            let mapping = region(ctx.heap.get(map))?;
            let count = usize::try_from(count)
                .unwrap_or(usize::MAX)
                .min(mapping.len() - pos);
            let result = read(mapping, pos..pos + count)?;
            let Value::Mmap { pos, .. } = ctx.heap.get_mut(map) else {
                unreachable!()
            };
            *pos += count;
            Ok(ctx.alloc(Value::Bytes(result)))
        }
        "write" => {
            let input = builtins::one_positional_arg("mmap.write", args, kwargs)?;
            ctx.with_read_buffer(input, |ctx, input| {
                let mapping = writable_region(ctx.heap.get(map))?;
                let source = ctx.contiguous_buffer(input)?;
                let Value::Mmap { pos, .. } = *ctx.heap.get(map) else {
                    unreachable!()
                };
                let size = source.len();
                if size > mapping.len() - pos {
                    return Err(Error::new("ValueError", "data out of range"));
                }
                write_buffer(mapping, pos as i64, (pos + size) as i64, 1, source)?;
                let Value::Mmap { pos, .. } = ctx.heap.get_mut(map) else {
                    unreachable!()
                };
                *pos += size;
                Ok(ctx.int(size as i64))
            })
        }
        _ => unreachable!("registered mmap method"),
    }
}
