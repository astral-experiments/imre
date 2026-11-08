//! Native memoryview indexing, casting and assignment.
use super::{Context, Error, Id, Result, Value, builtins, slice};
use num_traits::ToPrimitive;
use std::{borrow::Cow, ops::Range};

mod shape;
pub(super) use shape::ViewLayout;

pub(super) enum Storage<'a> {
    Native(&'a [u8]),
    #[cfg(target_os = "linux")]
    Mapped(&'a super::stdlib::mmap::Region, Range<usize>),
}

struct Span<'a> {
    data: Storage<'a>,
    array: Option<&'a super::array::Array>,
    layout: ViewLayout,
    format: &'static str,
    width: usize,
}

impl<'a> Storage<'a> {
    /// Append exactly the requested bytes, reserving the destination before reading a live map.
    pub(super) fn append_buffered(
        &self,
        buffer: &mut Vec<u8>,
        start: usize,
        count: usize,
    ) -> Result<()> {
        if start > self.len() || count > self.len() - start {
            return Err(Error::new("BufferError", "exported buffer changed size"));
        }
        buffer
            .try_reserve_exact(count)
            .map_err(|_| crate::Error::memory_error())?;
        match self {
            Self::Native(bytes) => buffer.extend_from_slice(&bytes[start..start + count]),
            #[cfg(target_os = "linux")]
            Self::Mapped(region, span) => {
                let old_len = buffer.len();
                buffer.resize(old_len + count, 0);
                if let Err(error) = region.read_into(span.start + start, &mut buffer[old_len..]) {
                    buffer.truncate(old_len);
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Native(bytes) => bytes.len(),
            #[cfg(target_os = "linux")]
            Self::Mapped(_, span) => span.len(),
        }
    }

    pub(super) fn read(&self, range: Range<usize>) -> Result<Cow<'a, [u8]>> {
        if range.end > self.len() || range.start > range.end {
            return Err(Error::new("BufferError", "exported buffer changed size"));
        }
        match self {
            Self::Native(bytes) => Ok(Cow::Borrowed(&bytes[range])),
            #[cfg(target_os = "linux")]
            Self::Mapped(map, span) => {
                super::stdlib::mmap::read(map, span.start + range.start..span.start + range.end)
                    .map(Cow::Owned)
            }
        }
    }
}

impl Storage<'_> {
    pub(in crate::dynamic) fn read_ptr(&self) -> Result<*const u8> {
        match self {
            Self::Native(bytes) => Ok(bytes.as_ptr()),
            #[cfg(target_os = "linux")]
            Self::Mapped(map, span) => map.readable_ptr(span.start, span.len()),
        }
    }

    /// Search two bounded exports without borrowing externally mutable mapped bytes as a Rust slice.
    #[cfg(target_os = "linux")]
    pub(super) fn find(&self, needle: &Storage<'_>, reverse: bool) -> Result<Option<usize>> {
        unsafe extern "C" {
            fn imre_mmap_find(
                data: *const u8,
                len: usize,
                needle: *const u8,
                needle_len: usize,
                reverse: bool,
            ) -> isize;
        }
        // SAFETY: both views stay live, and the search reads only within their checked lengths.
        let index = unsafe {
            imre_mmap_find(
                self.read_ptr()?,
                self.len(),
                needle.read_ptr()?,
                needle.len(),
                reverse,
            )
        };
        Ok(usize::try_from(index).ok())
    }
}

