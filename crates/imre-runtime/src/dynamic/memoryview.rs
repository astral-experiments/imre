//! One-dimensional native memoryview indexing and assignment.
use super::{Context, Error, Id, Result, Value, builtins, slice};
use num_traits::ToPrimitive;

#[derive(Clone, Copy)]
pub(super) struct ViewLayout {
    pub(super) start: usize,
    pub(super) len: usize,
    pub(super) stride: i64,
    pub(super) byte_format: Option<&'static str>,
}

impl ViewLayout {
    pub(super) fn full(len: usize) -> Self {
        Self {
            start: 0,
            len,
            stride: 1,
            byte_format: None,
        }
    }

    pub(super) fn is_contiguous(self) -> bool {
        self.len == 1 || self.stride == 1
    }

    fn position(self, index: usize) -> i128 {
        self.start as i128 + index as i128 * i128::from(self.stride)
    }

    // The first and last element bound every address in a one-dimensional view.
    fn checked(self, elements: usize) -> Result<Self> {
        if self.len > elements
            || self.start > elements
            || (self.len != 0
                && (self.start == elements
                    || !(0..elements as i128).contains(&self.position(self.len - 1))))
        {
            return Err(Error::new(
                "BufferError",
                "underlying buffer has changed size",
            ));
        }
        Ok(self)
    }

    fn element(self, index: usize) -> usize {
        self.position(index) as usize
    }

    fn sliced(self, bounds: [Option<i64>; 3]) -> Self {
        let [start, stop, step] = slice::normalize(bounds, self.len as i64);
        // PySlice_Unpack limits the absolute step to PY_SSIZE_T_MAX.
        let step = step.max(-i64::MAX);
        let distance = if step > 0 { stop - start } else { start - stop };
        let len = (distance.max(0) as u64).div_ceil(step.unsigned_abs()) as usize;
        // A stride too large to address another element is still reported by CPython
        // using signed Py_ssize_t arithmetic on singleton and empty views.
        let stride = self.stride.wrapping_mul(step);
        Self {
            start: if len == 0 {
                0
            } else {
                self.element(start as usize)
            },
            len,
            stride,
            byte_format: self.byte_format,
        }
    }
}

struct Span<'a> {
    data: &'a [u8],
    array: Option<&'a super::array::Array>,
    layout: ViewLayout,
    format: &'static str,
    width: usize,
}

impl<'a> Span<'a> {
    fn element(&self, index: usize) -> &[u8] {
        let offset = self.layout.element(index) * self.width;
        &self.data[offset..offset + self.width]
    }

    fn contiguous(&self) -> Result<&'a [u8]> {
        if !self.layout.is_contiguous() {
            return Err(Error::new(
                "BufferError",
                "memoryview: underlying buffer is not C-contiguous",
            ));
        }
        let begin = self.layout.start * self.width;
        Ok(&self.data[begin..begin + self.layout.len * self.width])
    }

    fn gather(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.layout.len * self.width)
            .map_err(|_| Error::new("MemoryError", ""))?;
        if let Ok(data) = self.contiguous() {
            bytes.extend_from_slice(data);
        } else {
            for i in 0..self.layout.len {
                bytes.extend_from_slice(self.element(i));
            }
        }
        Ok(bytes)
    }

    fn item(&self, index: usize) -> Option<Value> {
        if let Some(array) = self.array {
            array.at(self.layout.element(index)).ok()
        } else {
            Some(Value::Int(i64::from(self.element(index)[0]).into()))
        }
    }
}

fn structure_error() -> Error {
    Error::new(
        "ValueError",
        "memoryview assignment: lvalue and rvalue have different structures",
    )
}

impl Context {
    fn view_owner(&self, view: Id) -> Result<Id> {
        let Value::MemoryView { owner, .. } = *self.heap.get(view) else {
            unreachable!()
        };
        Self::memoryview_owner(owner)
    }

    pub(super) fn view_layout(&self, view: Id) -> Result<ViewLayout> {
        let Value::MemoryView { owner, layout, .. } = *self.heap.get(view) else {
            unreachable!()
        };
        let owner = Self::memoryview_owner(owner)?;
        let layout = match layout {
            Some(layout) => layout,
            None => ViewLayout::full(self.view_element_count(owner)?),
        };
        let (_, width) = self.view_format_width(owner, layout);
        layout.checked(self.owner_bytes(owner)?.len() / width)
    }

    pub(super) fn view_format_width(&self, owner: Id, layout: ViewLayout) -> (&'static str, usize) {
        if let Some(format) = layout.byte_format {
            return (format, 1);
        }
        if !matches!(self.heap.get(owner), Value::RawReadWindow(_))
            && let Value::Array(a) = self.heap.get(self.view_storage_id(owner))
        {
            return a.view_layout();
        }
        ("B", 1)
    }

