//! Writable byte windows passed to Python raw.readinto callbacks.
use super::{Context, Error, Id, Result, Value};
use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct Window {
    pub(super) backing: Id,
    backing_len: usize,
    start: usize,
    pub(super) len: usize,
}

impl Window {
    /// Retained raw views remain usable only while their backing storage has its original size.
    pub(super) fn checked_range(self, backing_len: usize) -> Result<Range<usize>> {
        if backing_len != self.backing_len {
            return Err(Error::new(
                "BufferError",
                "underlying buffer has changed size",
            ));
        }
        Ok(self.start..self.start + self.len)
    }
}

impl Context {
    fn mutable_storage_len(&self, id: Id) -> Option<usize> {
        match self.heap.get(id) {
            Value::ByteArray(bytes) => Some(bytes.len()),
            Value::Array(array) => Some(array.as_bytes().len()),
            Value::BytesIoStorage(Some(file)) => Some(file.storage().len()),
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
        readonly: bool,
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
            lease: None,
            anonymous: true,
            layout: None,
            readonly,
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

    pub(super) fn window_bytes(&self, w: Window) -> Result<&[u8]> {
        let data = self.owner_bytes(w.backing)?;
        Ok(&data[w.checked_range(data.len())?])
    }

    pub(super) fn writable_byte_span(&self, object: Id) -> Result<Option<(Id, Range<usize>)>> {
        self.contiguous_byte_span(object, true)
    }

    /// Return the backing and byte range of an exported contiguous buffer.
    pub(super) fn contiguous_byte_span(
        &self,
        object: Id,
        writable: bool,
    ) -> Result<Option<(Id, Range<usize>)>> {
        self.check_buffer_exporter(object)?;
        if writable
            && matches!(self.heap.get(object), Value::MemoryView { .. })
            && self.view_is_readonly(object)?
        {
            return Ok(None);
        }
        let storage_len = |id| match self.heap.get(id) {
            Value::Bytes(bytes) if !writable => Some(bytes.len()),
            _ => self.mutable_storage_len(id),
        };
        let (owner, start, len) = match *self.heap.get(object) {
            Value::MemoryView { owner, .. } => {
                let owner = Self::memoryview_owner(owner)?;
                let view = self.view_layout(object)?;
                if !view.is_contiguous() {
                    return Ok(None);
                }
                let (_, width) = self.view_format_width(owner, view);
                (owner, view.byte_start(width), view.len * width)
            }
            Value::BytesIoBuffer { .. } => (object, 0, self.view_element_count(object)?),
            _ => {
                let storage = self.scalar_value(object);
                return Ok(storage_len(storage).map(|n| (storage, 0..n)));
            }
        };
        if let Value::RawReadWindow(w) = *self.heap.get(owner) {
            // The view layout was checked against the window; exporters use backing coordinates.
            let start = w.start + start;
            return Ok(Some((w.backing, start..start + len)));
        }
        if writable {
            self.invalidate_stream_snapshot(owner);
        }
        let storage = self.view_storage_id(owner);
        Ok(storage_len(storage).map(|_| (storage, start..start + len)))
    }
}