impl<'a> Span<'a> {
    fn element(&self, index: usize) -> Result<Cow<'a, [u8]>> {
        let offset = self.layout.byte_at(index, self.width);
        self.data.read(offset..offset + self.width)
    }

    fn contiguous_range(&self) -> Result<Range<usize>> {
        if !self.layout.is_contiguous() {
            return Err(Error::new(
                "BufferError",
                "memoryview: underlying buffer is not C-contiguous",
            ));
        }
        let begin = self.layout.byte_start(self.width);
        Ok(begin..begin + self.layout.len * self.width)
    }

    fn contiguous(&self) -> Result<Cow<'a, [u8]>> {
        self.data.read(self.contiguous_range()?)
    }

    fn borrowed_contiguous(&self) -> Result<&'a [u8]> {
        let range = self.contiguous_range()?;
        match &self.data {
            Storage::Native(bytes) => Ok(&bytes[range]),
            #[cfg(target_os = "linux")]
            Storage::Mapped(..) => Err(super::stdlib::mmap::unsupported_borrow()),
        }
    }

    fn gather(&self, fortran: bool) -> Result<Vec<u8>> {
        let reorder = fortran && self.layout.shape().len() > 1 && !self.layout.is_f_contiguous();
        if !reorder && self.layout.is_contiguous() {
            return match self.contiguous()? {
                Cow::Borrowed(bytes) => super::copy_ids(bytes),
                Cow::Owned(bytes) => Ok(bytes),
            };
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.layout.len * self.width)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for i in 0..self.layout.len {
            let at = if reorder {
                self.layout.fortran_index(i)
            } else {
                i
            };
            bytes.extend_from_slice(&self.element(at)?);
        }
        Ok(bytes)
    }

    fn item(&self, index: usize) -> Result<Value> {
        if let Some(array) = self.array {
            return array.at(self.layout.element(index));
        }
        let bytes = self.element(index)?;
        let number = match self.format {
            "I" | "@I" => i64::from(u32::from_ne_bytes(bytes.as_ref().try_into().unwrap())),
            _ => i64::from(bytes[0]),
        };
        Ok(Value::Int(number.into()))
    }
}

fn structure_error() -> Error {
    Error::new(
        "ValueError",
        "memoryview assignment: lvalue and rvalue have different structures",
    )
}

impl Context {
    pub(super) fn check_view_export(&self, view: Id) -> Result<()> {
        if let Value::MemoryView {
            owner,
            restricted: true,
            ..
        } = self.heap.get(view)
        {
            Self::memoryview_owner(*owner)?;
            return Err(Error::new(
                "ValueError",
                "cannot create new view on restricted memoryview",
            ));
        }
        Ok(())
    }

    fn view_owner(&self, view: Id) -> Result<Id> {
        let Value::MemoryView { owner, .. } = *self.heap.get(view) else {
            unreachable!()
        };
        Self::memoryview_owner(owner)
    }

    pub(super) fn view_layout(&self, view: Id) -> Result<ViewLayout> {
        let Value::MemoryView { owner, layout, .. } = self.heap.get(view) else {
            unreachable!()
        };
        let owner = Self::memoryview_owner(*owner)?;
        let layout = match layout {
            Some(layout) => layout.clone(),
            None => ViewLayout::full(self.view_element_count(owner)?),
        };
        let (_, width) = self.view_format_width(owner, &layout);
        layout.checked(self.owner_byte_len(owner)?, width)
    }

    pub(super) fn view_format_width(
        &self,
        owner: Id,
        layout: &ViewLayout,
    ) -> (&'static str, usize) {
        if let Some(format) = layout.byte_format {
            let width = if matches!(format, "I" | "@I") {
                size_of::<u32>()
            } else {
                1
            };
            return (format, width);
        }
        if !matches!(self.heap.get(owner), Value::RawReadWindow(_))
            && let Value::Array(a) = self.heap.get(self.view_storage_id(owner))
        {
            return a.view_layout();
        }
        ("B", 1)
    }