    pub(super) fn unsigned_assignment_bytes(&self, value: Id, len: usize) -> Result<Vec<u8>> {
        let span = self.buffer_span(value)?;
        if span.format.trim_start_matches('@') != "B" || span.layout.len != len {
            return Err(structure_error());
        }
        span.gather()
    }

    fn buffer_span(&self, id: Id) -> Result<Span<'_>> {
        let (owner, layout) = if matches!(self.heap.get(id), Value::MemoryView { .. }) {
            (self.view_owner(id)?, self.view_layout(id)?)
        } else {
            (id, ViewLayout::full(self.view_element_count(id)?))
        };
        let (format, width) = self.view_format_width(owner, layout);
        Ok(Span {
            data: self.owner_bytes(owner)?,
            array: match self.heap.get(self.view_storage_id(owner)) {
                Value::Array(array) if layout.byte_format.is_none() => Some(array),
                _ => None,
            },
            layout,
            format,
            width,
        })
    }

    pub(super) fn cast_memoryview(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&view, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "unbound method memoryview.cast() needs an argument",
            ));
        };
        if !matches!(self.heap.get(view), Value::MemoryView { .. }) {
            return Err(Error::new("TypeError", "expected memoryview"));
        }
        let [format, shape] =
            builtins::bind_builtin_arguments("cast", rest, kwargs, ["format", "shape"], 1)?;
        let format = format.expect("required format");
        let name = self.str_value(format).map_err(|_| {
            Error::new(
                "TypeError",
                format!(
                    "cast() argument 'format' must be str, not {}",
                    self.type_name(format)
                ),
            )
        })?;
        let span = self.buffer_span(view)?;
        if !span.layout.is_contiguous() {
            return Err(Error::new(
                "TypeError",
                "memoryview: casts are restricted to C-contiguous views",
            ));
        }
        let byte_format = match name {
            "B" => "B",
            "@B" => "@B",
            _ => {
                let native = name.strip_prefix('@').unwrap_or(name);
                if native.len() != 1 || !"cb?hHiIlLqQnNefdP".contains(native) {
                    return Err(Error::new(
                        "ValueError",
                        "memoryview: destination format must be a native single character format prefixed with an optional '@'",
                    ));
                }
                if !matches!(span.format.trim_start_matches('@'), "B" | "b" | "c")
                    && !matches!(native, "B" | "b" | "c")
                {
                    return Err(Error::new(
                        "TypeError",
                        "memoryview: cannot cast between two non-byte formats",
                    ));
                }
                return Err(Error::new(
                    "NotImplementedError",
                    "only casts to unsigned bytes are supported",
                ));
            }
        };
        let len = span.layout.len * span.width;
        if let Some(shape) = shape {
            let (Value::List(dims) | Value::Tuple(dims)) = self.heap.get(self.scalar_value(shape))
            else {
                return Err(Error::new("TypeError", "shape must be a list or a tuple"));
            };
            if len == 0 {
                return Err(Error::new(
                    "TypeError",
                    "memoryview: cannot cast view with zeros in shape or strides",
                ));
            }
            let [size] = dims.as_slice() else {
                return Err(Error::new(
                    "NotImplementedError",
                    "only one-dimensional casts are supported",
                ));
            };
            let size = self.as_int(*size).map_err(|_| {
                Error::new(
                    "TypeError",
                    "memoryview.cast(): elements of shape must be integers",
                )
            })?;
            let size = size.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })?;
            if size <= 0 {
                return Err(Error::new(
                    "ValueError",
                    "memoryview.cast(): elements of shape must be integers > 0",
                ));
            }
            if size != len as i64 {
                return Err(Error::new(
                    "TypeError",
                    "memoryview: product(shape) * itemsize != buffer size",
                ));
            }
        }
        let cast = ViewLayout {
            start: span.layout.start * span.width,
            byte_format: Some(byte_format),
            ..ViewLayout::full(len)
        };
        let child = self.new_memoryview(view)?;
        if let Value::MemoryView { layout, .. } = self.heap.get_mut(child) {
            *layout = Some(cast);
        }
        Ok(child)
    }

    pub(super) fn view_bytes(&self, view: Id) -> Result<&[u8]> {
        self.buffer_span(view)?.contiguous()
    }

    pub(super) fn memoryview_bytes(&self, view: Id) -> Result<Vec<u8>> {
        self.buffer_span(view)?.gather()
    }

    pub(super) fn memoryview_equal(&self, a: Id, b: Id) -> bool {
        let (Ok(left), Ok(right)) = (self.buffer_span(a), self.buffer_span(b)) else {
            return a == b;
        };
        if left.layout.len != right.layout.len {
            return false;
        }
        if left.format == "B"
            && right.format == "B"
            && let (Ok(x), Ok(y)) = (left.contiguous(), right.contiguous())
        {
            return x == y;
        }
        (0..left.layout.len).all(|i| match (left.item(i), right.item(i)) {
            (Some(Value::Int(x)), Some(Value::Int(y))) => x.cmp(&y).is_eq(),
            (Some(Value::Float(x)), Some(Value::Float(y))) => x == y,
            (Some(Value::Int(n)), Some(Value::Float(f)))
            | (Some(Value::Float(f)), Some(Value::Int(n))) => crate::floats::compare(&n, f) == 0,
            _ => false,
        })
    }

    fn view_index(&mut self, id: Id, key: Id, assign: bool) -> Result<usize> {
        let key = if let Value::Tuple(items) = self.heap.get(key) {
            if items.is_empty() {
                return Err(Error::new(
                    "NotImplementedError",
                    "sub-views are not implemented",
                ));
            }
            if items
                .iter()
                .all(|&v| matches!(self.heap.get(v), Value::Slice(_)))
            {
                let text = if assign {
                    "memoryview slice assignments are currently restricted to ndim = 1"
                } else {
                    "multi-dimensional slicing is not implemented"
                };
                return Err(Error::new("NotImplementedError", text));
            }
            if items.iter().any(|&id| {
                !matches!(self.heap.get(self.scalar_value(id)), Value::Int(_) | Value::Bool(_))
                    && !matches!(self.heap.get(id), Value::Instance {class, ..} if self.class_attr(*class, "__index__").is_some())
            }) {
                return Err(Error::new("TypeError", "memoryview: invalid slice key"));
            }
            if items.len() != 1 {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "cannot index 1-dimension view with {}-element tuple",
                        items.len()
                    ),
                ));
            }
            items[0]
        } else {
            key
        };
        let index = self
            .index_value(key)?
            .ok_or_else(|| Error::new("TypeError", "memoryview: invalid slice key"))?;
        let index = index.to_i64().ok_or_else(|| {
            Error::new(
                "IndexError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    self.type_name(key)
                ),
            )
        })?;
        let layout = self.view_layout(id)?;
        let index = if index < 0 {
            index + layout.len as i64
        } else {
            index
        };
        if index < 0 || index as usize >= layout.len {
            return Err(Error::new(
                "IndexError",
                "index out of bounds on dimension 1",
            ));
        }
        Ok(layout.element(index as usize))
    }

    pub(super) fn memoryview_get(&mut self, view: Id, key: Id) -> Result<Id> {
        let owner = self.view_owner(view)?;
        if let Value::Slice(parts) = *self.heap.get(key) {
            let layout = self.view_layout(view)?;
            let Value::MemoryView {
                anonymous,
                readonly,
                ..
            } = *self.heap.get(view)
            else {
                unreachable!()
            };
            // CPython acquires the child before invoking user-defined slice indices.
            self.heap.reserve_temporaries(1)?;
            let child = self.alloc(Value::MemoryView {
                owner: Some(owner),
                anonymous,
                readonly,
                hash: None,
                layout: Some(layout),
            });
            let mark = self.heap.temporary_mark();
            self.heap.keep(child);
            let sliced = slice::sequence_components(self, parts)
                .and_then(|bounds| Ok(self.view_layout(child)?.sliced(bounds)));
            self.heap.release_temporaries_since(mark);
            let Value::MemoryView { layout, .. } = self.heap.get_mut(child) else {
                unreachable!()
            };
            *layout = Some(sliced?);
            return Ok(child);
        }
        let index = self.view_index(view, key, false)?;
        let owner = self.view_owner(view)?;
        if self.view_layout(view)?.byte_format.is_none()
            && let Value::Array(array) = self.heap.get(self.view_storage_id(owner))
        {
            let item = array.at(index)?;
            Ok(self.alloc(item))
        } else {
            let value = self.owner_bytes(owner)?[index];
            Ok(self.int(value.into()))
        }
    }

    pub(super) fn memoryview_item_method(
        &mut self,
        method: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some((&view, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '{method}' of 'memoryview' object needs an argument"),
            ));
        };
        if !matches!(self.heap.get(view), Value::MemoryView { .. }) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' requires a 'memoryview' object but received a '{}'",
                    self.type_name(view)
                ),
            ));
        }
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("wrapper {method}() takes no keyword arguments"),
            ));
        }
        match (method, rest) {
            ("__getitem__", [key]) => self.get_item(view, *key),
            ("__delitem__", [key]) => self.delete_item(view, *key).map(|()| super::value::NONE),
            ("__setitem__", [key, value]) => self
                .put_item(view, *key, *value)
                .map(|()| super::value::NONE),
            ("__setitem__", _) => Err(Error::new(
                "TypeError",
                format!("__setitem__ expected 2 arguments, got {}", rest.len()),
            )),
            _ => Err(Error::new(
                "TypeError",
                format!("expected 1 argument, got {}", rest.len()),
            )),
        }
    }

    pub(super) fn memoryview_delete(&self, view: Id) -> Result<()> {
        let msg = if self.view_is_readonly(view)? {
            "cannot modify read-only memory"
        } else {
            "cannot delete memory"
        };
        Err(Error::new("TypeError", msg))
    }

    fn packed_view_value(&mut self, format: &str, value: Id) -> Result<Vec<u8>> {
        let invalid_type = || {
            Error::new(
                "TypeError",
                format!("memoryview: invalid type for format '{format}'"),
            )
        };
        let invalid_value = || {
            Error::new(
                "ValueError",
                format!("memoryview: invalid value for format '{format}'"),
            )
        };
        if matches!(format, "f" | "d") {
            let f = builtins::from_numeric(self, value)?.ok_or_else(invalid_type)?;
            let Value::Float(x) = *self.heap.get(f) else {
                unreachable!()
            };
            return Ok(if format == "f" {
                (x as f32).to_ne_bytes().to_vec()
            } else {
                x.to_ne_bytes().to_vec()
            });
        }
        let n = match self.index_value(value) {
            Ok(Some(n)) => n,
            Ok(None) => return Err(invalid_type()),
            Err(err) => {
                // CPython's fix_error_int also translates exceptions raised by __index__.
                let type_error = self.builtin("TypeError");
                if self.raised_error_matches(&err, type_error) {
                    return Err(invalid_type());
                }
                let overflow = self.builtin("OverflowError");
                let value_error = self.builtin("ValueError");
                if self.raised_error_matches(&err, overflow)
                    || self.raised_error_matches(&err, value_error)
                {
                    return Err(invalid_value());
                }
                return Err(err);
            }
        };
        Ok(match format {
            "B" | "@B" => vec![n.big().to_u8().ok_or_else(invalid_value)?],
            "b" => vec![n.big().to_i8().ok_or_else(invalid_value)? as u8],
            "q" => n.to_i64().ok_or_else(invalid_value)?.to_ne_bytes().to_vec(),
            "i" => n
                .big()
                .to_i32()
                .ok_or_else(invalid_value)?
                .to_ne_bytes()
                .to_vec(),
            _ => {
                return Err(Error::new(
                    "NotImplementedError",
                    format!("memoryview: unsupported format {format}"),
                ));
            }
        })
    }

    pub(super) fn memoryview_set(&mut self, view: Id, key: Id, value: Id) -> Result<()> {
        let owner = self.view_owner(view)?;
        if self.view_is_readonly(view)? {
            return Err(Error::new("TypeError", "cannot modify read-only memory"));
        }
        let (format, width) = self.view_format_width(owner, self.view_layout(view)?);
        let (dest, source) = if let Value::Slice(parts) = *self.heap.get(key) {
            self.check_buffer_exporter(value)?;
            // CPython acquires the right-hand buffer before interpreting the slice, but
            // reads its contents after index conversions, which can mutate that buffer.
            self.buffer_span(value)?;
            self.with_buffer_export(value, |ctx| {
                let bounds = slice::sequence_components(ctx, parts)?;
                let dest = ctx.view_layout(view)?.sliced(bounds);
                let rhs = ctx.buffer_span(value)?;
                if rhs.format.trim_start_matches('@') != format.trim_start_matches('@')
                    || rhs.layout.len != dest.len
                {
                    return Err(structure_error());
                }
                // Take a copy before assignment because the two views can overlap.
                Ok((dest, rhs.gather()?))
            })?
        } else {
            let index = self.view_index(view, key, true)?;
            let bytes = self.packed_view_value(format, value)?;
            self.view_owner(view)?;
            (
                ViewLayout {
                    start: index,
                    ..ViewLayout::full(1)
                },
                bytes,
            )
        };
        let storage = self.view_storage_id(owner);
        let bytes = self.writable_storage_bytes(storage)?;
        dest.checked(bytes.len() / width)?;
        for (i, item) in source.chunks_exact(width).enumerate() {
            let start = dest.element(i) * width;
            bytes[start..start + width].copy_from_slice(item);
        }
        Ok(())
    }
}
