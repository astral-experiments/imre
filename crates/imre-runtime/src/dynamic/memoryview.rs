//! One-dimensional unsigned-byte views. The traced backing allocation is independent of
//! the exporter visible as `obj`, which is None for native PyMemoryView_FromBuffer views.
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, slice};

#[derive(Clone, Copy)]
pub(super) struct View {
    pub(super) backing: Id,
    pub(super) exporter: Id,
    backing_len: usize,
    start: i64,
    stride: i64,
    pub(super) len: usize,
    readonly: bool,
}

pub(super) const METHODS: &[&str] = &["release", "tobytes", "__enter__", "__exit__"];

fn released() -> Error {
    Error::new(
        "ValueError",
        "operation forbidden on released memoryview object",
    )
}

impl View {
    fn offset(self, i: usize) -> usize {
        // Slicing a valid view only selects positions within its original backing.
        (i128::from(self.start) + i as i128 * i128::from(self.stride)) as usize
    }

    fn select(self, bounds: [Option<i64>; 3]) -> Self {
        let [start, stop, step] = slice::normalize(bounds, self.len as i64);
        let span = if step > 0 { stop - start } else { start - stop };
        let len = (span.max(0) as u64).div_ceil(step.unsigned_abs()) as usize;
        Self {
            start: if len == 0 {
                0
            } else {
                self.offset(start as usize) as i64
            },
            stride: self.stride.wrapping_mul(step),
            len,
            ..self
        }
    }

    fn contiguous(self) -> bool {
        self.len <= 1 || self.stride == 1
    }
}

impl Context {
    /// Borrow part of a byte allocation for a native callback. Its caller keeps the storage
    /// exported during the call. A retained view traces the storage independently.
    pub(super) fn ownerless_byte_view(
        &mut self,
        backing: Id,
        start: usize,
        len: usize,
    ) -> Result<Id> {
        let Value::ByteArray(data) = self.heap.get(backing) else {
            return Err(Error::new("SystemError", "expected writable byte storage"));
        };
        if start > data.len() || len > data.len() - start {
            return Err(Error::new("SystemError", "view outside buffer"));
        }
        let view = View {
            backing,
            exporter: NONE,
            backing_len: data.len(),
            start: start as i64,
            stride: 1,
            len,
            readonly: false,
        };
        Ok(self.alloc(Value::MemoryView(Some(view))))
    }

    /// Locate a C-contiguous writable byte buffer without borrowing it across Python calls.
    /// Callers supply the converter-specific error when the argument cannot be exported.
    pub(super) fn writable_byte_span(&self, id: Id) -> Option<(Id, std::ops::Range<usize>)> {
        let storage = self.scalar_value(id);
        match self.heap.get(storage) {
            Value::ByteArray(bytes) => Some((storage, 0..bytes.len())),
            Value::MemoryView(_) => {
                let view = self.byte_view(storage).ok()?;
                let start = view.start as usize;
                (!view.readonly && view.contiguous())
                    .then(|| (view.backing, start..start + view.len))
            }
            _ => None,
        }
    }

    pub(super) fn byte_view(&self, id: Id) -> Result<View> {
        match self.heap.get(id) {
            Value::MemoryView(Some(view)) => {
                // Native ownerless exports need not pin the caller's bytearray beyond the
                // callback. Do not apply old offsets after its storage changes size.
                if self.view_storage(*view).len() != view.backing_len {
                    return Err(Error::new(
                        "BufferError",
                        "underlying buffer has changed size",
                    ));
                }
                Ok(*view)
            }
            Value::MemoryView(None) => Err(released()),
            _ => Err(Error::new("TypeError", "expected a memoryview")),
        }
    }

    fn view_storage(&self, view: View) -> &[u8] {
        match self.heap.get(view.backing) {
            Value::ByteArray(data) | Value::Bytes(data) => data,
            _ => unreachable!("traced byte storage"),
        }
    }