    fn buffer_span(&self, id: Id) -> Result<Span<'_>> {
        let (owner, layout) = if matches!(self.heap.get(id), Value::MemoryView { .. }) {
            (self.view_owner(id)?, self.view_layout(id)?)
        } else {
            (id, ViewLayout::full(self.view_element_count(id)?))
        };
        let (format, width) = self.view_format_width(owner, &layout);
        let native = || self.owner_bytes(owner).map(Storage::Native);
        #[cfg(target_os = "linux")]
        let data = match super::stdlib::mmap::owner(self, owner)? {
            Some((map, range)) => {
                Storage::Mapped(super::stdlib::mmap::region(self.heap.get(map))?, range)
            }
            None => native()?,
        };
        #[cfg(not(target_os = "linux"))]
        let data = native()?;
        Ok(Span {
            data,
            array: match self.heap.get(self.view_storage_id(owner)) {
                Value::Array(array)
                    if layout.byte_format.is_none()
                        && !matches!(self.heap.get(owner), Value::RawReadWindow(_)) =>
                {
                    Some(array)
                }
                _ => None,
            },
            layout,
            format,
            width,
        })
    }

    pub(super) fn memoryview_iterator(&mut self, view: Id) -> Result<Id> {
        let layout = self.view_layout(view)?;
        let length = match layout.shape() {
            [] => return Err(Error::new("TypeError", "invalid indexing of 0-dim memory")),
            [length] => *length,
            _ => {
                return Err(Error::new(
                    "NotImplementedError",
                    "multi-dimensional sub-views are not implemented",
                ));
            }
        };
        Ok(self.alloc(Value::MemoryIterator {
            view: Some(view),
            index: 0,
            length,
        }))
    }

    pub(super) fn next_memoryview_item(
        &mut self,
        iterator: Id,
        view: Option<Id>,
        index: usize,
        length: usize,
    ) -> Result<Option<Id>> {
        let Some(view) = view else {
            return Ok(None);
        };
        // Exhaustion uses the original length even if the view has since been released.
        if index == length {
            let Value::MemoryIterator { view: source, .. } = self.heap.get_mut(iterator) else {
                unreachable!()
            };
            *source = None;
            self.heap.weak_edge_removed(view);
            return Ok(None);
        }
        let item = self.buffer_span(view)?.item(index);
        let Value::MemoryIterator { index, .. } = self.heap.get_mut(iterator) else {
            unreachable!()
        };
        *index += 1;
        Ok(Some(self.alloc(item?)))
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
        self.check_view_export(view)?;
        let span = self.buffer_span(view)?;
        if !span.layout.is_contiguous() {
            return Err(Error::new(
                "TypeError",
                "memoryview: casts are restricted to C-contiguous views",
            ));
        }
        let rank = span.layout.shape().len();
        if (shape.is_some() || rank != 1) && span.layout.len == 0 {
            return Err(Error::new(
                "TypeError",
                "memoryview: cannot cast view with zeros in shape or strides",
            ));
        }
        let dims = if let Some(shape) = shape {
            let (Value::List(dims) | Value::Tuple(dims)) = self.heap.get(self.scalar_value(shape))
            else {
                return Err(Error::new("TypeError", "shape must be a list or a tuple"));
            };
            if dims.len() > 64 {
                return Err(Error::new(
                    "ValueError",
                    "memoryview: number of dimensions must not exceed 64",
                ));
            }
            if rank != 1 && dims.len() != 1 {
                return Err(Error::new(
                    "TypeError",
                    "memoryview: cast must be 1D -> ND or ND -> 1D",
                ));
            }
            Some(dims)
        } else {
            None
        };
        let native = name.strip_prefix('@').unwrap_or(name);
        if native.len() != 1 || !"cBb?hHiIlLqQnNefdP".contains(native) {
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
        let (format, width) = match name {
            "B" => ("B", 1),
            "@B" => ("@B", 1),
            "I" => ("I", size_of::<u32>()),
            "@I" => ("@I", size_of::<u32>()),
            _ => {
                return Err(Error::new(
                    "NotImplementedError",
                    "only casts to unsigned bytes and unsigned ints are supported",
                ));
            }
        };
        let nbytes = span.layout.len * span.width;
        if !nbytes.is_multiple_of(width) {
            return Err(Error::new(
                "TypeError",
                "memoryview: length is not a multiple of itemsize",
            ));
        }
        let len = nbytes / width;
        let shape = if let Some(dims) = dims {
            let mut shape = Vec::with_capacity(dims.len());
            let mut bytes = width;
            for &size in dims {
                let size = self.as_int(size).map_err(|_| {
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
                let size = size as usize;
                bytes = bytes
                    .checked_mul(size)
                    .filter(|&n| n <= isize::MAX as usize)
                    .ok_or_else(|| {
                        Error::new(
                            "ValueError",
                            "memoryview.cast(): product(shape) > SSIZE_MAX",
                        )
                    })?;
                shape.push(size);
            }
            if bytes != nbytes {
                return Err(Error::new(
                    "TypeError",
                    "memoryview: product(shape) * itemsize != buffer size",
                ));
            }
            Some(shape)
        } else {
            None
        };
        let cast = ViewLayout::cast(
            span.layout.byte_start(span.width),
            width,
            len,
            format,
            shape,
        );
        let child = self.new_memoryview(view)?;
        if let Value::MemoryView { layout, .. } = self.heap.get_mut(child) {
            *layout = Some(cast);
        }
        Ok(child)
    }

    /// Acquire a contiguous source before checking the exporter’s element size.
    pub(super) fn byte_sized_buffer(&self, input: Id) -> Result<&[u8]> {
        let span = self.buffer_span(input)?;
        let data = span.borrowed_contiguous()?;
        if span.width != 1 {
            return Err(Error::new("TypeError", "a bytes-like object is required"));
        }
        Ok(data)
    }

    pub(super) fn view_bytes(&self, view: Id) -> Result<&[u8]> {
        self.check_view_export(view)?;
        self.buffer_span(view)?.borrowed_contiguous()
    }

    pub(super) fn memoryview_bytes(&self, view: Id) -> Result<Vec<u8>> {
        self.memoryview_bytes_order(view, false)
    }

    pub(super) fn memoryview_bytes_order(&self, view: Id, fortran: bool) -> Result<Vec<u8>> {
        self.buffer_span(view)?.gather(fortran)
    }

    pub(super) fn memoryview_equal(&self, a: Id, b: Id) -> bool {
        let (Ok(left), Ok(right)) = (self.buffer_span(a), self.buffer_span(b)) else {
            return a == b;
        };
        if !left.layout.same_shape(&right.layout) {
            return false;
        }
        if left.format == "B"
            && right.format == "B"
            && let (Ok(x), Ok(y)) = (left.contiguous(), right.contiguous())
        {
            return x == y;
        }
        (0..left.layout.len).all(|i| match (left.item(i).ok(), right.item(i).ok()) {
            (Some(Value::Int(x)), Some(Value::Int(y))) => x.cmp(&y).is_eq(),
            (Some(Value::Float(x)), Some(Value::Float(y))) => x == y,
            (Some(Value::Int(n)), Some(Value::Float(f)))
            | (Some(Value::Float(f)), Some(Value::Int(n))) => crate::floats::compare(&n, f) == 0,
            _ => false,
        })
    }

    fn view_index(&mut self, id: Id, key: Id, assign: bool) -> Result<usize> {
        let layout = self.view_layout(id)?;
        let shape = layout.shape();
        let rank = shape.len();
        if rank == 0 {
            return if key == super::value::ELLIPSIS
                || self.tuple_items(key)?.is_some_and(|items| items.is_empty())
            {
                Ok(0)
            } else {
                Err(Error::new("TypeError", "invalid indexing of 0-dim memory"))
            };
        }
        let scalar_index = self.is_view_index(key);
        if !scalar_index && let Some(items) = self.tuple_items(key)? {
            if !items.is_empty()
                && items
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
            if items.iter().any(|&id| !self.is_view_index(id)) {
                return Err(Error::new("TypeError", "memoryview: invalid slice key"));
            }
            if items.len() < rank {
                return Err(Error::new(
                    "NotImplementedError",
                    "sub-views are not implemented",
                ));
            }
            if items.len() > rank {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "cannot index {rank}-dimension view with {}-element tuple",
                        items.len()
                    ),
                ));
            }
            let mut flat = 0;
            for (axis, (key, &size)) in items.into_iter().zip(shape).enumerate() {
                flat = flat * size + self.view_coordinate(key, size, axis)?;
            }
            return Ok(flat);
        }
        if rank != 1 {
            if !scalar_index {
                return Err(Error::new("TypeError", "memoryview: invalid slice key"));
            }
            if !assign {
                self.view_subscript_index(key)?;
            }
            let message = if assign {
                "sub-views are not implemented"
            } else {
                "multi-dimensional sub-views are not implemented"
            };
            return Err(Error::new("NotImplementedError", message));
        }
        self.view_coordinate(key, shape[0], 0)
    }

    fn is_view_index(&self, key: Id) -> bool {
        matches!(
            self.heap.get(self.scalar_value(key)),
            Value::Int(_) | Value::Bool(_)
        ) || matches!(self.heap.get(key), Value::Instance {class, ..} if self.class_attr(*class, "__index__").is_some())
    }

    fn view_subscript_index(&mut self, key: Id) -> Result<i64> {
        self.index_value(key)?
            .ok_or_else(|| Error::new("TypeError", "memoryview: invalid slice key"))?
            .to_i64()
            .ok_or_else(|| {
                Error::new(
                    "IndexError",
                    format!(
                        "cannot fit '{}' into an index-sized integer",
                        self.type_name(key)
                    ),
                )
            })
    }

    fn view_coordinate(&mut self, key: Id, size: usize, axis: usize) -> Result<usize> {
        let index = self.view_subscript_index(key)?;
        let index = if index < 0 {
            index + size as i64
        } else {
            index
        };
        if index < 0 || index as usize >= size {
            return Err(Error::new(
                "IndexError",
                format!("index out of bounds on dimension {}", axis + 1),
            ));
        }
        Ok(index as usize)
    }

    pub(super) fn memoryview_get(&mut self, view: Id, key: Id) -> Result<Id> {
        let owner = self.view_owner(view)?;
        if key == super::value::ELLIPSIS && self.view_layout(view)?.shape().is_empty() {
            return Ok(view);
        }
        if let Value::Slice(parts) = *self.heap.get(key) {
            self.check_view_export(view)?;
            let layout = self.view_layout(view)?;
            if layout.shape().is_empty() {
                return Err(Error::new("TypeError", "invalid indexing of 0-dim memory"));
            }
            let Value::MemoryView {
                anonymous,
                readonly,
                lease,
                ..
            } = *self.heap.get(view)
            else {
                unreachable!()
            };
            // CPython acquires the child before invoking user-defined slice indices.
            self.heap.reserve_temporaries(1)?;
            let child = self.alloc(Value::MemoryView {
                owner: Some(owner),
                lease,
                anonymous,
                restricted: false,
                readonly,
                hash: None,
                layout: Some(layout),
            });
            let mark = self.heap.temporary_mark();
            self.heap.keep(child);
            let sliced = slice::sequence_components(self, parts)
                .and_then(|bounds| self.view_layout(child)?.sliced(bounds));
            self.heap.release_temporaries_since(mark);
            let Value::MemoryView { layout, .. } = self.heap.get_mut(child) else {
                unreachable!()
            };
            *layout = Some(sliced?);
            return Ok(child);
        }
        let index = self.view_index(view, key, false)?;
        let item = self.buffer_span(view)?.item(index)?;
        Ok(self.alloc(item))
    }

    pub(super) fn buffer_item_method(
        &mut self,
        owner: &str,
        method: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '{method}' of '{owner}' object needs an argument"),
            ));
        };
        if !matches!(
            (owner, self.heap.get(receiver)),
            ("memoryview", Value::MemoryView { .. }) | ("array.array", Value::Array(_))
        ) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' requires a '{owner}' object but received a '{}'",
                    self.type_name(receiver)
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
            ("__getitem__", [key]) => self.get_item(receiver, *key),
            ("__delitem__", [key]) => self
                .delete_item(receiver, *key)
                .map(|()| super::value::NONE),
            ("__setitem__", [key, value]) => self
                .put_item(receiver, *key, *value)
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
        let format = format.strip_prefix('@').unwrap_or(format);
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
        // CPython's fix_error_int handles both floating-point and integer pack failures.
        let pack_error = |ctx: &mut Self, err: Error| {
            let type_error = ctx.builtin("TypeError");
            if ctx.raised_error_matches(&err, type_error) {
                return invalid_type();
            }
            let overflow = ctx.builtin("OverflowError");
            let value_error = ctx.builtin("ValueError");
            if ctx.raised_error_matches(&err, overflow)
                || ctx.raised_error_matches(&err, value_error)
            {
                return invalid_value();
            }
            err
        };
        if matches!(format, "f" | "d") {
            let f = builtins::from_numeric(self, value)
                .map_err(|err| pack_error(self, err))?
                .ok_or_else(invalid_type)?;
            let Value::Float(x) = *self.heap.get(f) else {
                unreachable!()
            };
            return Ok(if format == "f" {
                (x as f32).to_ne_bytes().to_vec()
            } else {
                x.to_ne_bytes().to_vec()
            });
        }
        let n = self
            .index_value(value)
            .map_err(|err| pack_error(self, err))?
            .ok_or_else(invalid_type)?;
        Ok(match format {
            "B" => vec![n.big().to_u8().ok_or_else(invalid_value)?],
            "I" => n
                .big()
                .to_u32()
                .ok_or_else(invalid_value)?
                .to_ne_bytes()
                .to_vec(),
            "b" => vec![n.big().to_i8().ok_or_else(invalid_value)? as u8],
            "q" => n.to_i64().ok_or_else(invalid_value)?.to_ne_bytes().to_vec(),
            "Q" => n
                .big()
                .to_u64()
                .ok_or_else(invalid_value)?
                .to_ne_bytes()
                .to_vec(),
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
        let layout = self.view_layout(view)?;
        let (format, width) = self.view_format_width(owner, &layout);
        let (dest, source) = if let Value::Slice(parts) = *self.heap.get(key) {
            match layout.shape().len() {
                0 => return Err(Error::new("TypeError", "invalid indexing of 0-dim memory")),
                1 => {}
                _ => {
                    return Err(Error::new(
                        "NotImplementedError",
                        "memoryview slice assignments are currently restricted to ndim = 1",
                    ));
                }
            }
            self.check_buffer_exporter(value)?;
            // CPython acquires the right-hand buffer before interpreting the slice, but
            // reads its contents after index conversions, which can mutate that buffer.
            self.buffer_span(value)?;
            self.with_buffer_export(value, |ctx| {
                let bounds = slice::sequence_components(ctx, parts)?;
                let dest = ctx.view_layout(view)?.sliced(bounds)?;
                let rhs = ctx.buffer_span(value)?;
                if rhs.format.trim_start_matches('@') != format.trim_start_matches('@')
                    || !rhs.layout.same_shape(&dest)
                {
                    return Err(structure_error());
                }
                // Take a copy before assignment because the two views can overlap.
                Ok((dest, rhs.gather(false)?))
            })?
        } else {
            let index = self.view_index(view, key, true)?;
            let bytes = self.packed_view_value(format, value)?;
            let layout = self.view_layout(view)?;
            (layout.scalar_at(index), bytes)
        };
        #[cfg(target_os = "linux")]
        if let Some((map, range)) = super::stdlib::mmap::owner(self, owner)? {
            let dest = dest.checked(range.len(), width)?;
            let map = super::stdlib::mmap::region(self.heap.get(map))?;
            if dest.is_contiguous() {
                return map.write_from(range.start + dest.byte_start(width), &source);
            }
            for (i, item) in source.chunks_exact(width).enumerate() {
                map.write_from(range.start + dest.byte_at(i, width), item)?;
            }
            return Ok(());
        }
        let bytes = self.writable_owner_bytes(owner)?;
        let dest = dest.checked(bytes.len(), width)?;
        for (i, item) in source.chunks_exact(width).enumerate() {
            let start = dest.byte_at(i, width);
            bytes[start..start + width].copy_from_slice(item);
        }
        Ok(())
    }
}
