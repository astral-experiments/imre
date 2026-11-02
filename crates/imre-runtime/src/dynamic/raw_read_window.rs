//! Writable byte windows passed to Python raw.readinto callbacks.
use super::{Context, Error, Id, Result, Value, slice};
use num_traits::ToPrimitive;
use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct Window {
    pub(super) backing: Id,
    backing_len: usize,
    start: usize,
    pub(super) len: usize,
}

impl Context {
    fn mutable_storage_len(&self, id: Id) -> Option<usize> {
        match self.heap.get(id) {
            Value::ByteArray(bytes) => Some(bytes.len()),
            Value::Array(array) => Some(array.as_bytes().len()),
            _ => None,
        }
    }

    /// A raw callback can retain its view. Trace the backing allocation separately from
    /// public exports so retaining a callback view does not pin the caller's buffer size.
    pub(super) fn ownerless_byte_view(
        &mut self,
        backing: Id,
        start: usize,
        len: usize,
    ) -> Result<Id> {
        let backing_len = self
            .mutable_storage_len(backing)
            .ok_or_else(|| Error::new("SystemError", "expected writable byte storage"))?;
        if start > backing_len || len > backing_len - start {
            return Err(Error::new("SystemError", "view outside buffer"));
        }
        let owner = self.alloc(Value::RawReadWindow(Window {
            backing,
            backing_len,
            start,
            len,
        }));
        Ok(self.alloc(Value::MemoryView {
            owner: Some(owner),
            anonymous: true,
            hash: None,
        }))
    }

    pub(super) fn raw_read_window(&self, id: Id) -> Option<Window> {
        if let Value::MemoryView {
            owner: Some(owner), ..
        } = self.heap.get(id)
            && let Value::RawReadWindow(window) = self.heap.get(*owner)
        {
            Some(*window)
        } else {
            None
        }
    }

    fn checked_window(&self, id: Id) -> Result<Window> {
        let w = self.raw_read_window(id).ok_or_else(|| {
            Error::new(
                "ValueError",
                "operation forbidden on released memoryview object",
            )
        })?;
        self.window_bytes(w)?;
        Ok(w)
    }

    pub(super) fn window_bytes(&self, w: Window) -> Result<&[u8]> {
        let data = match self.heap.get(w.backing) {
            Value::ByteArray(bytes) => bytes.as_slice(),
            Value::Array(array) => array.as_bytes(),
            _ => unreachable!("traced writable storage"),
        };
        if data.len() != w.backing_len {
            return Err(Error::new(
                "BufferError",
                "underlying buffer has changed size",
            ));
        }
        Ok(&data[w.start..w.start + w.len])
    }

    pub(super) fn writable_byte_span(&self, object: Id) -> Result<Option<(Id, Range<usize>)>> {
        self.check_buffer_exporter(object)?;
        if let Some(w) = self.raw_read_window(object) {
            self.window_bytes(w)?;
            return Ok(Some((w.backing, w.start..w.start + w.len)));
        }
        let owner = match self.heap.get(object) {
            Value::MemoryView { owner, .. } => Self::memoryview_owner(*owner)?,
            _ => object,
        };
        let storage = self.scalar_value(owner);
        Ok(self.mutable_storage_len(storage).map(|n| (storage, 0..n)))
    }

    fn raw_view_index(&mut self, id: Id, key: Id) -> Result<usize> {
        let n = self
            .index_integer(key)?
            .to_i64()
            .ok_or_else(|| Error::new("IndexError", "memoryview index out of range"))?;
        let len = self.checked_window(id)?.len as i64;
        let pos = if n < 0 { n + len } else { n };
        if !(0..len).contains(&pos) {
            return Err(Error::new(
                "IndexError",
                "index out of bounds on dimension 1",
            ));
        }
        Ok(pos as usize)
    }

    fn raw_view_slice(&mut self, id: Id, parts: [Id; 3]) -> Result<Range<usize>> {
        let bounds = slice::sequence_components(self, parts)?;
        let [start, end, step] = slice::normalize(bounds, self.checked_window(id)?.len as i64);
        if step != 1 {
            return Err(Error::new(
                "NotImplementedError",
                "strided raw buffer windows are unsupported",
            ));
        }
        Ok(start as usize..end.max(start) as usize)
    }

    pub(super) fn raw_view_get(&mut self, id: Id, key: Id) -> Result<Id> {
        if let Value::Slice(parts) = *self.heap.get(key) {
            let range = self.raw_view_slice(id, parts)?;
            let w = self.checked_window(id)?;
            return self.ownerless_byte_view(w.backing, w.start + range.start, range.len());
        }
        let pos = self.raw_view_index(id, key)?;
        let byte = self.window_bytes(self.checked_window(id)?)?[pos];
        Ok(self.int(i64::from(byte)))
    }

    pub(super) fn raw_view_set(&mut self, id: Id, key: Id, value: Id) -> Result<()> {
        let w = self.checked_window(id)?;
        self.with_buffer_export(w.backing, |ctx| {
            let (range, input) = if let Value::Slice(parts) = *ctx.heap.get(key) {
                let range = ctx.raw_view_slice(id, parts)?;
                ctx.check_buffer_exporter(value)?;
                // Snapshot first for assignments between overlapping windows.
                let source = match ctx.buffer_storage(value)? {
                    Value::Array(array) if array.view_layout().0 == "B" => array.as_bytes(),
                    Value::Array(_) => {
                        return Err(Error::new(
                            "ValueError",
                            "memoryview assignment: lvalue and rvalue have different structures",
                        ));
                    }
                    _ => ctx.bytes_like(value)?,
                };
                if source.len() != range.len() {
                    return Err(Error::new(
                        "ValueError",
                        "memoryview assignment: lvalue and rvalue have different structures",
                    ));
                }
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(source.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                bytes.extend_from_slice(source);
                (range, bytes)
            } else {
                let index = ctx.raw_view_index(id, key)?;
                let byte = ctx.index_integer(value)?.big().to_u8().ok_or_else(|| {
                    Error::new("ValueError", "memoryview: invalid value for format 'B'")
                })?;
                (index..index + 1, vec![byte])
            };
            let w = ctx.checked_window(id)?;
            let dst = match ctx.heap.get_mut(w.backing) {
                Value::ByteArray(bytes) => bytes.as_mut_slice(),
                Value::Array(array) => array.as_bytes_mut()?,
                _ => unreachable!("traced writable storage"),
            };
            dst[w.start + range.start..w.start + range.end].copy_from_slice(&input);
            Ok(())
        })
    }
}