    pub(super) fn view_contiguous_bytes(&self, id: Id) -> Result<&[u8]> {
        let view = self.byte_view(id)?;
        if !view.contiguous() {
            return Err(Error::new(
                "BufferError",
                "memoryview: underlying buffer is not C-contiguous",
            ));
        }
        let start = view.start as usize;
        Ok(&self.view_storage(view)[start..start + view.len])
    }

    pub(super) fn view_snapshot(&self, id: Id) -> Result<Vec<u8>> {
        let view = self.byte_view(id)?;
        let data = self.view_storage(view);
        let mut out = Vec::new();
        out.try_reserve_exact(view.len)
            .map_err(|_| Error::new("MemoryError", ""))?;
        out.extend((0..view.len).map(|i| data[view.offset(i)]));
        Ok(out)
    }

    pub(super) fn view_property(&mut self, id: Id, name: &str) -> Result<Option<Id>> {
        if !matches!(
            name,
            "obj"
                | "readonly"
                | "format"
                | "ndim"
                | "itemsize"
                | "nbytes"
                | "shape"
                | "strides"
                | "suboffsets"
                | "contiguous"
                | "c_contiguous"
                | "f_contiguous"
        ) {
            return Ok(None);
        }
        let view = self.byte_view(id)?;
        let value = match name {
            "obj" => view.exporter,
            "readonly" => {
                if view.readonly {
                    TRUE
                } else {
                    FALSE
                }
            }
            "contiguous" | "c_contiguous" | "f_contiguous" => {
                if view.contiguous() {
                    TRUE
                } else {
                    FALSE
                }
            }
            "format" => self.text("B"),
            "ndim" | "itemsize" => self.int(1),
            "nbytes" => self.int(view.len as i64),
            "shape" | "strides" => {
                let value = self.int(if name == "shape" {
                    view.len as i64
                } else {
                    view.stride
                });
                self.alloc(Value::Tuple(vec![value]))
            }
            "suboffsets" => self.alloc(Value::Tuple(vec![])),
            _ => unreachable!(),
        };
        Ok(Some(value))
    }

    fn view_index(&mut self, view_id: Id, key: Id) -> Result<usize> {
        let index = self
            .index_result(key)?
            .ok_or_else(|| Error::new("TypeError", "memoryview: invalid slice key"))?
            .1;
        let index = index.to_i64().ok_or_else(|| {
            Error::new("IndexError", "cannot fit 'int' into an index-sized integer")
        })?;
        let view = self.byte_view(view_id)?;
        let len = view.len as i64;
        let index = if index < 0 { index + len } else { index };
        if !(0..len).contains(&index) {
            return Err(Error::new(
                "IndexError",
                "index out of bounds on dimension 1",
            ));
        }
        Ok(view.offset(index as usize))
    }

    pub(super) fn view_get_item(&mut self, id: Id, key: Id) -> Result<Id> {
        let view = self.byte_view(id)?;
        if let Value::Slice(parts) = *self.heap.get(key) {
            // CPython creates the child before evaluating slice bounds. An index hook
            // may release its parent and run a collection during that conversion.
            let child = self.alloc(Value::MemoryView(Some(view)));
            self.heap.keep(child);
            let bounds = slice::sequence_components(self, parts)?;
            *self.heap.get_mut(child) =
                Value::MemoryView(Some(self.byte_view(child)?.select(bounds)));
            return Ok(child);
        }
        let pos = self.view_index(id, key)?;
        let view = self.byte_view(id)?;
        Ok(self.int(i64::from(self.view_storage(view)[pos])))
    }

