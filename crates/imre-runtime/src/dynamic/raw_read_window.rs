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
            Value::BytesIoStorage(state) => state.file.as_ref().map(|file| file.storage().len()),
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
            restricted: false,
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

    pub(super) fn writable_byte_span(&self, object: Id) -> Result<Option<(Id, Range<usize>)>> {
        self.contiguous_byte_span(object, true)
    }

    /// Acquire a contiguous writable buffer and retain its view and storage across Python calls.
    pub(super) fn with_writable_byte_span<T>(
        &mut self,
        object: Id,
        argument: &str,
        consume: impl FnOnce(&mut Self, Id, Range<usize>) -> Result<T>,
    ) -> Result<T> {
        // PyArg_Parse("w*") replaces any acquisition failure with this TypeError.
        let invalid = |ctx: &Self| {
            let kind = if object == super::NONE {
                "None"
            } else {
                ctx.type_name(object)
            };
            let kind = &kind[..kind.floor_char_boundary(50)];
            Error::new(
                "TypeError",
                format!("{argument} must be read-write bytes-like object, not {kind}"),
            )
        };
        let storage = self.scalar_value(object);
        if let Value::ByteArray(bytes) = self.heap.get(storage)
            && self.buffer_slot(object, "__buffer__") == Some(None)
        {
            let range = 0..bytes.len();
            return self.with_native_bytes_export(object, |ctx, _| consume(ctx, storage, range));
        }
        let method = self
            .python_buffer_method(object, true)
            .map_err(|_| invalid(self))?;
        let view = match method {
            Some(method) => self
                .acquire_python_buffer(object, method, 1)
                .map_err(|_| invalid(self))?,
            None => object,
        };
        let (storage, range) = self
            .writable_byte_span(view)
            .ok()
            .flatten()
            .ok_or_else(|| invalid(self))?;
        let write =
            |ctx: &mut Self| ctx.with_buffer_export(storage, |ctx| consume(ctx, storage, range));
        if method.is_some() {
            self.with_python_buffer_export(object, view, write)
        } else {
            self.with_buffer_export(view, write)
        }
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
                let (_, width) = self.view_format_width(owner, &view);
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

/// Reusable scratch space for a single I/O caller's exported spans.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) struct IoVectors {
    vectors: Vec<libc::iovec>,
    bases: std::collections::HashMap<Id, (*mut u8, usize)>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl IoVectors {
    pub(super) fn new(count: usize) -> Result<Self> {
        let mut vectors = Vec::new();
        let mut bases = std::collections::HashMap::new();
        vectors
            .try_reserve_exact(count)
            .map_err(|_| crate::Error::memory_error())?;
        bases
            .try_reserve(count)
            .map_err(|_| crate::Error::memory_error())?;
        Ok(Self { vectors, bases })
    }

    pub(super) fn readable<'a>(
        &'a mut self,
        ctx: &'a mut Context,
        spans: &[(Id, Range<usize>)],
    ) -> Result<BorrowedIoVectors<'a>> {
        self.borrow(ctx, spans, false)
    }

    pub(super) fn writable<'a>(
        &'a mut self,
        ctx: &'a mut Context,
        spans: &[(Id, Range<usize>)],
    ) -> Result<BorrowedIoVectors<'a>> {
        self.borrow(ctx, spans, true)
    }

    fn borrow<'a>(
        &'a mut self,
        ctx: &'a mut Context,
        spans: &[(Id, Range<usize>)],
        writable: bool,
    ) -> Result<BorrowedIoVectors<'a>> {
        use std::collections::hash_map::Entry;
        // Safe callers can forget a loan. Always resolve this context's addresses afresh.
        self.bases.clear();
        self.vectors.clear();
        let loan = BorrowedIoVectors { scratch: self, ctx };
        for (storage, range) in spans {
            let (base, length) = match loan.scratch.bases.entry(*storage) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let backing = if writable {
                        let bytes = loan.ctx.writable_storage_bytes(*storage)?;
                        (bytes.as_mut_ptr(), bytes.len())
                    } else {
                        let bytes = loan.ctx.owner_bytes(*storage)?;
                        (bytes.as_ptr().cast_mut(), bytes.len())
                    };
                    *entry.insert(backing)
                }
            };
            if range.start > range.end || range.end > length {
                return Err(Error::new(
                    "BufferError",
                    "underlying buffer has changed size",
                ));
            }
            loan.scratch.vectors.push(libc::iovec {
                iov_base: base.wrapping_add(range.start).cast(),
                iov_len: range.len(),
            });
        }
        Ok(loan)
    }
}

/// Export roots keep the Python objects alive. This loan also keeps their owning
/// context exclusively borrowed until native code finishes with the addresses.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) struct BorrowedIoVectors<'a> {
    scratch: &'a mut IoVectors,
    ctx: &'a mut Context,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl BorrowedIoVectors<'_> {
    /// # Safety
    /// Use the vectors only for synchronous native I/O during this borrow. Do
    /// not retain the addresses, reenter Python, or write through readable vectors.
    pub(super) unsafe fn as_raw(&mut self) -> &mut [libc::iovec] {
        &mut self.scratch.vectors
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for BorrowedIoVectors<'_> {
    fn drop(&mut self) {
        self.scratch.vectors.clear();
        self.scratch.bases.clear();
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn vector_scratch_releases_partial_and_completed_loans() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let id = ctx.alloc(Value::ByteArray(vec![1, 2, 3]));
        let mut scratch = IoVectors::new(2).unwrap();
        assert!(
            scratch
                .writable(&mut ctx, &[(id, 0..2), (id, 1..4)])
                .is_err()
        );
        assert!(scratch.vectors.is_empty() && scratch.bases.is_empty());

        let loan = scratch
            .readable(&mut ctx, &[(id, 0..2), (id, 1..3)])
            .unwrap();
        assert_eq!(loan.scratch.bases.len(), 1);
        assert_eq!(loan.scratch.vectors.len(), 2);
        drop(loan);
        assert!(scratch.vectors.is_empty() && scratch.bases.is_empty());
    }
}