    pub(super) fn view_put_item(&mut self, id: Id, key: Id, value: Id) -> Result<()> {
        if self.byte_view(id)?.readonly {
            return Err(Error::new("TypeError", "cannot modify read-only memory"));
        }
        if let Value::Slice(parts) = *self.heap.get(key) {
            let source = self.scalar_value(value);
            match self.heap.get(source) {
                Value::Bytes(_) | Value::ByteArray(_) => {}
                Value::MemoryView(_) => {
                    self.byte_view(source)?;
                }
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            self.type_name(value)
                        ),
                    ));
                }
            }
            self.with_buffer_export(source, |ctx| {
                let bounds = slice::sequence_components(ctx, parts)?;
                let dest = ctx.byte_view(id)?.select(bounds);
                let input = match ctx.heap.get(source) {
                    Value::MemoryView(_) => ctx.view_snapshot(source)?,
                    _ => super::copy_ids(ctx.bytes_like(source)?)?,
                };
                if input.len() != dest.len {
                    return Err(Error::new(
                        "ValueError",
                        "memoryview assignment: lvalue and rvalue have different structures",
                    ));
                }
                let Value::ByteArray(bytes) = ctx.heap.get_mut(dest.backing) else {
                    unreachable!()
                };
                for (i, byte) in input.into_iter().enumerate() {
                    bytes[dest.offset(i)] = byte;
                }
                Ok(())
            })?;
        } else {
            let pos = self.view_index(id, key)?;
            let byte = self
                .index_result(value)?
                .ok_or_else(|| Error::new("TypeError", "memoryview: invalid type for format 'B'"))?
                .1
                .to_i64()
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| {
                    Error::new("ValueError", "memoryview: invalid value for format 'B'")
                })?;
            let view = self.byte_view(id)?;
            let Value::ByteArray(bytes) = self.heap.get_mut(view.backing) else {
                unreachable!()
            };
            bytes[pos] = byte;
        }
        Ok(())
    }

    pub(super) fn clone_byte_view(&mut self, input: Id) -> Result<Id> {
        let view = match self.heap.get(self.scalar_value(input)) {
            Value::MemoryView(_) => self.byte_view(input)?,
            Value::Bytes(data) => View {
                backing: self.scalar_value(input),
                exporter: input,
                backing_len: data.len(),
                start: 0,
                stride: 1,
                len: data.len(),
                readonly: true,
            },
            Value::ByteArray(_) => {
                return Err(Error::new(
                    "NotImplementedError",
                    "bytearray memoryview exports are not supported",
                ));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "memoryview: a bytes-like object is required, not '{}'",
                        self.type_name(input)
                    ),
                ));
            }
        };
        Ok(self.alloc(Value::MemoryView(Some(view))))
    }

    pub(super) fn view_method(&mut self, name: &str, args: &[Id]) -> Result<Id> {
        let Some((&id, rest)) = args.split_first() else {
            return Err(Error::new("TypeError", "expected a memoryview"));
        };
        if !matches!(self.heap.get(id), Value::MemoryView(_)) {
            return Err(Error::new("TypeError", "expected a memoryview"));
        }
        let expected = if name == "__exit__" { 3 } else { 0 };
        if rest.len() != expected {
            return Err(Error::new(
                "TypeError",
                if expected == 0 {
                    format!(
                        "memoryview.{name}() takes no arguments ({} given)",
                        rest.len()
                    )
                } else {
                    format!(
                        "memoryview.{name}() takes exactly 3 arguments ({} given)",
                        rest.len()
                    )
                },
            ));
        }
        match name {
            "release" | "__exit__" => {
                let exports = self.bytearray_exports.iter().filter(|&&v| v == id).count();
                if exports != 0 {
                    return Err(Error::new(
                        "BufferError",
                        format!(
                            "memoryview has {exports} exported buffer{}",
                            if exports == 1 { "" } else { "s" }
                        ),
                    ));
                }
                *self.heap.get_mut(id) = Value::MemoryView(None);
                Ok(NONE)
            }
            "__enter__" => {
                self.byte_view(id)?;
                Ok(id)
            }
            "tobytes" => {
                let data = self.view_snapshot(id)?;
                Ok(self.alloc(Value::Bytes(data)))
            }
            _ => unreachable!(),
        }
    }
}
