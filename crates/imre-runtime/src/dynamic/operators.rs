use std::cmp::Ordering;

use num_traits::Signed;

use super::*;

/// An inherited numeric wrapper converts its argument as a direct method call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RepeatMode {
    Sequence,
    InPlace,
    Wrapper,
}

/// Quote one Python string, applying the ordinary repr rules to scalar values
/// and spelling each surrogate independently (including adjacent pairs).
pub(super) fn surrogate_repr(points: &[u32]) -> String {
    use std::fmt::Write;
    let quote = if points.contains(&u32::from('\'')) && !points.contains(&u32::from('"')) {
        '"'
    } else {
        '\''
    };
    let mut result = String::from(quote);
    for &point in points {
        match char::from_u32(point) {
            None => write!(result, "\\u{point:04x}").unwrap(),
            Some(ch) if ch == quote => {
                result.push('\\');
                result.push(ch);
            }
            Some(ch @ ('\'' | '"')) => result.push(ch),
            Some(ch) => {
                let quoted = crate::objects::exceptions::string_repr(ch.encode_utf8(&mut [0; 4]));
                result.push_str(&quoted[1..quoted.len() - 1]);
            }
        }
    }
    result.push(quote);
    result
}

/// Leave CPython's two native-stack margins available for exception handling.
pub(super) fn remaining_repr_words() -> Option<usize> {
    const STACK_MARGIN_WORDS: usize = 2048;
    stacker::remaining_stack()
        .map(|bytes| (bytes / size_of::<usize>()).saturating_sub(2 * STACK_MARGIN_WORDS))
}

fn render_allocation(_: std::collections::TryReserveError) -> Error {
    Error::new("MemoryError", "")
}

fn push_frame<T>(stack: &mut Vec<T>, value: T) -> Result<()> {
    stack.try_reserve(1).map_err(render_allocation)?;
    stack.push(value);
    Ok(())
}

fn enter_ancestor(seen: &mut std::collections::HashSet<Id>, id: Id) -> Result<bool> {
    if seen.contains(&id) {
        return Ok(false);
    }
    seen.try_reserve(1).map_err(render_allocation)?;
    Ok(seen.insert(id))
}

/// Spell a boxed integer in decimal with checked allocations and Python's digit limit.
pub(super) fn rendered_integer(value: &crate::Int) -> Result<crate::Str> {
    let limit = crate::integer_text::decimal_digit_limit();
    // Each decimal digit needs fewer than four bits. Reject very large inputs before converting.
    if limit == 0 || value.as_small().is_some() || value.big().bits() <= u64::from(limit) * 4 {
        let text = crate::integer_text::try_decimal_coefficient(value)
            .ok_or_else(|| Error::new("MemoryError", ""))?;
        if crate::integer_text::decimal_digits_allowed(
            text.len() - usize::from(text.starts_with('-')),
        ) {
            return Ok(text);
        }
    }
    let detail = crate::integer_text::output_limit_message(limit)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let mut error = Error::new("", detail.into_string());
    error
        .kind
        .to_mut()
        .try_reserve_exact("ValueError".len())
        .map_err(render_allocation)?;
    error.kind.to_mut().push_str("ValueError");
    Err(error)
}

pub(super) fn append_rendered_text(out: &mut String, text: &str) -> Result<()> {
    out.try_reserve(text.len()).map_err(render_allocation)?;
    out.push_str(text);
    Ok(())
}

fn close_parens(out: &mut String, count: usize) -> Result<()> {
    out.try_reserve(count).map_err(render_allocation)?;
    out.extend(std::iter::repeat_n(')', count));
    Ok(())
}

enum RenderRoot<'a> {
    Object(Id),
    Cursor(Id),
    BuiltinContainer {
        object: Id,
        storage: Id,
        with_factory: bool,
    },
    Exception(&'a Error),
}

#[derive(Clone, Copy)]
enum FormatLookup {
    Repr,
    Str,
    MethodName,
    ModuleRepr,
    DictViewList,
    TypingTuple(usize),
}

enum FormatPart {
    Value(Id, bool),
    Cursor(Id),
    CursorEnd,
    TypingArgument(Id),
    TypeArgument(Id, bool),
    TypeList(Id, usize, usize),
    TypeEnd(Id, &'static str, bool),
    Text(&'static str),
    SyntaxMessage(Id),
    SyntaxEnd(Id),
    Suffix(String),
    Closing(usize),
    Sequence(Id, usize),
    Entries(Id, usize, bool),
    DefaultDictFactory(Id, usize),
    DefaultDictName(Id, usize),
    DefaultDictEnd(String),
    SnapshotMembers(Vec<Id>, usize),
    Fields(Id, usize),
    Leave(Id, &'static str),
    ContextEnd(Id),
    SliceEnd,
    NamedTupleEnd,
}

impl FormatPart {
    /// Snapshot members before an element's repr can mutate the container.
    fn snapshot(items: impl ExactSizeIterator<Item = Id>) -> Result<Self> {
        let mut members = Vec::new();
        members
            .try_reserve_exact(items.len())
            .map_err(render_allocation)?;
        members.extend(items);
        Ok(Self::SnapshotMembers(members, 0))
    }

    /// Values a suspended renderer will read after a Python callback returns.
    fn keep_alive(&mut self, heap: &mut Heap) {
        match self {
            Self::Value(id, _)
            | Self::Cursor(id)
            | Self::TypingArgument(id)
            | Self::TypeArgument(id, _)
            | Self::TypeList(id, _, _)
            | Self::TypeEnd(id, _, _)
            | Self::SyntaxMessage(id)
            | Self::SyntaxEnd(id)
            | Self::Sequence(id, _)
            | Self::Entries(id, _, _)
            | Self::DefaultDictFactory(id, _)
            | Self::DefaultDictName(id, _)
            | Self::Fields(id, _)
            | Self::Leave(id, _)
            | Self::ContextEnd(id) => heap.keep(*id),
            Self::SnapshotMembers(members, index) => {
                // One traced owner keeps the snapshot alive across every following callback.
                let tuple = heap.alloc(Value::Tuple(std::mem::take(members)));
                *self = Self::Sequence(tuple, *index);
            }
            Self::Suffix(_)
            | Self::Text(_)
            | Self::Closing(_)
            | Self::CursorEnd
            | Self::SliceEnd
            | Self::NamedTupleEnd
            | Self::DefaultDictEnd(_) => {}
        }
    }
}

/// Count normalized positions without traversing the slice or negating a minimum stride.
pub(super) fn slice_count(start: i64, end: i64, step: i64) -> usize {
    let distance = if step > 0 {
        end.saturating_sub(start)
    } else {
        start.saturating_sub(end)
    };
    (distance.max(0) as u64).div_ceil(step.unsigned_abs()) as usize
}

pub(super) fn slice_positions(
    start: i64,
    end: i64,
    step: i64,
) -> impl Iterator<Item = usize> + Clone {
    std::iter::successors(Some(start), move |&i| i.checked_add(step))
        .take_while(move |&i| if step > 0 { i < end } else { i > end })
        .map(|i| i as usize)
}

fn remove_slice_positions<T>(target: &mut Vec<T>, start: i64, end: i64, step: i64) {
    let mut i = 0_i64;
    target.retain(|_| {
        let selected = (if step > 0 {
            start <= i && i < end
        } else {
            end < i && i <= start
        }) && i.abs_diff(start).is_multiple_of(step.unsigned_abs());
        i += 1;
        !selected
    });
}

fn slice_memory_error(_: std::collections::TryReserveError) -> Error {
    Error::new("MemoryError", "sequence slice is too large")
}

/// Copies only the elements selected by normalized slice bounds.
fn sliced_values<T: Copy>(
    source: &[T],
    positions: impl Iterator<Item = usize>,
    count: usize,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(slice_memory_error)?;
    out.extend(positions.map(|i| source[i]));
    Ok(out)
}

/// Collects the selected code points in output order without boxing the input string.
fn sliced_text(chars: impl Iterator<Item = char>, stride: usize, count: usize) -> Result<String> {
    let mut out = String::new();
    out.try_reserve_exact(count).map_err(slice_memory_error)?;
    for c in chars.step_by(stride).take(count) {
        out.try_reserve(c.len_utf8()).map_err(slice_memory_error)?;
        out.push(c);
    }
    Ok(out)
}

impl Context {
    /// Expose a tuple subclass's stored positions even when its length differs from the named fields.
    pub(super) fn tuple_backing(&self, value: Id) -> &Value {
        let object = self.heap.get(value);
        if let Value::Instance {
            scalar: Some(storage),
            ..
        } = *object
            && let tuple @ Value::Tuple(_) = self.heap.get(storage)
        {
            return tuple;
        }
        object
    }

    pub(super) fn sequence(&self, id: Id) -> Option<Vec<Id>> {
        match self.tuple_backing(id) {
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => {
                Some(v.clone())
            }
            _ => None,
        }
    }
    pub(super) fn sequence_at(&self, id: Id, index: usize) -> Option<(usize, Option<Id>)> {
        match self.tuple_backing(id) {
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => {
                Some((v.len(), v.get(index).copied()))
            }
            _ => None,
        }
    }

    pub(super) fn optional_truth(&mut self, value: Option<Id>, default: bool) -> Result<bool> {
        value.map_or(Ok(default), |id| self.truth(id))
    }

    pub(super) fn index_value(&mut self, value: Id) -> Result<Option<crate::Int>> {
        Ok(self.index_result(value)?.map(|(_, number)| number))
    }

    /// Preserve the integer object returned by the index protocol for type-sensitive errors.
    pub(super) fn index_result(&mut self, value: Id) -> Result<Option<(Id, crate::Int)>> {
        if let Ok(integer) = self.as_int(value) {
            return Ok(Some((value, integer)));
        }
        let target = self.weak_proxy_target(value)?;
        if target != value {
            return self.with_weak_target(target, |ctx| ctx.exact_index_result(target).map(Some));
        }
        let Some(index) = self.bound_slot(value, "__index__")? else {
            return Ok(None);
        };
        let result = self.call(index, Vec::new(), Vec::new())?;
        let number = self.integer_slot_result(result, "__index__")?;
        Ok(Some((result, number)))
    }

    /// Return an exact int for PyNumber_Index, preserving an already exact result.
    pub(super) fn exact_index_result(&mut self, value: Id) -> Result<(Id, crate::Int)> {
        let (index, number) = self.require_index_result(value)?;
        if matches!(self.heap.get(index), Value::Int(_)) {
            Ok((index, number))
        } else {
            Ok((self.alloc(Value::Int(number.clone())), number))
        }
    }

    fn integer_slot_result(&mut self, result: Id, slot: &str) -> Result<crate::Int> {
        let number = self.as_int(result).map_err(|_| {
            Error::new(
                "TypeError",
                format!("{slot} returned non-int (type {})", self.type_name(result)),
            )
        })?;
        if !matches!(self.heap.get(result), Value::Int(_)) {
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(result);
            self.emit_builtin_warning(
                crate::objects::exceptions::Kind::DeprecationWarning,
                &format!(
                    "{slot} returned non-int (type {}).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.",
                    self.type_name(result)
                ),
                1,
            )?;
        }
        Ok(number)
    }

    /// Apply the numeric conversion used by int(x) before attempting to parse text.
    pub(super) fn numeric_int(&mut self, value: Id) -> Result<Option<Id>> {
        if matches!(self.heap.get(value), Value::Int(_)) {
            return Ok(Some(value));
        }
        let (result, number) = if let Some(method) = self.bound_slot(value, "__int__")? {
            let result = self.call(method, Vec::new(), Vec::new())?;
            (result, self.integer_slot_result(result, "__int__")?)
        } else if let Value::Float(number) = *self.heap.get(self.scalar_value(value)) {
            let integer = crate::floats::float_to_integer(number)?;
            return Ok(Some(self.alloc(Value::Int(crate::Int::new(integer)))));
        } else if let Some(index) = self.index_result(value)? {
            index
        } else {
            return Ok(None);
        };
        Ok(Some(if matches!(self.heap.get(result), Value::Int(_)) {
            result
        } else {
            self.alloc(Value::Int(number))
        }))
    }

    /// Convert a bytearray element using the index protocol and the inclusive byte bounds.
    pub(super) fn byte_value(&mut self, value: Id) -> Result<u8> {
        self.index_integer(value)?
            .to_i64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| Error::new("ValueError", "byte must be in range(0, 256)"))
    }

    /// Resolve Python's integer-index protocol without rounding or truncating a numeric value.
    pub(super) fn index_integer(&mut self, value: Id) -> Result<crate::Int> {
        Ok(self.require_index_result(value)?.1)
    }

    /// Convert an index to the signed C long used by Python's native argument parser.
    pub(super) fn index_c_long(&mut self, value: Id) -> Result<i64> {
        self.index_integer(value)?
            .to_i64()
            .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C long"))
    }

    /// Convert an index to the signed C int used by Python's native argument parser.
    pub(super) fn index_c_int(&mut self, value: Id) -> Result<i32> {
        let long = self.index_c_long(value)?;
        i32::try_from(long).map_err(|_| {
            Error::new(
                "OverflowError",
                if long < 0 {
                    "signed integer is less than minimum"
                } else {
                    "signed integer is greater than maximum"
                },
            )
        })
    }

    /// Require an index and retain its Python type alongside its numeric value.
    fn require_index_result(&mut self, value: Id) -> Result<(Id, crate::Int)> {
        self.index_result(value)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    self.type_name(value)
                ),
            )
        })
    }

    pub(super) fn instance_len(&mut self, id: Id) -> Result<Option<usize>> {
        let Some(hook) = self.bound_slot(id, "__len__")? else {
            return Ok(match self.heap.get(self.scalar_value(id)) {
                Value::List(items) | Value::Tuple(items) => Some(items.len()),
                Value::Str(text) => Some(text.chars().count()),
                Value::SurrogateStr(points) => Some(points.len()),
                Value::Bytes(data) | Value::ByteArray(data) => Some(data.len()),
                _ => None,
            });
        };
        let value = self.call(hook, Vec::new(), Vec::new())?;
        let (index, length) = self.require_index_result(value)?;
        if length.cmp(&crate::Int::from(0)).is_lt() {
            return Err(Error::new("ValueError", "__len__() should return >= 0"));
        }
        let length = length
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    format!(
                        "cannot fit '{}' into an index-sized integer",
                        self.type_name(index)
                    ),
                )
            })?;
        Ok(Some(length as usize))
    }

    /// In nb_bool, an AttributeError during descriptor binding skips that slot.
    /// The fallback tests the returned length object itself (CPython slot_nb_bool).
    fn truth_after_missing_bool(&mut self, id: Id) -> Result<bool> {
        let hook = match self.bound_slot(id, "__len__") {
            Ok(Some(hook)) => hook,
            Ok(None) => return Ok(true),
            Err(error) if self.is_attribute_error(&error) => return Ok(true),
            Err(error) => return Err(error),
        };
        let value = self.call(hook, Vec::new(), Vec::new())?;
        self.with_slot_depth("maximum recursion depth exceeded", |ctx| ctx.truth(value))
    }

    pub(super) fn truth(&mut self, id: Id) -> Result<bool> {
        let id = self.weak_proxy_target(id)?;
        Ok(match self.heap.get(id) {
            Value::None => false,
            Value::NotImplemented => {
                return Err(Error::new(
                    "TypeError",
                    "NotImplemented should not be used in a boolean context",
                ));
            }
            Value::Bool(b) => *b,
            Value::Int(i) => i.as_small() != Some(0),
            Value::Float(f) => *f != 0.0,
            Value::Complex(z) => z.real != 0.0 || z.imag != 0.0,
            Value::Calendar { value, .. } => {
                value.class() != "timedelta" || value.delta_parts() != (0, 0, 0)
            }
            Value::Deque(ring) => !ring.items.is_empty(),
            Value::Str(s) => !s.is_empty(),
            Value::SurrogateStr(points) => !points.is_empty(),
            Value::Bytes(b) | Value::ByteArray(b) => !b.is_empty(),
            Value::MemoryView { .. } => self.view_layout(id)?.first_len()? != 0,
            #[cfg(target_os = "linux")]
            obj @ Value::Mmap { .. } => super::stdlib::mmap::region(obj)?.len() != 0,
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => !v.is_empty(),
            Value::Set { entries: v, .. }
            | Value::Dict(v)
            | Value::DefaultDict { entries: v, .. } => !v.is_empty(),
            Value::MappingProxy(_) => {
                let length = builtins::call(self, "len", &[id], &[])?;
                self.truth(length)?
            }
            Value::DictView { dict, .. } => !self
                .dict_entries(*dict)
                .expect("dict view backing")
                .is_empty(),
            Value::Context(bindings)
            | Value::Iterator {
                context: Some(bindings),
                ..
            } => !bindings.is_empty(),
            Value::Array(array) => array.len() != 0,
            Value::Range(range) => range.len.as_small() != Some(0),
            Value::Instance { .. }
            | Value::Class {
                metaclass: Some(_), ..
            }
            | Value::Raised(_)
            | Value::FrameLocalsProxy { .. } => {
                let hook = match self.bound_slot(id, "__bool__") {
                    Err(error) if self.is_attribute_error(&error) => {
                        return self.truth_after_missing_bool(id);
                    }
                    result => result?,
                };
                if let Some(hook) = hook {
                    if hook == NONE {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "'{:.200}' cannot be interpreted as a boolean",
                                self.type_name(id)
                            ),
                        ));
                    }
                    let value = self.call(hook, Vec::new(), Vec::new())?;
                    let Value::Bool(result) = self.heap.get(value) else {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "__bool__ should return bool, returned {}",
                                self.type_name(value)
                            ),
                        ));
                    };
                    *result
                } else {
                    self.instance_len(id)?.is_none_or(|len| len != 0)
                }
            }
            _ => true,
        })
    }
    /// Unwrap nested method bindings and retain the identity comparison of their receivers.
    pub(super) fn method_equality_operands(&self, mut a: Id, mut b: Id) -> Option<(Id, Id, bool)> {
        let mut same_receivers = true;
        while a != b {
            let (
                Value::Method {
                    callable: left,
                    receiver: x,
                    ..
                },
                Value::Method {
                    callable: right,
                    receiver: y,
                    ..
                },
            ) = (self.heap.get(a), self.heap.get(b))
            else {
                break;
            };
            if self.builtin_type_name(a) != self.builtin_type_name(b) {
                return None;
            }
            same_receivers &= x == y;
            (a, b) = (*left, *right);
        }
        Some((a, b, same_receivers))
    }

    pub(super) fn equal(&self, a: Id, b: Id) -> bool {
        // Hashable dictionary, set and cache keys cannot contain cyclic containers.
        self.equal_inner(a, b, usize::MAX, &mut false)
    }

    pub(super) fn mapping_proxy(&mut self, mapping: Id) -> Result<Id> {
        // Like mappingproxy_new, require mapping subscription and exclude lists and tuples.
        let supported = match self.heap.get(self.scalar_value(mapping)) {
            Value::List(_) | Value::Tuple(_) | Value::NamedTuple { .. } => false,
            Value::Dict(_)
            | Value::DefaultDict { .. }
            | Value::MappingProxy(_)
            | Value::Str(_)
            | Value::SurrogateStr(_)
            | Value::Bytes(_)
            | Value::ByteArray(_)
            | Value::Array(_)
            | Value::Range(_)
            | Value::MemoryView { .. }
            | Value::Context(_)
            | Value::RegexMatch(_)
            | Value::GenericAlias(_)
            | Value::Union(_) => true,
            _ => {
                self.dict_storage(mapping).is_some()
                    || self.slot(mapping, "__getitem__").is_some()
                    || self.enum_values(mapping).is_some()
            }
        };
        if !supported {
            return Err(Error::new(
                "TypeError",
                format!(
                    "mappingproxy() argument must be a mapping, not {}",
                    self.type_name(mapping)
                ),
            ));
        }
        Ok(self.alloc(Value::MappingProxy(mapping)))
    }

    /// Return the wrapped object without recursing through nested read-only views.
    pub(super) fn mapping_proxy_target(&self, mut value: Id) -> Id {
        while let Value::MappingProxy(mapping) = *self.heap.get(value) {
            value = mapping;
        }
        value
    }

    /// Walk arbitrary container pairs without tying their depth to the Rust stack.
    pub(super) fn checked_equal(&mut self, a: Id, b: Id) -> Result<bool> {
        self.checked_equal_impl(a, b, true)
    }

    pub(super) fn checked_equal_impl(&mut self, a: Id, b: Id, rich_initial: bool) -> Result<bool> {
        use std::collections::HashSet;

        enum Children {
            Sequence(usize),
            Dict {
                position: usize,
                roots: Option<usize>,
            },
            Context(usize),
        }
        let mut active = HashSet::new();
        let mut frames = Vec::new();
        let mut pair = Some((a, b));
        let mut check_special = rich_initial;
        'walk: while pair.is_some() || !frames.is_empty() {
            let mut equal = 'current: {
                let Some((a, b)) = pair.take() else {
                    break 'current true;
                };
                if a == b {
                    break 'current true;
                }
                if check_special
                    && (self.heap.get(a).instance_class().is_some()
                        || self.heap.get(b).instance_class().is_some())
                {
                    self.heap.keep(a);
                    self.heap.keep(b);
                }
                if check_special && let Some(result) = self.binary_special("==", a, b)? {
                    break 'current self.truth(result)?;
                }
                if let (Value::RegexPattern(left), Value::RegexPattern(right)) =
                    (self.heap.get(a), self.heap.get(b))
                {
                    if !left.same_program(right) {
                        break 'current false;
                    }
                    pair = Some((left.source, right.source));
                    check_special = true;
                    continue 'walk;
                }
                if let Some(equal) = stdlib::datetime::comparison::compare(self, a, b, "==")? {
                    break 'current equal;
                }
                if matches!(self.heap.get(a), Value::Method { .. })
                    && matches!(self.heap.get(b), Value::Method { .. })
                {
                    let Some((left, right, same_receivers)) = self.method_equality_operands(a, b)
                    else {
                        break 'current false;
                    };
                    if left != right {
                        self.heap.keep(a);
                        self.heap.keep(b);
                        if !self.checked_equal(left, right)? {
                            break 'current false;
                        }
                    }
                    break 'current same_receivers;
                }
                let left = self.mapping_proxy_target(a);
                let right = self.mapping_proxy_target(b);
                let (a, b) = if right != b { (right, left) } else { (left, b) };
                if a == b {
                    break 'current true;
                }
                let storage = |id| {
                    self.dict_storage(id)
                        .or_else(|| self.sequence_payload(id).map(|(storage, _)| storage))
                        .unwrap_or(id)
                };
                let (a, b) = (storage(a), storage(b));
                if matches!(
                    (self.heap.get(a), self.heap.get(b)),
                    (Value::Set { .. }, Value::Set { .. })
                ) {
                    break 'current self.dictionary_equal(a, b)?;
                }
                if let (Value::Union(left), Value::Union(right)) =
                    (self.heap.get(a), self.heap.get(b))
                {
                    let (left, right) = (*left, *right);
                    self.heap.keep(a);
                    self.heap.keep(b);
                    break 'current left.checked_equal(right, self)?;
                }
                if matches!(
                    (
                        self.heap.get(self.scalar_value(a)),
                        self.heap.get(self.scalar_value(b))
                    ),
                    (Value::GenericAlias(_), Value::GenericAlias(_))
                ) {
                    let Some(parts) = super::typing::checked_alias_parts(self, a, b)? else {
                        break 'current false;
                    };
                    pair = Some(parts);
                    check_special = true;
                    continue 'walk;
                }
                let children = match (self.heap.get(a), self.heap.get(b)) {
                    (Value::List(_), right) if !matches!(right, Value::List(_)) => {
                        break 'current false;
                    }
                    (left, Value::List(_)) if !matches!(left, Value::List(_)) => {
                        break 'current false;
                    }
                    (
                        Value::Dict(x) | Value::DefaultDict { entries: x, .. },
                        Value::Dict(y) | Value::DefaultDict { entries: y, .. },
                    ) => {
                        if x.len() != y.len() {
                            break 'current false;
                        }
                        Some(Children::Dict {
                            position: 0,
                            roots: None,
                        })
                    }
                    (Value::Context(x), Value::Context(y)) => {
                        if x.len() != y.len() {
                            break 'current false;
                        }
                        (!std::rc::Rc::ptr_eq(x, y)).then_some(Children::Context(0))
                    }
                    _ => match (self.sequence_at(a, 0), self.sequence_at(b, 0)) {
                        (Some((x, _)), Some((y, _))) => {
                            if x != y {
                                break 'current false;
                            }
                            Some(Children::Sequence(0))
                        }
                        _ if self.equal(a, b) => None,
                        _ => break 'current false,
                    },
                };
                if let Some(children) = children {
                    let memory = |_| Error::new("MemoryError", "");
                    active.try_reserve(1).map_err(memory)?;
                    if !active.insert((a, b)) {
                        return Err(Error::new(
                            "RecursionError",
                            "maximum comparison depth exceeded",
                        ));
                    }
                    frames.try_reserve(1).map_err(memory)?;
                    self.heap.keep(a);
                    self.heap.keep(b);
                    frames.push(((a, b), children));
                }
                true
            };
            check_special = true;
            while let Some((parents, children)) = frames.last_mut() {
                let (a, b) = *parents;
                // An element callback may have removed the unequal item from both lists.
                if !equal
                    && let Children::Sequence(index) = children
                    && let (Value::List(left), Value::List(right)) =
                        (self.heap.get(a), self.heap.get(b))
                    && *index > left.len()
                    && left.len() == right.len()
                {
                    equal = true;
                }
                pair = match (equal, &mut *children) {
                    (false, _) => None,
                    (true, Children::Sequence(index)) => {
                        let (xlen, x) = self.sequence_at(a, *index).expect("sequence parent");
                        let (ylen, y) = self.sequence_at(b, *index).expect("sequence parent");
                        if let (Some(x), Some(y)) = (x, y) {
                            *index += 1;
                            Some((x, y))
                        } else {
                            equal = xlen == ylen;
                            None
                        }
                    }
                    (true, Children::Dict { position, roots }) => {
                        // Retain one entry across reentrant key and value comparisons.
                        let roots = *roots.get_or_insert_with(|| self.heap.temporary_mark());
                        self.heap.release_temporaries_since(roots);
                        if let Some((at, key, left, hash)) = self.dictionary_pair(a, *position) {
                            *position = at + 1;
                            self.heap.keep(left);
                            if let Some(index) = self.find_dictionary_key(b, key, hash)?.1 {
                                let right = self.dict_entries(b).expect("dict parent")[index].1;
                                self.heap.keep(right);
                                Some((left, right))
                            } else {
                                equal = false;
                                None
                            }
                        } else {
                            None
                        }
                    }
                    (true, Children::Context(index)) => {
                        let (Value::Context(x), Value::Context(y)) =
                            (self.heap.get(a), self.heap.get(b))
                        else {
                            unreachable!()
                        };
                        if x.len() != y.len() {
                            equal = false;
                            None
                        } else if let Some((&key, &value)) = x.iter().nth(*index) {
                            *index += 1;
                            let entry = y.get(&key).map(|&right| (value, right));
                            equal = entry.is_some();
                            entry
                        } else {
                            None
                        }
                    }
                };
                if pair.is_some() {
                    break;
                }
                if !equal
                    && let Children::Dict {
                        roots: Some(mark), ..
                    } = children
                {
                    self.heap.release_temporaries_since(*mark);
                }
                active.remove(parents);
                frames.pop();
            }
            if !equal {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn equal_inner(&self, a: Id, b: Id, remaining: usize, exhausted: &mut bool) -> bool {
        if a == b {
            return true;
        }
        if remaining == 0 {
            *exhausted = true;
            return false;
        }
        let (a, b) = (self.scalar_value(a), self.scalar_value(b));
        let mut equal = |x, y| self.equal_inner(x, y, remaining - 1, exhausted);
        let left = self.dict_storage(a).unwrap_or(a);
        let right = self.dict_storage(b).unwrap_or(b);
        match (self.heap.get(left), self.heap.get(right)) {
            (Value::Weak(left), Value::Weak(right))
                if left.kind == weakref::Kind::Reference
                    && right.kind == weakref::Kind::Reference =>
            {
                match (left.target, right.target) {
                    (Some(left), Some(right)) => equal(left, right),
                    _ => false,
                }
            }
            (Value::None, Value::None) => true,
            (Value::Int(_) | Value::Bool(_), Value::Int(_) | Value::Bool(_)) => self
                .as_int(a)
                .unwrap()
                .cmp(&self.as_int(b).unwrap())
                .is_eq(),
            (Value::Float(x), Value::Float(y)) => x == y,
            (Value::Complex(z), _) => self.complex_equal(*z, right),
            (_, Value::Complex(z)) => self.complex_equal(*z, left),
            (Value::Calendar { value: a, .. }, Value::Calendar { value: b, .. }) => a.equal(b),
            (Value::Range(a), Value::Range(b)) => a.equal(b),
            (Value::Float(f), Value::Int(_) | Value::Bool(_)) => {
                crate::floats::compare(&self.as_int(b).unwrap(), *f) == 0
            }
            (Value::Int(_) | Value::Bool(_), Value::Float(f)) => {
                crate::floats::compare(&self.as_int(a).unwrap(), *f) == 0
            }
            (Value::MemoryView { .. }, _) | (_, Value::MemoryView { .. }) => {
                self.memoryview_equal(a, b)
            }
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::SurrogateStr(a), Value::SurrogateStr(b)) => a == b,
            (Value::Bytes(a) | Value::ByteArray(a), Value::Bytes(b) | Value::ByteArray(b)) => {
                a == b
            }
            (Value::Method { .. }, Value::Method { .. }) => self
                .method_equality_operands(a, b)
                .is_some_and(|(x, y, same)| same && equal(x, y)),
            (Value::Native(a), Value::Native(b)) => {
                super::builtins::native_implementation(a)
                    == super::builtins::native_implementation(b)
            }
            (Value::GenericAlias(a), Value::GenericAlias(b)) => {
                a.comparison_parts(b).is_some_and(|(x, y)| equal(x, y))
            }
            (Value::Slice(x), Value::Slice(y)) => x.iter().zip(y).all(|(&i, &j)| equal(i, j)),
            (Value::RegexPattern(a), Value::RegexPattern(b)) => {
                a.same_program(b) && equal(a.source, b.source)
            }
            (Value::Union(a), Value::Union(b)) => a.structural_equal(*b, self, equal),
            (Value::List(x), Value::List(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(&i, &j)| equal(i, j))
            }
            (Value::List(_), _) | (_, Value::List(_)) => false,
            (
                Value::Dict(x) | Value::DefaultDict { entries: x, .. },
                Value::Dict(y) | Value::DefaultDict { entries: y, .. },
            ) => {
                x.len() == y.len()
                    && x.iter()
                        .all(|&(k, v)| y.iter().any(|&(j, w)| equal(k, j) && equal(v, w)))
            }
            (Value::Context(x), Value::Context(y)) => {
                std::rc::Rc::ptr_eq(x, y)
                    || (x.len() == y.len()
                        && x.iter().all(|(key, &v)| {
                            y.get(key).is_some_and(|&w| {
                                self.equal_inner(v, w, remaining.min(256) - 1, exhausted)
                            })
                        }))
            }
            (Value::Set { .. }, Value::Set { .. }) => self.equal_sets(a, b, remaining, exhausted),
            _ => match (self.sequence(a), self.sequence(b)) {
                (Some(x), Some(y)) => {
                    x.len() == y.len() && x.iter().zip(y).all(|(&i, j)| equal(i, j))
                }
                _ => false,
            },
        }
    }
    // Preserve set candidate order while visiting nested set and tuple pairs iteratively.
    fn equal_sets(&self, a: Id, b: Id, remaining: usize, exhausted: &mut bool) -> bool {
        struct Frame {
            left: Id,
            right: Id,
            index: usize,
            candidate: usize,
            remaining: usize,
            tuple: bool,
        }
        let mut frames = vec![Frame {
            left: a,
            right: b,
            index: 0,
            candidate: 0,
            remaining,
            tuple: false,
        }];
        let mut result = None;
        loop {
            let Some(frame) = frames.last_mut() else {
                return result.unwrap_or(true);
            };
            if let Some(equal) = result.take() {
                if equal {
                    frame.index += 1;
                    frame.candidate = 0;
                } else if frame.tuple {
                    frames.pop();
                    result = Some(false);
                    continue;
                } else {
                    frame.candidate += 1;
                }
            }
            let pair = if frame.tuple {
                let (Value::Tuple(left), Value::Tuple(right)) =
                    (self.heap.get(frame.left), self.heap.get(frame.right))
                else {
                    unreachable!()
                };
                if left.len() != right.len() {
                    frames.pop();
                    result = Some(false);
                    continue;
                }
                if frame.index == left.len() {
                    frames.pop();
                    result = Some(true);
                    continue;
                }
                (left[frame.index], right[frame.index])
            } else {
                let (Value::Set { entries: left, .. }, Value::Set { entries: right, .. }) =
                    (self.heap.get(frame.left), self.heap.get(frame.right))
                else {
                    unreachable!()
                };
                if left.len() != right.len()
                    || frame.candidate == right.len() && frame.index < left.len()
                {
                    frames.pop();
                    result = Some(false);
                    continue;
                }
                if frame.index == left.len() {
                    frames.pop();
                    result = Some(true);
                    continue;
                }
                (left[frame.index].0, right[frame.candidate].0)
            };
            let (mut x, mut y) = pair;
            let mut depth = frame.remaining - 1;
            loop {
                if x == y {
                    result = Some(true);
                    break;
                }
                if depth == 0 {
                    *exhausted = true;
                    result = Some(false);
                    break;
                }
                let (raw_x, raw_y) = (x, y);
                (x, y) = (self.scalar_value(x), self.scalar_value(y));
                let tail = match (self.heap.get(x), self.heap.get(y)) {
                    (Value::GenericAlias(a), Value::GenericAlias(b)) => {
                        let Some(parts) = a.comparison_parts(b) else {
                            result = Some(false);
                            break;
                        };
                        Some(parts)
                    }
                    (Value::Weak(a), Value::Weak(b))
                        if a.kind == weakref::Kind::Reference
                            && b.kind == weakref::Kind::Reference =>
                    {
                        match (a.target, b.target) {
                            (Some(left), Some(right)) => Some((left, right)),
                            _ => {
                                result = Some(false);
                                break;
                            }
                        }
                    }
                    (Value::Method { .. }, Value::Method { .. }) => {
                        match self.method_equality_operands(x, y) {
                            Some((left, right, true)) => Some((left, right)),
                            _ => {
                                result = Some(false);
                                break;
                            }
                        }
                    }
                    _ => None,
                };
                if let Some(pair) = tail {
                    (x, y) = pair;
                    depth -= 1;
                    continue;
                }
                let tuple = match (self.heap.get(x), self.heap.get(y)) {
                    (Value::Set { .. }, Value::Set { .. }) => Some(false),
                    (Value::Tuple(_), Value::Tuple(_)) => Some(true),
                    _ => None,
                };
                if let Some(tuple) = tuple {
                    frames.push(Frame {
                        left: x,
                        right: y,
                        index: 0,
                        candidate: 0,
                        remaining: depth,
                        tuple,
                    });
                } else {
                    result = Some(self.equal_inner(raw_x, raw_y, depth, exhausted));
                }
                break;
            }
        }
    }
    /// Identify operands that share the native list or tuple comparison slot.
    fn same_sequence_family(&self, a: Id, b: Id) -> bool {
        ["list", "tuple"].into_iter().any(|family| {
            self.builtin_slot_accepts(a, family) && self.builtin_slot_accepts(b, family)
        })
    }

    /// Report an ordering operation declined by both operands.
    fn unsupported_order(&self, op: &str, a: Id, b: Id) -> Error {
        let module = |id| match self.heap.get(id) {
            Value::Calendar { .. } => "datetime.",
            _ => "",
        };
        Error::new(
            "TypeError",
            format!(
                "'{op}' not supported between instances of '{}{}' and '{}{}'",
                module(a),
                self.type_name(a),
                module(b),
                self.type_name(b)
            ),
        )
    }

    /// Compare stored set members using their insertion hashes, including for frozen sets.
    pub(super) fn order_sets(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        let (subset, superset) = if matches!(op, ">" | ">=") {
            (b, a)
        } else {
            (a, b)
        };
        let subset_len = self.stored_entries(subset).len();
        let superset_len = self.stored_entries(superset).len();
        if subset_len > superset_len || (subset_len == superset_len && matches!(op, "<" | ">")) {
            return Ok(FALSE);
        }
        self.set_members_match(subset, superset, true)
    }

    /// Test stored keys with their insertion hashes, stopping at the first mismatch.
    pub(super) fn set_members_match(&mut self, source: Id, other: Id, present: bool) -> Result<Id> {
        let mut position = 0;
        while let Some((at, key, _, hash)) = self.dictionary_pair(source, position) {
            let member = self.find_dictionary_key(other, key, hash);
            if let Err(error) = &member {
                for id in error.roots() {
                    self.heap.keep(id);
                }
            }
            // Equality can remove the current key. Release the lookup reference
            // before advancing the live cursor, so its finalizer may mutate the set.
            let mark = self.heap.temporary_mark();
            self.heap.keep(key);
            self.discard_truth(mark);
            if member?.1.is_some() != present {
                return Ok(FALSE);
            }
            position = at + 1;
        }
        Ok(TRUE)
    }

    /// Return the scalar ordering or the comparison result of the first unequal elements.
    fn order(&mut self, op: &str, original_a: Id, original_b: Id) -> Result<Id> {
        if let Some(result) =
            stdlib::datetime::comparison::compare(self, original_a, original_b, op)?
        {
            return Ok(builtins::boolean(result));
        }
        let (a, b) = (self.scalar_value(original_a), self.scalar_value(original_b));
        let int_float = |integer, float| -> Result<Option<Ordering>> {
            let order = crate::floats::compare(&self.as_int(integer)?, float);
            Ok((order != 2).then(|| order.cmp(&0)))
        };
        let ordering = match (self.heap.get(a), self.heap.get(b)) {
            (Value::Set { .. }, Value::Set { .. }) => return self.order_sets(op, a, b),
            (Value::Int(_) | Value::Bool(_), Value::Int(_) | Value::Bool(_)) => {
                Ok(Some(self.as_int(a)?.cmp(&self.as_int(b)?)))
            }
            (Value::Float(x), Value::Float(y)) => Ok(x.partial_cmp(y)),
            (Value::Calendar { value: a, .. }, Value::Calendar { value: b, .. })
                if a.class() == b.class() && a.class() != "timezone" =>
            {
                Ok(Some(a.order(b, op)?))
            }
            (Value::Int(_) | Value::Bool(_), Value::Float(f)) => int_float(a, *f),
            (Value::Float(f), Value::Int(_) | Value::Bool(_)) => {
                Ok(int_float(b, *f)?.map(Ordering::reverse))
            }
            (Value::Str(x), Value::Str(y)) => Ok(Some(x.cmp(y))),
            (Value::SurrogateStr(x), Value::SurrogateStr(y)) => Ok(Some(x.cmp(y))),
            (Value::Str(x), Value::SurrogateStr(y)) => {
                Ok(Some(x.chars().map(u32::from).cmp(y.iter().copied())))
            }
            (Value::SurrogateStr(x), Value::Str(y)) => {
                Ok(Some(x.iter().copied().cmp(y.chars().map(u32::from))))
            }
            (Value::Bytes(x) | Value::ByteArray(x), Value::Bytes(y) | Value::ByteArray(y)) => {
                Ok(Some(x.cmp(y)))
            }
            _ => {
                if !self.same_sequence_family(a, b) {
                    return Err(self.unsupported_order(op, original_a, original_b));
                }
                let mut i = 0;
                let mark = self.heap.temporary_mark();
                loop {
                    let (_, left) = self.sequence_at(a, i).expect("sequence operand");
                    let (_, right) = self.sequence_at(b, i).expect("sequence operand");
                    let (Some(left), Some(right)) = (left, right) else {
                        break;
                    };
                    if left != right {
                        self.heap.reserve_temporaries(2)?;
                        self.heap.keep(left);
                        self.heap.keep(right);
                        let equal = self.checked_equal(left, right)?;
                        self.heap.release_temporaries_since(mark);
                        if !equal {
                            break;
                        }
                    }
                    i += 1;
                }
                // Equality can resize either list or replace the pair that compares unequal.
                let (left_len, left) = self.sequence_at(a, i).expect("sequence operand");
                let (right_len, right) = self.sequence_at(b, i).expect("sequence operand");
                if let (Some(left), Some(right)) = (left, right) {
                    self.heap.reserve_temporaries(2)?;
                    self.heap.keep(left);
                    self.heap.keep(right);
                    return self.with_slot_depth("maximum comparison depth exceeded", |ctx| {
                        ctx.binary(op, left, right)
                    });
                }
                Ok(Some(left_len.cmp(&right_len)))
            }
        }?;
        Ok(builtins::boolean(ordering.is_some_and(|order| match op {
            "<" => order.is_lt(),
            "<=" => order.is_le(),
            ">" => order.is_gt(),
            ">=" => order.is_ge(),
            _ => unreachable!("ordering operator"),
        })))
    }
    pub(super) fn display(&self, id: Id) -> String {
        Self::render_containers(
            self,
            RenderRoot::Object(id),
            false,
            false,
            |_, _, _, _, _, _| Ok(None),
        )
        .unwrap_or_else(|_| "<exception str() failed>".to_owned())
    }

    pub(super) fn decode_message(&self, state: &super::value::DecodeState) -> Result<String> {
        let Value::Bytes(data) = self.heap.get(state.object) else {
            return Err(Error::new(
                "TypeError",
                "UnicodeError 'object' attribute must be a bytes",
            ));
        };
        let start = state.start;
        let location = if let Ok(index) = usize::try_from(start)
            && let Some(byte) = data.get(index)
            && state.end.checked_sub(1) == Some(start)
        {
            format!("byte 0x{byte:02x} in position {start}")
        } else {
            format!("bytes in position {start}-{}", state.end.wrapping_sub(1))
        };
        Ok(format!(
            "'{}' codec can't decode {location}: {}",
            self.display(state.encoding),
            self.display(state.reason)
        ))
    }

    pub(super) fn repr(&self, id: Id) -> String {
        Self::render_containers(
            self,
            RenderRoot::Object(id),
            true,
            false,
            |_, _, _, _, _, _| Ok(None),
        )
        .unwrap_or_else(|_| "<exception repr() failed>".to_owned())
    }

    /// Keep the boxed result of str(arg) when an exception has one positional argument.
    pub(super) fn single_exception_str(&mut self, root: Id) -> Result<Option<Id>> {
        let mut value = root;
        let mut seen = std::collections::HashSet::new();
        while let Value::Raised(error) = self.heap.get(value) {
            if error.os_args.is_some()
                || error.group().is_some()
                || error.trace.as_ref().is_some_and(|trace| {
                    trace
                        .import_message
                        .is_some_and(|msg| matches!(self.heap.get(msg), Value::Str(_)))
                        || trace.source_syntax.is_some()
                })
                || self.is_builtin_error(error, "KeyError")
                || error
                    .class()
                    .is_some_and(|class| self.class_attr(class, "__str__").is_some())
            {
                break;
            }
            let Some([arg]) = self.exception_arguments(error) else {
                break;
            };
            if matches!(self.heap.get(*arg), Value::Raised(_)) {
                seen.try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                if !seen.insert(value) {
                    return Err(Error::new(
                        "RecursionError",
                        "maximum recursion depth exceeded while getting the str of an object",
                    ));
                }
            }
            value = *arg;
        }
        if value == root {
            return Ok(None);
        }
        let string = self.builtin("str");
        self.call(string, vec![value], Vec::new()).map(Some)
    }

    /// Render container ancestors iteratively so recursive values do not consume the native stack.
    pub(super) fn checked_format(&mut self, root: Id, quoted: bool) -> Result<String> {
        if !quoted && let Some(text) = super::stdlib::datetime::render::exact(self, root, false)? {
            return self.str_value(text).map(str::to_owned);
        }
        if !quoted && let Some(value) = self.single_exception_str(root)? {
            return self.str_value(value).map(str::to_owned);
        }
        self.checked_render(RenderRoot::Object(root), quoted)
    }

    /// Explicit builtin repr slots use their receiver's storage while children retain their hooks.
    pub(super) fn checked_container_slot_repr(
        &mut self,
        object: Id,
        storage: Id,
    ) -> Result<String> {
        self.checked_render(
            RenderRoot::BuiltinContainer {
                object,
                storage,
                with_factory: false,
            },
            true,
        )
    }

    /// Bypass the receiver's override for an explicit count or repeat descriptor call.
    pub(super) fn checked_cursor_slot_repr(&mut self, object: Id) -> Result<String> {
        self.checked_render(RenderRoot::Cursor(object), true)
    }

    /// Render the native defaultdict slot while looking up hooks on its live children.
    pub(super) fn checked_defaultdict_slot_repr(
        &mut self,
        object: Id,
        storage: Id,
    ) -> Result<String> {
        self.checked_render(
            RenderRoot::BuiltinContainer {
                object,
                storage,
                with_factory: true,
            },
            true,
        )
    }

    fn checked_render(&mut self, root: RenderRoot<'_>, quoted: bool) -> Result<String> {
        Self::render_containers(
            self,
            root,
            quoted,
            true,
            |ctx, id, lookup, work, ancestors, remaining| {
                ctx.element_format(id, lookup, work, ancestors, remaining)
            },
        )
    }

    /// Resolve formatting methods on heap instances and exceptions that retain their Python class.
    fn format_slot(&self, id: Id, name: &str) -> Option<(Id, Id)> {
        let class = match self.heap.get(id) {
            Value::Instance { class, .. } | Value::FrameLocalsProxy { class, .. } => *class,
            Value::Module {
                constructor: Some(class),
                ..
            } => {
                // The native module slot uses the renderer's recursion guard.
                let slot = self.class_attr(*class, name)?;
                return (self.builtins.get("module.__repr__") != Some(&slot))
                    .then_some((*class, slot));
            }
            Value::Raised(error) => error.class()?,
            Value::Weak(wr) if name == "__repr__" => *self.builtins.get(wr.kind.class())?,
            Value::Partial { .. } if name == "__repr__" => {
                *self.builtins.get(stdlib::functools::PARTIAL_CLASS)?
            }
            value if name == "__repr__" => match value.native_io_class_name()? {
                class @ ("_io.BufferedReader" | "_io.BufferedWriter" | "_io.TextIOWrapper") => {
                    *self.builtins.get(class)?
                }
                _ => return None,
            },
            _ => return None,
        };
        self.class_attr(class, name).map(|slot| (class, slot))
    }

    /// Write a set's type name or cycle marker and return the closer for a nonempty body.
    fn begin_set_repr(
        &self,
        output: &mut String,
        object: Id,
        wrapped: bool,
        empty: bool,
        recursive: bool,
    ) -> Result<Option<&'static str>> {
        if empty || recursive {
            append_rendered_text(output, self.type_name(object))?;
            append_rendered_text(output, if recursive { "(...)" } else { "()" })?;
            return Ok(None);
        }
        if wrapped {
            append_rendered_text(output, self.type_name(object))?;
            append_rendered_text(output, "(")?;
        }
        Ok(Some(if wrapped { "})" } else { "}" }))
    }

    /// Resolve inherited builtin container repr slots that can share the active worklist.
    fn native_repr_storage(&self, object: Id) -> Option<(Id, bool)> {
        let (_, slot) = self.format_slot(object, "__repr__")?;
        let Value::Native(name) = self.heap.get(slot) else {
            return None;
        };
        match name.as_str() {
            "dict.__repr__" => self.dict_storage(object).map(|storage| (storage, false)),
            "collections.defaultdict.__repr__" => self
                .dict_storage(object)
                .filter(|&storage| matches!(self.heap.get(storage), Value::DefaultDict { .. }))
                .map(|storage| (storage, true)),
            "itertools.count.__repr__" | "itertools.repeat.__repr__" => {
                let storage = self.scalar_value(object);
                matches!(
                    (name.as_str(), self.heap.get(storage)),
                    ("itertools.count.__repr__", Value::Count { .. })
                        | ("itertools.repeat.__repr__", Value::Repeat(_))
                )
                .then_some((storage, false))
            }
            "list.__repr__" | "tuple.__repr__" | "set.__repr__" | "frozenset.__repr__" => {
                let storage = self.scalar_value(object);
                matches!(
                    (name.as_str(), self.heap.get(storage)),
                    ("list.__repr__", Value::List(_))
                        | ("tuple.__repr__", Value::Tuple(_))
                        | ("set.__repr__", Value::Set { frozen: false, .. })
                        | ("frozenset.__repr__", Value::Set { frozen: true, .. })
                )
                .then_some((storage, false))
            }
            _ => None,
        }
    }

    /// Invoke the special method once and preserve an accepted string subclass.
    pub(super) fn repr_override(&mut self, id: Id) -> Result<Option<Id>> {
        let value = self.call_repr_slot(id)?;
        if let Some(result) = value
            && self.string_storage(result).is_none()
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__repr__ returned non-string (type {})",
                    self.type_name(result)
                ),
            ));
        }
        Ok(value)
    }

    /// Call the live repr slot directly, as the inherited object.__str__ descriptor does.
    pub(super) fn call_repr_slot(&mut self, id: Id) -> Result<Option<Id>> {
        if let Some(text) = super::stdlib::datetime::render::exact(self, id, true)? {
            return Ok(Some(text));
        }
        if let Some(result) = super::paramspec::repr(self, id) {
            return result.map(Some);
        }
        let Some((class, slot)) = self.format_slot(id, "__repr__") else {
            return Ok(None);
        };
        self.with_repr_slot_depth(|ctx| {
            if matches!(ctx.heap.get(slot), Value::Native(symbol)
                if super::wrapper_descriptor::BuiltinDescriptor::lookup(symbol).is_some())
            {
                ctx.call(slot, vec![id], Vec::new())
            } else {
                let hook = ctx.bind_attribute(slot, id, class)?;
                ctx.call(hook, Vec::new(), Vec::new())
            }
        })
        .map(Some)
    }

    /// A recursive repr may delegate entirely through native descriptors, without a Python frame.
    fn with_repr_slot_depth<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        const MESSAGE: &str =
            "maximum recursion depth exceeded while getting the repr of an object";
        let dispatch = |ctx: &mut Self| ctx.with_slot_depth(MESSAGE, operation);
        // Unoptimized builtin dispatch can cross the usual slot margin in one call.
        // Allow a finite delegation to continue on one spare segment. Its own
        // margin remains the boundary for an unbounded chain.
        if cfg!(debug_assertions)
            && stacker::remaining_stack().is_some_and(|remaining| remaining < 512 * 1024)
        {
            if self.repr_spare_stack {
                return Err(Error::new("RecursionError", MESSAGE));
            }
            self.repr_spare_stack = true;
            let result = stacker::grow(4 * 1024 * 1024, || dispatch(self));
            self.repr_spare_stack = false;
            return result;
        }
        dispatch(self)
    }

    /// Look up a method's live qualified name, falling back only when that attribute is absent.
    fn bound_method_name(&mut self, callable: Id) -> Result<Option<Id>> {
        for attribute in ["__qualname__", "__name__"] {
            match self.lookup_attr(callable, attribute) {
                Ok(name) => return Ok(self.string_storage(name)),
                Err(error) if self.is_attribute_error(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    /// Keep pending children alive while a hook mutates and reenters its container.
    fn element_format(
        &mut self,
        id: Id,
        lookup: FormatLookup,
        work: &mut [FormatPart],
        ancestors: &[Id],
        sequence_words: Option<usize>,
    ) -> Result<Option<Id>> {
        let calendar_repr = matches!(lookup, FormatLookup::Repr)
            && super::stdlib::datetime::render::python_repr(self, id);
        if matches!(lookup, FormatLookup::Repr | FormatLookup::Str)
            && !calendar_repr
            && self.format_slot(id, "__repr__").is_none()
            && (matches!(lookup, FormatLookup::Repr) || self.format_slot(id, "__str__").is_none())
        {
            let (Value::ParamSpec(_) | Value::ParamSpecMarker(_, _)) = self.heap.get(id) else {
                return Ok(None);
            };
        }
        self.repr_ancestors
            .try_reserve(ancestors.len())
            .map_err(render_allocation)?;
        self.heap
            .reserve_temporaries(work.len().saturating_add(1))?;
        let mark = self.heap.temporary_mark();
        let depth = self.repr_ancestors.len();
        self.heap.keep(id);
        for part in work {
            part.keep_alive(&mut self.heap);
        }
        self.repr_ancestors.extend_from_slice(ancestors);
        let previous = std::mem::replace(&mut self.repr_sequence_words, sequence_words);
        let converted = match lookup {
            FormatLookup::Repr => self.repr_override(id),
            FormatLookup::Str => builtins::call(self, "str", &[id], &[]).map(Some),
            FormatLookup::MethodName => self.bound_method_name(id),
            FormatLookup::ModuleRepr => self.module_base_repr(id).map(Some),
            FormatLookup::DictViewList => builtins::call(self, "list", &[id], &[]).map(Some),
            FormatLookup::TypingTuple(frames) => {
                self.suspended_typing_frames += frames;
                let result = typing::typing_alias_helper(self, "_type_repr", vec![id]);
                self.suspended_typing_frames -= frames;
                result.map(Some)
            }
        };
        self.repr_sequence_words = previous;
        self.repr_ancestors.truncate(depth);
        self.heap.release_temporaries_since(mark);
        converted
    }

    /// Write exception headers and queue their arguments on the same container worklist.
    fn render_exception<'a>(
        &'a self,
        mut error: &'a Error,
        mut repr: bool,
        checked: bool,
        output: &mut String,
        work: &mut Vec<FormatPart>,
    ) -> Result<()> {
        use FormatPart as Part;
        let mut closing = 0;
        let mut seen = std::collections::HashSet::new();
        loop {
            let name = error.kind.rsplit('.').next().unwrap_or(&error.kind);
            if let Some([message, exceptions]) = error.group() {
                push_frame(work, Part::Closing(closing))?;
                if repr {
                    append_rendered_text(output, name)?;
                    append_rendered_text(output, "(")?;
                    push_frame(work, Part::Text(")"))?;
                    if let Some(saved) = error.group_sequence_repr() {
                        push_frame(work, Part::Value(saved, false))?;
                    } else if self
                        .exception_arguments(error)
                        .and_then(|args| args.get(1))
                        .is_some_and(|&source| self.list_storage(source).is_some())
                    {
                        push_frame(work, Part::Text("]"))?;
                        push_frame(work, Part::Sequence(exceptions, 0))?;
                        push_frame(work, Part::Text("["))?;
                    } else {
                        push_frame(work, Part::Value(exceptions, true))?;
                    }
                    push_frame(work, Part::Text(", "))?;
                } else {
                    let Value::Tuple(children) = self.heap.get(exceptions) else {
                        unreachable!("exception group children are stored as a tuple")
                    };
                    let count = children.len();
                    let plural = if count == 1 { "" } else { "s" };
                    push_frame(
                        work,
                        Part::Suffix(format!(" ({count} sub-exception{plural})")),
                    )?;
                }
                push_frame(work, Part::Value(message, repr))?;
                break;
            }
            if repr {
                append_rendered_text(output, name)?;
            } else if let Some(message) =
                error.trace.as_ref().and_then(|trace| trace.import_message)
                && let Value::Str(text) = self.heap.get(message)
            {
                append_rendered_text(output, text)?;
                close_parens(output, closing)?;
                break;
            } else if error.kind == "UnicodeEncodeError"
                && error.args.is_some()
                && !error.message.is_empty()
            {
                append_rendered_text(output, &error.message)?;
                close_parens(output, closing)?;
                break;
            } else if let Some(state) = &error.decode {
                append_rendered_text(output, &self.decode_message(state)?)?;
                output.extend(std::iter::repeat_n(')', closing));
                break;
            } else if let Some(super::os_error::Args::Python(fields)) = error.os_args.as_deref()
                && let Some(fragments) = fields.fragments()
            {
                work.try_reserve(9)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                push_frame(work, Part::Closing(closing))?;
                for part in fragments.into_iter().rev().flatten() {
                    push_frame(
                        work,
                        match part {
                            super::os_error::Fragment::Text(text) => Part::Text(text),
                            super::os_error::Fragment::Value(id, repr) => Part::Value(id, repr),
                        },
                    )?;
                }
                break;
            } else if matches!(
                error.os_args.as_deref(),
                Some(super::os_error::Args::Native { .. })
            ) || error
                .trace
                .as_ref()
                .is_some_and(|trace| trace.source_syntax.is_some())
            {
                append_rendered_text(output, &error.message)?;
                close_parens(output, closing)?;
                break;
            }
            let args = self.exception_arguments(error);
            if let Some([arg]) = args {
                if repr {
                    append_rendered_text(output, "(")?;
                    closing += 1;
                }
                repr |= self.is_builtin_error(error, "KeyError");
                if let Value::Raised(next) = self.heap.get(*arg)
                    && !(checked && repr && self.format_slot(*arg, "__repr__").is_some())
                {
                    if !repr
                        && next
                            .trace
                            .as_ref()
                            .is_some_and(|trace| trace.source_syntax.is_some())
                    {
                        push_frame(work, Part::Closing(closing))?;
                        push_frame(work, Part::Value(*arg, false))?;
                        break;
                    }
                    if !enter_ancestor(&mut seen, *arg)? {
                        let op = if repr { "repr" } else { "str" };
                        return Err(Error::new(
                            "RecursionError",
                            format!(
                                "maximum recursion depth exceeded while getting the {op} of an object"
                            ),
                        ));
                    }
                    error = next;
                    continue;
                }
                push_frame(work, Part::Closing(closing))?;
                push_frame(work, Part::Value(*arg, repr))?;
            } else if let Some(tuple) = error.args {
                push_frame(work, Part::Closing(closing))?;
                if repr || args.is_some_and(|items| !items.is_empty()) {
                    push_frame(work, Part::Value(tuple, true))?;
                }
            } else {
                if repr {
                    append_rendered_text(output, "(")?;
                    if let Some(super::os_error::Args::Native { source: os, .. }) =
                        error.os_args.as_deref()
                    {
                        append_rendered_text(
                            output,
                            &format!(
                                "{}, {}",
                                os.errno,
                                crate::objects::exceptions::string_repr(&os.description)
                            ),
                        )?;
                    } else if !error.message.is_empty() {
                        append_rendered_text(
                            output,
                            &crate::objects::exceptions::string_repr(&error.message),
                        )?;
                    }
                    append_rendered_text(output, ")")?;
                } else {
                    append_rendered_text(output, &error.message)?;
                }
                close_parens(output, closing)?;
            }
            break;
        }

        Ok(())
    }

    fn render_containers<C: std::ops::Deref<Target = Self>>(
        mut ctx: C,
        root: RenderRoot<'_>,
        quoted: bool,
        checked: bool,
        mut element_repr: impl FnMut(
            &mut C,
            Id,
            FormatLookup,
            &mut [FormatPart],
            &[Id],
            Option<usize>,
        ) -> Result<Option<Id>>,
    ) -> Result<String> {
        use FormatPart as Part;
        let mut output = String::new();
        let mut work = Vec::new();
        let mut active = Vec::new();
        let mut syntax = std::collections::HashSet::new();
        let mut ancestors = std::collections::HashSet::new();
        if checked {
            ancestors
                .try_reserve(ctx.repr_ancestors.len())
                .map_err(render_allocation)?;
            ancestors.extend(ctx.repr_ancestors.iter().copied());
        }
        match root {
            RenderRoot::Object(id) => push_frame(&mut work, Part::Value(id, quoted))?,
            RenderRoot::Cursor(id) => push_frame(&mut work, Part::Cursor(id))?,
            RenderRoot::BuiltinContainer {
                object,
                storage,
                with_factory,
            } => {
                let (open, end, cycle, contents) = match ctx.heap.get(storage) {
                    Value::Set { entries, frozen } => {
                        let Some(end) = ctx.begin_set_repr(
                            &mut output,
                            object,
                            *frozen || object != storage,
                            entries.is_empty(),
                            ancestors.contains(&object),
                        )?
                        else {
                            return Ok(output);
                        };
                        (
                            "{",
                            end,
                            "set(...)",
                            Part::snapshot(entries.iter().map(|&(key, _)| key))?,
                        )
                    }
                    Value::List(_) => ("[", "]", "[...]", Part::Sequence(storage, 0)),
                    Value::Tuple(items) => (
                        "(",
                        if items.len() == 1 { ",)" } else { ")" },
                        "(...)",
                        Part::Sequence(storage, 0),
                    ),
                    Value::Dict(_) | Value::DefaultDict { .. } => {
                        ("{", "}", "{...}", Part::Entries(storage, 0, true))
                    }
                    _ => unreachable!("native container repr slot"),
                };
                if with_factory {
                    push_frame(&mut work, Part::DefaultDictFactory(object, 0))?;
                }
                if enter_ancestor(&mut ancestors, object)? {
                    append_rendered_text(&mut output, open)?;
                    push_frame(&mut active, object)?;
                    push_frame(&mut work, Part::Leave(object, end))?;
                    push_frame(&mut work, contents)?;
                } else {
                    append_rendered_text(&mut output, cycle)?;
                }
            }
            RenderRoot::Exception(error) => {
                ctx.render_exception(error, quoted, checked, &mut output, &mut work)?;
            }
        }
        // CPython guards nested slice formatting against the thread's native stack.
        // The pinned x86-64 Linux reference build uses 624 bytes per slice
        // and keeps two native-stack margins before raising RecursionError.
        const SLICE_FRAME_WORDS: usize = 78;
        // Nested list and tuple repr use 128 bytes per level in CPython 3.14 on x86-64.
        const SEQUENCE_FRAME_WORDS: usize = 16;
        // A pinned x86-64 CPython deque also formats a temporary list; each
        // nested deque uses 832 bytes, including its ordinary sequence frame.
        const EXTRA_DEQUE_FRAME_WORDS: usize = 104 - SEQUENCE_FRAME_WORDS;
        let stack_words = checked.then(remaining_repr_words).flatten();
        let slice_budget = stack_words.map(|words| words / SLICE_FRAME_WORDS);
        // A Python callback may run on a new stack segment. Its renderer
        // still shares the logical space left by the suspended containers.
        let sequence_stack_words = if checked {
            ctx.repr_sequence_words.or(stack_words)
        } else {
            None
        };
        let mut extra_deque_words = 0usize;
        // Stock count and repeat consume 1,088 native stack bytes per nested repr on x86-64.
        const CURSOR_FRAME_WORDS: usize = 136;
        let mut cursor_words = 0usize;
        let mut slice_depth = 0usize;
        let mut namedtuple_depth = 0usize;
        // Pinned CPython 3.14 uses 192 native bytes per nested builtin alias with
        // an exact list of arguments. A Literal or Final adds three Python frames
        // and 1,264 native bytes when formatting the same nesting on x86-64.
        let type_budget = stack_words.unwrap_or(1024 * 1024);
        let mut type_words = 0usize;
        let mut typing_frames = 0usize;
        while let Some(part) = work.pop() {
            let remaining_sequence_words = sequence_stack_words.map(|limit| {
                limit.saturating_sub(
                    active.len() * SEQUENCE_FRAME_WORDS + extra_deque_words + cursor_words,
                )
            });
            match part {
                Part::Closing(count) => close_parens(&mut output, count)?,
                Part::Cursor(id) => {
                    if remaining_sequence_words
                        .map_or(cursor_words >= 1024 * 1024, |n| n < CURSOR_FRAME_WORDS)
                    {
                        return Err(Error::new(
                            "RecursionError",
                            "maximum recursion depth exceeded while getting the repr of an object",
                        ));
                    }
                    cursor_words += CURSOR_FRAME_WORDS;
                    push_frame(&mut work, Part::CursorEnd)?;
                    append_rendered_text(&mut output, ctx.type_name(id))?;
                    append_rendered_text(&mut output, "(")?;
                    // Snapshot both operands before a Python repr hook can advance the cursor.
                    let first = match ctx.heap.get(ctx.scalar_value(id)) {
                        Value::Count { current, step, .. } => {
                            if stdlib::itertools::machine_integer(&ctx, *step) != Some(1) {
                                push_frame(&mut work, Part::Value(*step, true))?;
                                push_frame(&mut work, Part::Text(", "))?;
                            }
                            *current
                        }
                        Value::Repeat(repeat) => {
                            if let Some(count) = repeat.remaining {
                                push_frame(&mut work, Part::Suffix(format!(", {count}")))?;
                            }
                            repeat.value
                        }
                        _ => unreachable!("native count or repeat repr slot"),
                    };
                    push_frame(&mut work, Part::Value(first, true))?;
                }
                Part::CursorEnd => {
                    cursor_words -= CURSOR_FRAME_WORDS;
                    append_rendered_text(&mut output, ")")?;
                }
                Part::Text(text) => append_rendered_text(&mut output, text)?,
                Part::ContextEnd(id) => {
                    use std::fmt::Write;
                    output
                        .try_reserve(7 + 2 * std::mem::size_of::<Id>())
                        .map_err(render_allocation)?;
                    write!(&mut output, " at {id:#x}>")
                        .expect("the hexadecimal object ID fits the reserved space");
                }
                Part::TypeEnd(_, end, python) => {
                    type_words -= if python { 158 } else { 24 };
                    if python {
                        typing_frames -= 3;
                    }
                    append_rendered_text(&mut output, end)?;
                }
                Part::TypingArgument(id) => {
                    if !matches!(ctx.heap.get(id), Value::Tuple(_))
                        && !ctx
                            .builtins
                            .get("tuple")
                            .is_some_and(|&ty| ctx.instance_of(id, ty))
                    {
                        push_frame(&mut work, Part::TypeArgument(id, false))?;
                        continue;
                    }
                    if let Some(text) = element_repr(
                        &mut ctx,
                        id,
                        // Of this alias's three modeled frames, only __repr__ is suspended.
                        // The callback enters _type_repr and its callees as real Python frames.
                        FormatLookup::TypingTuple(typing_frames - 2),
                        &mut work,
                        &active,
                        remaining_sequence_words,
                    )? {
                        append_rendered_text(&mut output, ctx.str_value(text)?)?;
                    } else if let Some(items) = ctx.tuple_items(id)? {
                        // Internal diagnostics render stored tuple elements without calling Python.
                        append_rendered_text(&mut output, "[")?;
                        push_frame(&mut work, Part::Text("]"))?;
                        for (i, item) in items.into_iter().enumerate().rev() {
                            push_frame(&mut work, Part::TypingArgument(item))?;
                            if i != 0 {
                                push_frame(&mut work, Part::Text(", "))?;
                            }
                        }
                    } else {
                        push_frame(&mut work, Part::TypeArgument(id, false))?;
                    }
                }
                Part::TypeArgument(id, direct_list) => {
                    if direct_list && let Value::List(items) = ctx.heap.get(id) {
                        append_rendered_text(&mut output, "[")?;
                        push_frame(&mut work, Part::Text("]"))?;
                        push_frame(&mut work, Part::TypeList(id, 0, items.len()))?;
                    } else if let Value::Function { fields, .. } = ctx.heap.get(id)
                        && fields.module != NONE
                        && (ctx.dict_named_item(fields.dict, "__origin__").is_none()
                            || ctx.dict_named_item(fields.dict, "__args__").is_none())
                    {
                        push_frame(&mut work, Part::Value(fields.qualname, false))?;
                        if ctx.str_value(fields.module).ok() != Some("builtins") {
                            push_frame(&mut work, Part::Text("."))?;
                            push_frame(&mut work, Part::Value(fields.module, false))?;
                        }
                    } else if !typing::append_type_label(&ctx, id, &mut output)? {
                        push_frame(&mut work, Part::Value(id, true))?;
                    }
                }
                Part::TypeList(id, index, len) => {
                    if index < len {
                        let Value::List(items) = ctx.heap.get(id) else {
                            unreachable!()
                        };
                        let item = *items
                            .get(index)
                            .ok_or_else(|| Error::new("IndexError", "list index out of range"))?;
                        if index != 0 {
                            append_rendered_text(&mut output, ", ")?;
                        }
                        push_frame(&mut work, Part::TypeList(id, index + 1, len))?;
                        push_frame(&mut work, Part::TypeArgument(item, false))?;
                    }
                }
                Part::NamedTupleEnd => {
                    namedtuple_depth -= 1;
                    append_rendered_text(&mut output, ")")?;
                }
                Part::SliceEnd => {
                    slice_depth -= 1;
                    append_rendered_text(&mut output, ")")?;
                }
                Part::Leave(id, end) => {
                    if matches!(ctx.heap.get(id), Value::Deque(_)) {
                        extra_deque_words -= EXTRA_DEQUE_FRAME_WORDS;
                    }
                    ancestors.remove(&id);
                    let last = active.pop();
                    debug_assert_eq!(last, Some(id));
                    append_rendered_text(&mut output, end)?;
                }
                Part::Sequence(id, index) => {
                    let item = match ctx.heap.get(id) {
                        Value::Slice(v) => v.get(index),
                        Value::List(v) | Value::Tuple(v) => v.get(index),
                        _ => unreachable!("active sequence"),
                    };
                    if let Some(&item) = item {
                        if index != 0 {
                            append_rendered_text(&mut output, ", ")?;
                        }
                        push_frame(&mut work, Part::Sequence(id, index + 1))?;
                        push_frame(&mut work, Part::Value(item, true))?;
                    }
                }
                Part::Fields(id, index) => {
                    let Value::Instance { class, .. } = ctx.heap.get(id) else {
                        unreachable!("active named tuple")
                    };
                    let Some(names) = ctx.namedtuple_fields(*class) else {
                        unreachable!("active named tuple layout")
                    };
                    if let Some(name) = names.get(index) {
                        if index != 0 {
                            append_rendered_text(&mut output, ", ")?;
                        }
                        append_rendered_text(&mut output, name)?;
                        append_rendered_text(&mut output, "=")?;
                        push_frame(&mut work, Part::Fields(id, index + 1))?;
                        let Value::Tuple(items) = ctx.tuple_backing(id) else {
                            unreachable!("active named tuple storage")
                        };
                        push_frame(&mut work, Part::Value(items[index], true))?;
                    }
                }
                Part::SnapshotMembers(members, index) => {
                    if let Some(&member) = members.get(index) {
                        if index != 0 {
                            append_rendered_text(&mut output, ", ")?;
                        }
                        push_frame(&mut work, Part::SnapshotMembers(members, index + 1))?;
                        push_frame(&mut work, Part::Value(member, true))?;
                    }
                }
                Part::DefaultDictFactory(id, start) => {
                    // A key or value repr may have replaced the factory while rendering the table.
                    let factory = ctx.defaultdict_factory(id).expect("defaultdict storage");
                    let mut contents = String::new();
                    append_rendered_text(&mut contents, &output[start..])?;
                    output.truncate(start);
                    push_frame(&mut work, Part::DefaultDictEnd(contents))?;
                    push_frame(&mut work, Part::DefaultDictName(id, start))?;
                    if enter_ancestor(&mut ancestors, factory)? {
                        push_frame(&mut active, factory)?;
                        push_frame(&mut work, Part::Leave(factory, ""))?;
                        push_frame(&mut work, Part::Value(factory, true))?;
                    } else {
                        append_rendered_text(&mut output, "...")?;
                    }
                }
                Part::DefaultDictName(id, start) => {
                    let name = ctx.type_name(id);
                    output
                        .try_reserve(name.len().saturating_add(1))
                        .map_err(render_allocation)?;
                    output.insert(start, '(');
                    output.insert_str(start, name);
                }
                Part::DefaultDictEnd(contents) => {
                    append_rendered_text(&mut output, ", ")?;
                    append_rendered_text(&mut output, &contents)?;
                    append_rendered_text(&mut output, ")")?;
                }
                Part::Entries(id, index, include_values) => {
                    let entry = match ctx.heap.get(id) {
                        Value::Dict(entries) | Value::DefaultDict { entries, .. } => {
                            entries.at(index)
                        }
                        Value::Set { entries, .. } => {
                            entries.get(index).map(|&(k, v)| (index, k, v))
                        }
                        _ => unreachable!("active dictionary or set"),
                    };
                    if let Some((position, key, value)) = entry {
                        if index != 0 {
                            append_rendered_text(&mut output, ", ")?;
                        }
                        push_frame(&mut work, Part::Entries(id, position + 1, include_values))?;
                        if include_values {
                            push_frame(&mut work, Part::Value(value, true))?;
                            push_frame(&mut work, Part::Text(": "))?;
                        }
                        push_frame(&mut work, Part::Value(key, true))?;
                    }
                }
                Part::SyntaxMessage(id) => {
                    if let Some(text) = element_repr(
                        &mut ctx,
                        id,
                        FormatLookup::Str,
                        &mut work,
                        &active,
                        remaining_sequence_words,
                    )? {
                        append_rendered_text(&mut output, ctx.str_value(text)?)?;
                    } else if let Some(storage) = ctx.string_storage(id) {
                        append_rendered_text(&mut output, ctx.str_value(storage)?)?;
                    } else {
                        push_frame(&mut work, Part::Value(id, false))?;
                    }
                }
                Part::SyntaxEnd(id) => {
                    syntax.remove(&id);
                }
                Part::Suffix(suffix) => append_rendered_text(&mut output, &suffix)?,
                Part::Value(id, repr) => {
                    let native_storage = if checked && repr {
                        ctx.native_repr_storage(id)
                    } else {
                        None
                    };
                    if checked
                        && repr
                        && native_storage.is_none()
                        && let Some(text) = element_repr(
                            &mut ctx,
                            id,
                            FormatLookup::Repr,
                            &mut work,
                            &active,
                            remaining_sequence_words,
                        )?
                    {
                        append_rendered_text(&mut output, ctx.str_value(text)?)?;
                        continue;
                    }
                    let (storage, with_factory) = native_storage.unwrap_or((id, true));
                    if checked
                        && !repr
                        && matches!(
                            ctx.heap.get(ctx.scalar_value(storage)),
                            Value::SurrogateStr(_)
                        )
                    {
                        // This renderer returns scalar UTF-8, so it cannot emit the code points verbatim.
                        return Err(Error::new(
                            "UnicodeError",
                            "operation requires Unicode scalar values",
                        ));
                    }
                    match ctx.heap.get(storage) {
                        Value::Class { name, scope, .. } if ctx.has_base(storage, "enum.Enum") => {
                            let prefix = if ctx.has_base(storage, "enum.Flag") {
                                "<flag "
                            } else {
                                "<enum "
                            };
                            append_rendered_text(&mut output, prefix)?;
                            push_frame(&mut work, Part::Text(">"))?;
                            if let Some(name) = ctx.assigned_class_name(*scope) {
                                push_frame(&mut work, Part::Value(name, true))?;
                            } else {
                                append_rendered_text(
                                    &mut output,
                                    &crate::objects::exceptions::string_repr(name),
                                )?;
                            }
                            continue;
                        }
                        Value::Struct { .. } => {
                            append_rendered_text(&mut output, &stdlib::structure::repr(&ctx, id)?)?;
                            continue;
                        }
                        Value::RegexMatch(m) => {
                            append_rendered_text(&mut output, &m.repr(&ctx)?)?;
                            continue;
                        }
                        Value::Method {
                            callable,
                            receiver,
                            explicit,
                        } if *explicit
                            || matches!(ctx.heap.get(*callable), Value::Function { .. }) =>
                        {
                            let (callable, receiver) = (*callable, *receiver);
                            append_rendered_text(&mut output, "<bound method ")?;
                            push_frame(&mut work, Part::Text(">"))?;
                            push_frame(&mut work, Part::Value(receiver, true))?;
                            if let Value::Function { fields, .. } = ctx.heap.get(callable) {
                                append_rendered_text(&mut output, ctx.str_value(fields.qualname)?)?;
                            } else {
                                let name = element_repr(
                                    &mut ctx,
                                    callable,
                                    FormatLookup::MethodName,
                                    &mut work,
                                    &active,
                                    remaining_sequence_words,
                                )?;
                                append_rendered_text(
                                    &mut output,
                                    name.map(|id| ctx.str_value(id)).transpose()?.unwrap_or("?"),
                                )?;
                            }
                            append_rendered_text(&mut output, " of ")?;
                            continue;
                        }
                        Value::Union(union) => {
                            type_words += 24;
                            if type_words > type_budget {
                                return Err(Error::new(
                                    "RecursionError",
                                    "maximum recursion depth exceeded while getting the repr of an object",
                                ));
                            }
                            let Value::Tuple(args) = ctx.heap.get(union.args) else {
                                unreachable!()
                            };
                            work.try_reserve(args.len().saturating_mul(2).saturating_add(1))
                                .map_err(render_allocation)?;
                            work.push(Part::TypeEnd(id, "", false));
                            for (index, &arg) in args.iter().enumerate().rev() {
                                work.push(if ctx.builtins.get("NoneType") == Some(&arg) {
                                    Part::Text("None")
                                } else {
                                    Part::TypeArgument(arg, false)
                                });
                                if index != 0 {
                                    work.push(Part::Text(" | "));
                                }
                            }
                            continue;
                        }
                        Value::GenericAlias(alias) => {
                            use typing::AliasKind;
                            let python = alias.kind.is_typing_alias()
                                || matches!(alias.kind, AliasKind::Literal(_));
                            if python {
                                ctx.check_call_depth(typing_frames + 2)?;
                                typing_frames += 3;
                            }
                            type_words += if python { 158 } else { 24 };
                            if type_words > type_budget {
                                return Err(Error::new(
                                    "RecursionError",
                                    "maximum recursion depth exceeded while getting the repr of an object",
                                ));
                            }
                            let Value::Tuple(args) = ctx.heap.get(alias.args) else {
                                unreachable!()
                            };
                            work.try_reserve(args.len().saturating_mul(2).saturating_add(6))
                                .map_err(render_allocation)?;
                            work.push(Part::TypeEnd(id, "]", python));
                            let callable = matches!(
                                alias.kind,
                                AliasKind::Callable | AliasKind::AbcCallable
                            ) && !matches!(args.as_slice(), [arg, _] if typing::is_param_expr(&ctx, *arg));
                            if args.is_empty() {
                                if callable {
                                    return Err(Error::new(
                                        "IndexError",
                                        "tuple index out of range",
                                    ));
                                }
                                work.push(Part::Text("()"));
                            }
                            for (index, &arg) in args.iter().enumerate().rev() {
                                work.push(if python {
                                    Part::TypingArgument(arg)
                                } else {
                                    Part::TypeArgument(arg, alias.kind == AliasKind::Builtin)
                                });
                                if callable && index + 1 == args.len() {
                                    work.push(Part::Text("], "));
                                } else if index != 0 {
                                    work.push(Part::Text(", "));
                                }
                            }
                            work.push(Part::Text(if callable { "[[" } else { "[" }));
                            if let Some(name) = alias.name.filter(|_| alias.naming_fields.is_none())
                            {
                                work.push(Part::Value(name, false));
                                work.push(Part::Text("typing."));
                            } else {
                                work.push(match alias.kind {
                                    AliasKind::Callable => Part::Text("typing.Callable"),
                                    AliasKind::AbcCallable if callable => {
                                        Part::Text("collections.abc.Callable")
                                    }
                                    AliasKind::Literal(_) => Part::Text("typing.Literal"),
                                    _ => Part::TypeArgument(alias.origin, false),
                                });
                            }
                            if alias.unpacked {
                                work.push(Part::Text("*"));
                            }
                            continue;
                        }
                        Value::Raised(error) => {
                            if !repr && let Some(fields) = ctx.syntax_members(error) {
                                syntax
                                    .try_reserve(1)
                                    .map_err(|_| Error::new("MemoryError", ""))?;
                                work.try_reserve(3)
                                    .map_err(|_| Error::new("MemoryError", ""))?;
                                if !enter_ancestor(&mut syntax, id)? {
                                    return Err(Error::new(
                                        "RecursionError",
                                        "maximum recursion depth exceeded while getting the str of an object",
                                    ));
                                }
                                let suffix = ctx.syntax_suffix(fields[1], fields[2])?;
                                push_frame(&mut work, Part::SyntaxEnd(id))?;
                                push_frame(&mut work, Part::Suffix(suffix))?;
                                push_frame(&mut work, Part::SyntaxMessage(fields[0]))?;
                                continue;
                            }
                            if !repr
                                && let Some(super::os_error::Args::Python(fields)) =
                                    error.os_args.as_deref()
                                && fields.fragments().is_some()
                            {
                                if !enter_ancestor(&mut ancestors, id)? {
                                    append_rendered_text(&mut output, "...")?;
                                    continue;
                                }
                                push_frame(&mut active, id)?;
                                push_frame(&mut work, Part::Leave(id, ""))?;
                            }
                            ctx.render_exception(error, repr, checked, &mut output, &mut work)?;
                            continue;
                        }
                        Value::Module {
                            scope,
                            constructor: Some(_),
                            ..
                        } => {
                            // The Python formatter can re-enter this module after a callback
                            // changes its spec. Only enclosing containers are active here.
                            if checked && ctx.bootstrap_globals != NONE {
                                let text = element_repr(
                                    &mut ctx,
                                    id,
                                    FormatLookup::ModuleRepr,
                                    &mut work,
                                    &active,
                                    remaining_sequence_words,
                                )?
                                .expect("checked module repr");
                                append_rendered_text(&mut output, ctx.str_value(text)?)?;
                                continue;
                            }
                            if !enter_ancestor(&mut ancestors, id)? {
                                if checked {
                                    return Err(Error::new(
                                        "RecursionError",
                                        "maximum recursion depth exceeded",
                                    ));
                                }
                                append_rendered_text(&mut output, "<module ...>")?;
                                continue;
                            }
                            push_frame(&mut active, id)?;
                            append_rendered_text(&mut output, "<module ")?;
                            push_frame(&mut work, Part::Leave(id, ">"))?;
                            if let Some(file) =
                                ctx.scope_name(*scope, "__file__").filter(|&id| id != NONE)
                            {
                                push_frame(&mut work, Part::Value(file, true))?;
                                push_frame(&mut work, Part::Text(" from "))?;
                            } else if let Some(loader) = ctx
                                .scope_name(*scope, "__loader__")
                                .filter(|&id| id != NONE)
                            {
                                push_frame(&mut work, Part::Text(")"))?;
                                push_frame(&mut work, Part::Value(loader, true))?;
                                push_frame(&mut work, Part::Text(" ("))?;
                            }
                            if let Some(name) = ctx.scope_name(*scope, "__name__") {
                                push_frame(&mut work, Part::Value(name, true))?;
                            } else {
                                push_frame(&mut work, Part::Text("'?'"))?;
                            }
                            continue;
                        }
                        Value::Template {
                            strings,
                            interpolations,
                        } => {
                            append_rendered_text(&mut output, "Template(strings=")?;
                            push_frame(&mut work, Part::Text(")"))?;
                            push_frame(&mut work, Part::Value(*interpolations, true))?;
                            push_frame(&mut work, Part::Text(", interpolations="))?;
                            push_frame(&mut work, Part::Value(*strings, true))?;
                            continue;
                        }
                        Value::Interpolation {
                            value,
                            expression,
                            conversion,
                            format_spec,
                        } => {
                            append_rendered_text(&mut output, "Interpolation(")?;
                            push_frame(&mut work, Part::Text(")"))?;
                            for field in [*format_spec, *conversion, *expression] {
                                push_frame(&mut work, Part::Value(field, true))?;
                                push_frame(&mut work, Part::Text(", "))?;
                            }
                            push_frame(&mut work, Part::Value(*value, true))?;
                            continue;
                        }
                        Value::DictView { .. } if checked => {
                            if !enter_ancestor(&mut ancestors, id)? {
                                append_rendered_text(&mut output, "...")?;
                                continue;
                            }
                            push_frame(&mut active, id)?;
                            append_rendered_text(&mut output, ctx.type_name(id))?;
                            append_rendered_text(&mut output, "(")?;
                            push_frame(&mut work, Part::Leave(id, ")"))?;
                            // Snapshot the view before an element's repr can mutate its dictionary.
                            let list = element_repr(
                                &mut ctx,
                                id,
                                FormatLookup::DictViewList,
                                &mut work,
                                &active,
                                remaining_sequence_words,
                            )?
                            .expect("checked dictionary view snapshot");
                            push_frame(&mut work, Part::Value(list, true))?;
                            continue;
                        }
                        Value::MappingProxy(mapping) => {
                            if repr {
                                append_rendered_text(&mut output, "mappingproxy(")?;
                                push_frame(&mut work, Part::Text(")"))?;
                            }
                            push_frame(&mut work, Part::Value(*mapping, repr))?;
                            continue;
                        }
                        Value::Slice(_) => {
                            if slice_budget.is_some_and(|limit| slice_depth >= limit) {
                                return Err(Error::new(
                                    "RecursionError",
                                    "maximum recursion depth exceeded while getting the repr of an object",
                                ));
                            }
                            slice_depth += 1;
                            append_rendered_text(&mut output, "slice(")?;
                            push_frame(&mut work, Part::SliceEnd)?;
                            push_frame(&mut work, Part::Sequence(id, 0))?;
                            continue;
                        }
                        Value::Instance {
                            scalar: None,
                            dictionary: Some(backing),
                            ..
                        } if ctx.enum_member(id).is_none() => {
                            push_frame(&mut work, Part::Value(*backing, repr))?;
                            continue;
                        }
                        Value::Instance { class, .. }
                            if let Value::Class {
                                name,
                                namedtuple_fields: Some(names),
                                ..
                            } = ctx.heap.get(*class)
                                && let Value::Tuple(items) = ctx.tuple_backing(id)
                                && items.len() >= names.len() =>
                        {
                            if checked {
                                ctx.check_call_depth(namedtuple_depth)?;
                            }
                            namedtuple_depth += 1;
                            append_rendered_text(&mut output, name)?;
                            append_rendered_text(&mut output, "(")?;
                            push_frame(&mut work, Part::NamedTupleEnd)?;
                            push_frame(&mut work, Part::Fields(id, 0))?;
                            continue;
                        }
                        Value::ContextVar { name, default, .. } => {
                            append_rendered_text(&mut output, "<ContextVar name=")?;
                            push_frame(&mut work, Part::ContextEnd(id))?;
                            if let Some(default) = default {
                                push_frame(&mut work, Part::Value(*default, true))?;
                                push_frame(&mut work, Part::Text(" default="))?;
                            }
                            push_frame(&mut work, Part::Value(*name, true))?;
                            continue;
                        }
                        Value::ContextToken { variable, used, .. } => {
                            append_rendered_text(
                                &mut output,
                                if *used {
                                    "<Token used var="
                                } else {
                                    "<Token var="
                                },
                            )?;
                            push_frame(&mut work, Part::ContextEnd(id))?;
                            push_frame(&mut work, Part::Value(*variable, true))?;
                            continue;
                        }
                        _ => {}
                    }
                    if matches!(
                        ctx.heap.get(ctx.scalar_value(storage)),
                        Value::Count { .. } | Value::Repeat(_)
                    ) {
                        push_frame(&mut work, Part::Cursor(id))?;
                        continue;
                    }
                    let (start, end, placeholder, contents) = match ctx.heap.get(storage) {
                        Value::Int(value) => {
                            let text = rendered_integer(value)?;
                            append_rendered_text(&mut output, &text)?;
                            continue;
                        }
                        Value::Range(range) => {
                            append_rendered_text(&mut output, "range(")?;
                            append_rendered_text(&mut output, &rendered_integer(&range.start)?)?;
                            append_rendered_text(&mut output, ", ")?;
                            append_rendered_text(&mut output, &rendered_integer(&range.stop)?)?;
                            if range.step.as_small() != Some(1) {
                                append_rendered_text(&mut output, ", ")?;
                                append_rendered_text(&mut output, &rendered_integer(&range.step)?)?;
                            }
                            append_rendered_text(&mut output, ")")?;
                            continue;
                        }
                        Value::Deque(ring) => {
                            if ancestors.contains(&id) {
                                append_rendered_text(&mut output, "[...]")?;
                                continue;
                            }
                            let end = if let Some(maxlen) = ring.maxlen {
                                push_frame(&mut work, Part::Suffix(format!(", maxlen={maxlen})")))?;
                                "]"
                            } else {
                                "])"
                            };
                            (
                                "deque([",
                                end,
                                "[...]",
                                Part::snapshot(ring.items.iter().copied())?,
                            )
                        }
                        Value::List(_) => ("[", "]", "[...]", Part::Sequence(storage, 0)),
                        Value::Tuple(items) => (
                            "(",
                            if items.len() == 1 { ",)" } else { ")" },
                            "(...)",
                            Part::Sequence(storage, 0),
                        ),
                        Value::SlotMember { owner, name, .. } => {
                            let owner = ctx.class_name(*owner);
                            let field = ctx.str_value(*name)?;
                            append_rendered_text(
                                &mut output,
                                &format!("<member '{field}' of '{owner}' objects>"),
                            )?;
                            continue;
                        }
                        Value::ReadOnlyMember(owner, field) => {
                            append_rendered_text(
                                &mut output,
                                &format!("<member '{field}' of '{owner}' objects>"),
                            )?;
                            continue;
                        }
                        Value::TupleGetterDoc => {
                            append_rendered_text(&mut output, super::namedtuple::DOC_REPR)?;
                            continue;
                        }
                        Value::TupleGetter { index, doc } => {
                            use std::fmt::Write;
                            if !enter_ancestor(&mut ancestors, id)? {
                                return Err(Error::new(
                                    "RecursionError",
                                    "maximum recursion depth exceeded while getting the repr of an object",
                                ));
                            }
                            push_frame(&mut active, id)?;
                            output
                                .try_reserve("_tuplegetter(-9223372036854775808, ".len())
                                .map_err(render_allocation)?;
                            write!(&mut output, "_tuplegetter({index}, ")
                                .expect("the signed field position fits the reserved space");
                            push_frame(&mut work, Part::Leave(id, ")"))?;
                            if let Some(doc) = doc {
                                push_frame(&mut work, Part::Value(*doc, true))?;
                            } else {
                                append_rendered_text(&mut output, "<NULL>")?;
                            }
                            continue;
                        }
                        Value::MethodCaller {
                            name,
                            args,
                            keywords,
                        } => {
                            if !enter_ancestor(&mut ancestors, id)? {
                                append_rendered_text(&mut output, "operator.methodcaller(...)")?;
                                continue;
                            }
                            push_frame(&mut active, id)?;
                            append_rendered_text(&mut output, "operator.methodcaller(")?;
                            push_frame(&mut work, Part::Leave(id, ")"))?;
                            for &(key, value) in ctx.dict_entries(*keywords).unwrap().iter().rev() {
                                push_frame(&mut work, Part::Value(value, true))?;
                                push_frame(&mut work, Part::Text("="))?;
                                push_frame(&mut work, Part::Value(key, false))?;
                                push_frame(&mut work, Part::Text(", "))?;
                            }
                            let Value::Tuple(positional) = ctx.heap.get(*args) else {
                                unreachable!("captured positional tuple")
                            };
                            for &arg in positional.iter().rev() {
                                push_frame(&mut work, Part::Value(arg, true))?;
                                push_frame(&mut work, Part::Text(", "))?;
                            }
                            push_frame(&mut work, Part::Value(*name, true))?;
                            continue;
                        }
                        Value::ItemGetter(keys) => {
                            // Multi-key getters expose this same tuple through __reduce__.
                            let (argument, start, end) = match *ctx.itemgetter_keys(*keys) {
                                [key] => (key, "operator.itemgetter(", ")"),
                                _ => (*keys, "operator.itemgetter", ""),
                            };
                            (
                                start,
                                end,
                                "operator.itemgetter(...)",
                                Part::Value(argument, true),
                            )
                        }
                        Value::Dict(_) => ("{", "}", "{...}", Part::Entries(storage, 0, true)),
                        Value::DefaultDict { .. } => {
                            if with_factory {
                                push_frame(&mut work, Part::DefaultDictFactory(id, output.len()))?;
                            }
                            ("{", "}", "{...}", Part::Entries(storage, 0, true))
                        }
                        Value::Set { entries, frozen } => {
                            let Some(end) = ctx.begin_set_repr(
                                &mut output,
                                id,
                                *frozen || id != storage,
                                entries.is_empty(),
                                ancestors.contains(&id),
                            )?
                            else {
                                continue;
                            };
                            ("{", end, "set(...)", Part::Entries(storage, 0, false))
                        }
                        _ => {
                            append_rendered_text(
                                &mut output,
                                &ctx.format_value(id, repr, &mut active),
                            )?;
                            continue;
                        }
                    };
                    if !enter_ancestor(&mut ancestors, id)? {
                        append_rendered_text(&mut output, placeholder)?;
                        continue;
                    }
                    if matches!(
                        ctx.heap.get(storage),
                        Value::List(_) | Value::Tuple(_) | Value::Deque(_)
                    ) && remaining_sequence_words == Some(0)
                    {
                        return Err(Error::new(
                            "RecursionError",
                            "maximum recursion depth exceeded while getting the repr of an object",
                        ));
                    }
                    if matches!(ctx.heap.get(storage), Value::Deque(_)) {
                        extra_deque_words += EXTRA_DEQUE_FRAME_WORDS;
                    }
                    push_frame(&mut active, id)?;
                    append_rendered_text(&mut output, start)?;
                    push_frame(&mut work, Part::Leave(id, end))?;
                    match contents {
                        Part::Entries(_, _, false) if checked => {
                            // CPython renders a snapshot so a hook can mutate the original set.
                            let Value::Set { entries, .. } = ctx.heap.get(storage) else {
                                unreachable!()
                            };
                            push_frame(
                                &mut work,
                                Part::snapshot(entries.iter().map(|&(key, _)| key))?,
                            )?;
                        }
                        part => push_frame(&mut work, part)?,
                    }
                }
            }
        }
        Ok(output)
    }

    fn format_elements(&self, values: &[Id], active: &mut Vec<Id>) -> String {
        values
            .iter()
            .map(|&arg| self.format_value(arg, true, active))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Escaping exceptions retain their type and frames if their arguments cannot render.
    pub(super) fn exception_message(&self, error: &Error) -> String {
        Self::render_containers(
            self,
            RenderRoot::Exception(error),
            false,
            false,
            |_, _, _, _, _, _| Ok(None),
        )
        .unwrap_or_else(|_| "<exception str() failed>".to_owned())
    }

    /// Render the receiver's identity without invoking its own __repr__ slot.
    pub(super) fn object_repr(&mut self, receiver: Id) -> Result<Id> {
        let class = self.exact_type(receiver)?;
        Ok(self.text(self.default_object_repr(receiver, class)))
    }

    /// Spell the default repr using the actual receiver, even when its class overrides repr.
    fn default_object_repr(&self, receiver: Id, class: Id) -> String {
        let name = match self.heap.get(class) {
            Value::Class { scope, .. } => self.qualified_class_name(class, *scope),
            Value::Native(name) => name.clone(),
            _ => self.type_name(receiver).to_owned(),
        };
        crate::objects::base_object_repr(&name, receiver).to_string()
    }

    pub(super) fn qualified_class_name(&self, class: Id, scope: Id) -> String {
        let module = match self.namespace_binding(self.scope_data(scope), "__module__") {
            Some(value) => self.str_value(value).ok(),
            None => Some(self.scope_data(scope).module.as_str()),
        };
        let name = self.class_name(class);
        match module {
            None | Some("builtins") => name.to_owned(),
            Some(module) => {
                let qualname = self
                    .namespace_binding(self.scope_data(scope), "__qualname__")
                    .and_then(|value| self.str_value(value).ok())
                    .unwrap_or(name);
                format!("{module}.{qualname}")
            }
        }
    }

    /// Bytearray repr includes the receiver's class name even for an explicit base slot call.
    pub(super) fn bytearray_repr(&self, object: Id, bytes: &[u8]) -> String {
        let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
            '"'
        } else {
            '\''
        };
        let mut output = format!("{}(b{quote}", self.type_name(object));
        for &byte in bytes {
            if byte == b'"' {
                output.push('"');
            } else {
                output.extend(std::ascii::escape_default(byte).map(char::from));
            }
        }
        output.push(quote);
        output.push(')');
        output
    }

    /// Keeps recursive container arguments on the same formatting stack.
    fn format_value(&self, id: Id, repr: bool, active: &mut Vec<Id>) -> String {
        let value = self.heap.get(id);
        if matches!(self.heap.get(self.scalar_value(id)), Value::Super { .. }) {
            return self.super_repr(id);
        }
        if matches!(
            self.heap.get(self.scalar_value(id)),
            Value::Count { .. } | Value::Repeat(_)
        ) {
            return self.repr(id);
        }
        if let Value::HandlerLock(lock) = self.heap.get(self.scalar_value(id)) {
            let (state, owner, depth) = match lock.state() {
                Some((owner, depth)) => ("locked", owner, depth),
                None => ("unlocked", 0, 0),
            };
            let class = self.type_name(id);
            return format!("<{state} {class} object owner={owner} count={depth} at {id:#x}>");
        }

        if let Value::Instance { class, .. } = value
            && self.classes.get("re.RegexFlag") == Some(class)
            && let Some((_, name, bits)) = self.enum_member(id)
        {
            return match self.str_value(name) {
                Ok(names) => names
                    .split('|')
                    .map(|name| {
                        if name.starts_with("0x") {
                            name.to_owned()
                        } else {
                            format!("re.{name}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("|"),
                Err(_) => format!("re.RegexFlag({})", self.repr(bits)),
            };
        }
        if let Value::Instance { scalar, .. } = value
            && (repr || scalar.is_none())
            && let Some((class, name, member_value)) = self.enum_member(id)
        {
            let name = self.format_value(name, false, active);
            return if repr {
                format!(
                    "<{class}.{name}: {}>",
                    self.format_value(member_value, true, active)
                )
            } else {
                format!("{class}.{name}")
            };
        }
        let recursive = match value {
            Value::List(_) => Some("[...]"),
            Value::Tuple(_) => Some("(...)"),
            Value::Dict(_) => Some("{...}"),
            Value::ItemGetter(_) => Some("operator.itemgetter(...)"),
            _ => None,
        };
        if let Some(placeholder) = recursive {
            if active.contains(&id) {
                return placeholder.to_owned();
            }
            active.push(id);
        }
        let result = match value {
            Value::ContextVar {
                name,
                default,
                repr_active,
            } => {
                if repr_active.replace(true) {
                    return "...".to_owned();
                }
                let default = default.map_or_else(String::new, |v| {
                    format!(" default={}", self.format_value(v, true, active))
                });
                let repr = format!(
                    "<ContextVar name={}{default} at {id:#x}>",
                    self.format_value(*name, true, active)
                );
                repr_active.set(false);
                repr
            }
            Value::ContextToken { variable, used, .. } => {
                let state = if *used { " used" } else { "" };
                format!(
                    "<Token{state} var={} at {id:#x}>",
                    self.format_value(*variable, true, active)
                )
            }
            Value::Template { .. } | Value::Interpolation { .. } => self.repr(id),
            Value::TemplateIter { .. } => {
                format!("<{} object at {id:#x}>", super::value::TEMPLATE_ITER_TYPE)
            }
            Value::Scandir(scan) => super::stdlib::os::scandir::iterator_repr(scan),
            Value::DirEntry(entry) => {
                format!("<DirEntry {}>", self.format_value(entry.name, true, active))
            }
            Value::ThreadHandle(handle) => {
                format!("<_thread._ThreadHandle object: ident={}>", handle.ident())
            }
            Value::PrimitiveLock(lock) => {
                let state = if lock.locked() { "locked" } else { "unlocked" };
                format!("<{state} _thread.lock object at {id:#x}>")
            }
            Value::Hash(state) => state.lock().repr(id as usize).to_string(),
            Value::Context(_) => format!("<_contextvars.Context object at {id:#x}>"),
            Value::TokenMissing => "<Token.MISSING>".to_owned(),
            Value::PartialPlaceholder => "Placeholder".to_owned(),
            Value::None => "None".into(),
            Value::Ellipsis => "Ellipsis".into(),
            Value::NotImplemented => "NotImplemented".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Int(n) => n.to_string(),
            Value::RegexPattern(pattern) => pattern.repr(self),
            Value::Float(n) => crate::floats::render(*n).to_string(),
            Value::Complex(z) => z.repr(),
            Value::Calendar { value, .. } if repr => value.repr(),
            Value::Calendar { value, .. } => value.text(),
            Value::NamedTuple {
                class: Some(class),
                names,
                values,
                ..
            } => {
                let fields = names
                    .iter()
                    .zip(values)
                    .map(|(k, v)| format!("{k}={}", self.format_value(*v, true, active)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{class}({fields})")
            }
            Value::SurrogateStr(points) => surrogate_repr(points),
            Value::Str(s) if repr => crate::objects::exceptions::string_repr(s).into_string(),
            Value::Str(s) => s.to_string(),
            Value::Instance { class, .. } if let Some(name) = self.heap.get(*class).hash_type() => {
                format!("<{name} object at {id:#x}>")
            }
            Value::Instance {
                scalar: Some(payload),
                ..
            } => self.format_value(*payload, repr, active),
            Value::Bytes(bytes) => {
                let mut output = String::new();
                crate::objects::write_bytes_repr(&mut output, bytes);
                output
            }
            Value::ByteArray(bytes) => self.bytearray_repr(id, bytes),
            Value::MemoryView { owner, .. } => {
                let state = if owner.is_some() { "" } else { "released " };
                format!("<{state}memory at {id:#x}>")
            }
            Value::Instance {
                dictionary: Some(backing),
                ..
            } => self.format_value(*backing, repr, active),
            Value::Dict(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        self.format_value(*k, true, active),
                        self.format_value(*v, true, active)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Value::Slice(parts) => format!("slice({})", self.format_elements(parts, active)),
            Value::Set { entries, frozen } => {
                if entries.is_empty() {
                    return if *frozen { "frozenset()" } else { "set()" }.to_owned();
                }
                let body = entries
                    .iter()
                    .map(|&(key, _)| self.format_value(key, true, active))
                    .collect::<Vec<_>>()
                    .join(", ");
                if *frozen {
                    format!("frozenset({{{body}}})")
                } else {
                    format!("{{{body}}}")
                }
            }
            Value::List(v) | Value::Tuple(v) => {
                let body = self.format_elements(v, active);
                match self.heap.get(id) {
                    Value::List(_) => format!("[{body}]"),
                    _ if v.len() == 1 => format!("({body},)"),
                    _ => format!("({body})"),
                }
            }
            Value::Raised(error) => Self::render_containers(
                self,
                RenderRoot::Exception(error),
                repr,
                false,
                |_, _, _, _, _, _| Ok(None),
            )
            .unwrap_or_else(|_| {
                if repr {
                    "<exception repr() failed>"
                } else {
                    "<exception str() failed>"
                }
                .to_owned()
            }),
            Value::GenericAlias(_) | Value::Union(_) => self.repr(id),
            Value::Class { name, scope, .. } if self.scope_data(*scope).module == "typing" => {
                format!("<class 'typing.{name}'>")
            }
            Value::Class { name, scope, .. }
                if matches!(
                    self.scope_data(*scope).module.as_str(),
                    "_collections_abc" | "collections.abc"
                ) =>
            {
                format!("<class 'collections.abc.{name}'>")
            }
            Value::Native(name) if matches!(name.as_str(), "typing.Any" | "typing.Callable") => {
                name.clone()
            }
            Value::TypeVarTuple { name, .. } | Value::TypeAlias { name, .. } => self.display(*name),
            Value::NoDefault => "typing.NoDefault".to_owned(),
            Value::TypeVar(var) => {
                let variance = if var.covariant {
                    "+"
                } else if var.contravariant {
                    "-"
                } else if var.infer_variance {
                    ""
                } else {
                    "~"
                };
                format!("{variance}{}", self.display(var.name))
            }
            Value::GetSetDescriptor { owner, name, .. }
                if matches!(
                    *owner,
                    "code" | "int" | "float" | "wrapper_descriptor" | "getset_descriptor"
                ) || crate::objects::HashState::is_provider_class(owner)
                    || *owner == super::value::TEMPLATE_TYPE
                    || stdlib::datetime::members(owner).is_some() =>
            {
                format!("<attribute '{name}' of '{owner}' objects>")
            }
            Value::LayoutDescriptor(owner, name) => format!(
                "<attribute '{name}' of '{}' objects>",
                self.class_name(*owner)
            ),
            Value::Class { scope, .. } => {
                format!("<class '{}'>", self.qualified_class_name(id, *scope))
            }
            Value::Method {
                callable,
                receiver,
                explicit: false,
            } if let Value::Native(symbol) = self.heap.get(*callable)
                && let Some(slot) =
                    super::wrapper_descriptor::BuiltinDescriptor::lookup(symbol)
                && slot.class() == "wrapper_descriptor" =>
            {
                slot.bound_repr(self, *receiver)
            }
            Value::Method {
                callable,
                explicit: false,
                receiver,
            } if let Value::Native(symbol) = self.heap.get(*callable)
                && matches!(self.heap.get(*receiver), Value::Module { .. })
                && let Some((_, leaf)) = symbol.rsplit_once('.') =>
            {
                format!("<built-in function {leaf}>")
            }
            Value::HeapTypeMethod {
                name,
                owner,
                slot_wrapper,
            } => {
                let class = self.heap.get(*owner).heap_type().unwrap();
                let kind = if *slot_wrapper {
                    "slot wrapper"
                } else {
                    "method"
                };
                format!("<{kind} '{name}' of '{class}' objects>")
            }
            Value::Method {
                callable, receiver, ..
            } if let Value::HeapTypeMethod {
                name,
                owner,
                slot_wrapper,
            } = self.heap.get(*callable) =>
            {
                let class = self.heap.get(*owner).heap_type().unwrap();
                if matches!(self.heap.get(*receiver), Value::Module { .. }) {
                    format!("<built-in function {name}>")
                } else if *name == "__new__" {
                    format!("<built-in method __new__ of type object at {receiver:#x}>")
                } else if *slot_wrapper {
                    format!("<method-wrapper '{name}' of {class} object at {receiver:#x}>")
                } else {
                    format!("<built-in method {name} of {class} object at {receiver:#x}>")
                }
            }
            Value::Native(name)
                if matches!(name.as_str(), "binascii.a2b_hex" | "binascii.unhexlify") =>
            {
                format!("<built-in function {}>", name.rsplit('.').next().unwrap())
            }
            Value::Native(name)
                if let Some((_, function)) = super::calls::module_function(name) =>
            {
                format!("<built-in function {function}>")
            }
            Value::Native(name)
                if let Some(owner) = builtins::constructor_owner(name)
                    && let Some(target) = self.builtins.get(owner) =>
            {
                format!("<built-in method __new__ of type object at {target:#x}>")
            }
            Value::Native(name) => super::wrapper_descriptor::BuiltinDescriptor::lookup(name)
                .map(|slot| slot.repr())
                .unwrap_or_else(|| format!("<class '{name}'>")),
            Value::Module {
                constructor: Some(_),
                ..
            } => self.repr(id),
            Value::Module { name, .. } => format!("<module '{name}'>"),
            Value::StaticMethod { callable, .. } => {
                format!(
                    "<staticmethod({})>",
                    self.format_value(*callable, true, active)
                )
            }
            Value::Cached { .. } => format!("<functools._lru_cache_wrapper object at {id:#x}>"),
            Value::Function { fields, .. } => {
                format!(
                    "<function {} at {id:#x}>",
                    self.format_value(fields.qualname, false, active)
                )
            }
            Value::AttrGetter(paths) => {
                let arguments = paths
                    .iter()
                    .map(|path| {
                        crate::objects::exceptions::string_repr(&self.attrgetter_name(path))
                            .into_string()
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("operator.attrgetter({arguments})")
            }
            Value::ItemGetter(keys) => format!(
                "operator.itemgetter({})",
                self.format_elements(self.itemgetter_keys(*keys), active)
            ),
            Value::Instance { class, .. } => self.default_object_repr(id, *class),
            Value::Socket { handle: socket, .. } => {
                use crate::objects::socket::{family, fd, kind, proto};
                format!(
                    "<socket object, fd={}, family={}, type={}, proto={}>",
                    fd(socket),
                    family(socket),
                    kind(socket),
                    proto(socket),
                )
            }
            _ => "<object>".to_owned(),
        };
        if recursive.is_some() {
            active.pop();
        }
        result
    }
    pub(super) fn exact_sequence_item(&self, obj: Id, index: &crate::Int) -> Option<Id> {
        let (Value::List(items) | Value::Tuple(items)) = self.heap.get(obj) else {
            return None;
        };
        items.get(usize::try_from(index.as_small()?).ok()?).copied()
    }

    pub(super) fn get_item(&mut self, obj: Id, key: Id) -> Result<Id> {
        if let Value::Int(index) = self.heap.get(key)
            && let Some(item) = self.exact_sequence_item(obj, index)
        {
            return Ok(item);
        }
        #[cfg(target_os = "linux")]
        if matches!(self.heap.get(obj), Value::Mmap { .. }) {
            return super::stdlib::mmap::get_item(self, obj, key);
        }
        if matches!(self.heap.get(obj), Value::MemoryView { .. }) {
            return self.memoryview_get(obj, key);
        }
        let target = self.weak_proxy_target(obj)?;
        if target != obj {
            return self.with_weak_target(target, |ctx| ctx.get_item(target, key));
        }
        if matches!(self.heap.get(obj), Value::Deque(_)) {
            return deque::IndexOp::Get.apply(self, obj, key, false);
        }
        if let Value::RegexMatch(m) = self.heap.get(obj) {
            return m.clone().group_item(self, key);
        }
        if let Some(value) = super::union::subscribe(self, obj, key) {
            return value;
        }
        if let Some(getter) = self.bound_slot(obj, "__getitem__")? {
            return self.call(getter, vec![key], Vec::new());
        }
        if self.enum_values(obj).is_some() {
            let names = self
                .class_attr(obj, "_member_map_")
                .filter(|&names| matches!(self.heap.get(names), Value::Dict(_)))
                .ok_or_else(|| Error::new("TypeError", "enum member names must be a dictionary"))?;
            return self.get_item(names, key);
        }
        let alias = super::typing::alias_payload(self, obj).unwrap_or(obj);
        if let Some(value) = super::typing::subscribe(self, alias, key) {
            return value;
        }
        if matches!(self.heap.get(obj), Value::MappingProxy(_)) {
            return self.get_item(self.mapping_proxy_target(obj), key);
        }
        if let Value::Context(bindings) = self.heap.get(obj) {
            self.context_key(key)?;
            return bindings
                .get(&key)
                .copied()
                .ok_or_else(|| self.lookup_error("KeyError", key));
        }
        if matches!(
            self.heap.get(obj),
            Value::Dict(_) | Value::DefaultDict { .. }
        ) {
            return self.dictionary_item(obj, key);
        }
        self.sequence_subscript(obj, key)
    }

    /// Read a native sequence directly once Python's subscription slots have been resolved.
    pub(super) fn sequence_subscript(&mut self, obj: Id, key: Id) -> Result<Id> {
        if let Value::Slice(parts) = *self.heap.get(key) {
            return self.slice(obj, parts);
        }
        let storage = self.scalar_value(obj);
        let kind = match self.heap.get(storage) {
            Value::List(_) => "list",
            Value::Tuple(_) | Value::NamedTuple { .. } => "tuple",
            Value::Str(_) | Value::SurrogateStr(_) => "string",
            Value::Bytes(_) => "byte",
            Value::ByteArray(_) => "bytearray",
            Value::Array(_) => "array",
            Value::Range(_) => "range",
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("'{}' object is not subscriptable", self.type_name(obj)),
                ));
            }
        };
        let index = self.index_value(key)?.ok_or_else(|| {
            let key_type = self.type_name(key);
            let message = if kind == "string" {
                format!("string indices must be integers, not '{key_type}'")
            } else {
                format!("{kind} indices must be integers or slices, not {key_type}")
            };
            Error::new("TypeError", message)
        })?;
        let bounds_error = || {
            let message = match kind {
                "byte" => "index out of range".to_owned(),
                "range" => "range object index out of range".to_owned(),
                _ => format!("{kind} index out of range"),
            };
            Error::new("IndexError", message)
        };
        if let Value::Range(range) = self.heap.get(storage) {
            let index = index.big();
            let len = range.len.big();
            let offset = if index.is_negative() {
                index.as_ref() + len.as_ref()
            } else {
                index.into_owned()
            };
            if offset.is_negative() || offset >= *len {
                return Err(bounds_error());
            }
            return Ok(self.alloc(Value::Int(range.at(&offset))));
        }
        let offset = i128::from(index.to_i64().ok_or_else(|| {
            Error::new(
                "IndexError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    self.type_name(key)
                ),
            )
        })?);
        // __index__ can resize a list or bytearray. Read its current storage after the call.
        let position = |len: usize| {
            let i = if offset < 0 {
                len as i128 + offset
            } else {
                offset
            };
            usize::try_from(i)
                .ok()
                .filter(|&i| i < len)
                .ok_or_else(bounds_error)
        };
        Ok(match self.heap.get(storage) {
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => {
                v[position(v.len())?]
            }
            Value::SurrogateStr(points) => {
                let point = points[position(points.len())?];
                self.text_codepoints(vec![point])
            }
            Value::Str(s) if self.heap.is_ascii_str(storage) => {
                let character = char::from(s.as_bytes()[position(s.len())?]);
                self.text(character.to_string())
            }
            Value::Str(s) => {
                let index = isize::try_from(offset).map_err(|_| bounds_error())?;
                let character = if index < 0 {
                    s.chars().nth_back(index.unsigned_abs() - 1)
                } else {
                    s.chars().nth(index as usize)
                }
                .ok_or_else(bounds_error)?;
                self.text(character.to_string())
            }
            Value::Array(array) => {
                let item = array.at(position(array.len())?)?;
                self.alloc(item)
            }
            Value::Bytes(v) | Value::ByteArray(v) => self.int(i64::from(v[position(v.len())?])),
            _ => unreachable!(),
        })
    }
    /// Call the current factory, preserving any value it inserts for this key.
    /// Look up a builtin mapping key before dispatching the original object's missing hook.
    pub(super) fn dictionary_item(&mut self, object: Id, key: Id) -> Result<Id> {
        let storage = self.dict_storage(object).expect("dictionary");
        if let Some(value) = self.dictionary_value(storage, key)? {
            return Ok(value);
        }
        if let Some(hook) = self.bound_slot(object, "__missing__")? {
            return self.call(hook, vec![key], Vec::new());
        }
        if matches!(self.heap.get(storage), Value::DefaultDict { .. }) {
            return self.defaultdict_missing(object, key);
        }
        Err(self.lookup_error("KeyError", key))
    }

    pub(super) fn defaultdict_missing(&mut self, obj: Id, key: Id) -> Result<Id> {
        let Some(factory) = self.defaultdict_factory(obj) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '__missing__' for 'collections.defaultdict' objects doesn't apply to a '{}' object",
                    self.type_name(obj)
                ),
            ));
        };
        if factory == NONE {
            return Err(self.lookup_error("KeyError", key));
        }
        let value = self.call(factory, Vec::new(), Vec::new())?;
        let storage = self.dict_storage(obj).expect("validated defaultdict");
        self.set_dictionary_key(storage, key, value, None, false)
    }

    /// Convert a scalar subscript before looking up a mutable sequence's current length.
    fn mutation_index(&mut self, key: Id, kind: &str) -> Result<i64> {
        let index = self.index_value(key)?.ok_or_else(|| {
            if kind == "array" {
                return Error::new("TypeError", "array indices must be integers");
            }
            Error::new(
                "TypeError",
                format!(
                    "{kind} indices must be integers or slices, not {}",
                    self.type_name(key)
                ),
            )
        })?;
        index.to_i64().ok_or_else(|| {
            Error::new(
                "IndexError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    self.type_name(key)
                ),
            )
        })
    }

    /// Resolve a list position after the index callback has had a chance to resize it.
    fn list_mutation_position(&mut self, obj: Id, key: Id) -> Result<usize> {
        let offset = self.mutation_index(key, "list")?;
        let Value::List(items) = self.heap.get(obj) else {
            unreachable!()
        };
        let len = items.len() as i64;
        let index = if offset < 0 { offset + len } else { offset };
        if !(0..len).contains(&index) {
            return Err(Error::new(
                "IndexError",
                "list assignment index out of range",
            ));
        }
        Ok(index as usize)
    }

    pub(super) fn put_item(&mut self, obj: Id, key: Id, val: Id) -> Result<()> {
        #[cfg(target_os = "linux")]
        if matches!(self.heap.get(obj), Value::Mmap { .. }) {
            return super::stdlib::mmap::put_item(self, obj, key, val);
        }
        if matches!(self.heap.get(obj), Value::MemoryView { .. }) {
            if let Some(window) = self.raw_read_window(obj) {
                window.checked_range(self.owner_byte_len(window.backing)?)?;
                // An index or value conversion can try to resize the unpinned backing.
                return self
                    .with_buffer_export(window.backing, |ctx| ctx.memoryview_set(obj, key, val));
            }
            return self.memoryview_set(obj, key, val);
        }
        let target = self.weak_proxy_target(obj)?;
        if target != obj {
            return self.with_weak_target(target, |ctx| ctx.put_item(target, key, val));
        }
        if matches!(self.heap.get(obj), Value::Array(_))
            && !matches!(self.heap.get(key), Value::Slice(_))
        {
            let index = self.mutation_index(key, "array")?;
            return super::array::set_item(self, obj, index, val);
        }
        if matches!(self.heap.get(obj), Value::Deque(_)) {
            return deque::IndexOp::Set(val)
                .apply(self, obj, key, false)
                .map(|_| ());
        }
        if matches!(
            self.heap.get(obj),
            Value::Dict(_) | Value::DefaultDict { .. }
        ) {
            self.set_dictionary_key(obj, key, val, None, true)?;
        } else if matches!(self.heap.get(obj), Value::ByteArray(_)) {
            if let Value::Slice(parts) = *self.heap.get(key) {
                return self.set_slice(obj, parts, Some(val));
            }
            let i = self.mutation_index(key, "bytearray")?;
            let byte = self.byte_value(val)?;
            let Value::ByteArray(data) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            let i = if i < 0 { data.len() as i64 + i } else { i };
            *data
                .get_mut(i as usize)
                .ok_or_else(|| Error::new("IndexError", "bytearray index out of range"))? = byte;
        } else if matches!(self.heap.get(obj), Value::List(_)) {
            if let Value::Slice(parts) = *self.heap.get(key) {
                return self.set_list_slice(obj, parts, Some(val));
            }
            let i = self.list_mutation_position(obj, key)?;
            let Value::List(items) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            let previous = std::mem::replace(&mut items[i], val);
            if previous != val {
                self.heap.weak_edge_removed(previous);
            }
        } else if let Some(setter) = self.bound_slot(obj, "__setitem__")? {
            self.call(setter, vec![key, val], Vec::new())?;
        } else if let Some(storage) = self.mutable_sequence_storage(obj) {
            return self.put_item(storage, key, val);
        } else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "'{}' object does not support item assignment",
                    self.type_name(obj)
                ),
            ));
        }
        Ok(())
    }
    pub(super) fn contains(&mut self, item: Id, container: Id) -> Result<bool> {
        let container = self.mapping_proxy_target(container);
        if let Some(hook) = self.bound_slot(container, "__contains__")? {
            if hook == NONE {
                return Err(Error::new(
                    "TypeError",
                    format!("'{}' object is not a container", self.type_name(container)),
                ));
            }
            let result = self.call(hook, vec![item], Vec::new())?;
            return self.truth(result);
        }
        if self.enum_values(container).is_some() && self.has_base(container, "enum.Flag") {
            return self.flag_contains(container, item);
        }
        if let Value::Context(bindings) = self.heap.get(container) {
            self.context_key(item)?;
            return Ok(bindings.contains_key(&item));
        }
        if let Value::Range(range) = self.heap.get(container)
            && matches!(self.heap.get(item), Value::Int(_) | Value::Bool(_))
        {
            return Ok(range.index_of(&self.as_int(item)?).is_some());
        }
        let scalar = self.scalar_value(container);
        if let Value::SurrogateStr(hay) = self.heap.get(scalar) {
            let needle = self.string_codepoints(item).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "'in <string>' requires string as left operand, not {:.100}",
                        self.type_name(item)
                    ),
                )
            })?;
            return Ok(needle.is_empty()
                || hay
                    .windows(needle.len())
                    .any(|part| part == needle.as_ref()));
        }
        if let Value::Str(hay) = self.heap.get(scalar) {
            if matches!(
                self.heap.get(self.scalar_value(item)),
                Value::SurrogateStr(_)
            ) {
                return Ok(false);
            }
            let needle = self.str_value(item).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "'in <string>' requires string as left operand, not {:.100}",
                        self.type_name(item)
                    ),
                )
            })?;
            return Ok(hay.contains(needle));
        }
        if let Value::ByteArray(data) = self.heap.get(scalar)
            && matches!(self.heap.get(item), Value::MemoryView { .. })
        {
            return Ok(crate::objects::bytes_contains(data, self.bytes_like(item)?));
        }
        if matches!(self.heap.get(scalar), Value::Bytes(_)) {
            // CPython falls back to the buffer protocol when integer conversion fails.
            let number = self.index_value(item).ok().flatten();
            let Value::Bytes(data) = self.heap.get(scalar) else {
                unreachable!()
            };
            if let Some(number) = number {
                return Ok(crate::objects::bytes_contains_int(data, &number)?);
            }
            return Ok(crate::objects::bytes_contains(data, self.bytes_like(item)?));
        }
        if matches!(self.heap.get(container), Value::Set { .. }) {
            return builtins::set_member(self, container, item, true);
        }
        if matches!(
            self.heap.get(container),
            Value::Dict(_) | Value::DefaultDict { .. }
        ) {
            return Ok(self.find_dictionary_key(container, item, None)?.1.is_some());
        }
        let iterator = self.iterator(container).map_err(|error| {
            let ty = self.builtin("TypeError");
            if self.raised_error_matches(&error, ty) {
                Error::new(
                    "TypeError",
                    format!(
                        "argument of type '{}' is not a container or iterable",
                        self.type_name(container)
                    ),
                )
            } else {
                error
            }
        })?;
        self.heap.keep(iterator);
        self.contains_next(item, |ctx| ctx.next_item(iterator))
    }

    /// Compare candidates in order and release rich-comparison temporaries between calls.
    pub(super) fn contains_next(
        &mut self,
        item: Id,
        mut next: impl FnMut(&mut Self) -> Result<Option<Id>>,
    ) -> Result<bool> {
        let mark = self.heap.temporary_mark();
        while let Some(value) = next(self)? {
            if self.sequence_item_matches(value, item, mark)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// Keep each candidate alive through user equality and truth, then release comparison temporaries.
    pub(super) fn sequence_item_matches(
        &mut self,
        value: Id,
        item: Id,
        mark: usize,
    ) -> Result<bool> {
        if value == item {
            return Ok(true);
        }
        self.heap.keep(value);
        let result = self
            .binary("==", value, item)
            .and_then(|equal| self.truth(equal));
        self.heap.release_temporaries_since(mark);
        result
    }

    /// Read an ordinary sequence slice directly when its bounds need no Python callbacks.
    pub(super) fn get_slice(
        &mut self,
        receiver: Id,
        start: Id,
        stop: Id,
        slot: Option<usize>,
    ) -> Result<Id> {
        let parts = [start, stop, NONE];
        if matches!(
            self.heap.get(receiver),
            Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::Bytes(_)
                | Value::Tuple(_)
                | Value::List(_)
        ) && [start, stop].iter().all(|&id| {
            matches!(
                self.heap.get(id),
                Value::None | Value::Int(_) | Value::Bool(_)
            )
        }) {
            return self.slice(receiver, parts);
        }
        let key = if let Some(slot) = slot {
            self.heap
                .cached_constant(super::value::ConstantSlot::Linked(slot), |heap| {
                    Ok(heap.alloc(Value::Slice(parts)))
                })?
        } else {
            self.alloc(Value::Slice(parts))
        };
        self.get_item(receiver, key)
    }

    fn slice(&mut self, receiver: Id, parts: [Id; 3]) -> Result<Id> {
        let obj = self.scalar_value(receiver);
        if let Value::Range(range) = self.heap.get(obj) {
            return super::slice::range(self, parts, range.clone());
        }
        if !matches!(
            self.heap.get(obj),
            Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
                | Value::Array(_)
                | Value::List(_)
                | Value::Tuple(_)
                | Value::NamedTuple { .. }
        ) {
            return Err(Error::new(
                "TypeError",
                format!("'{}' object is not subscriptable", self.type_name(obj)),
            ));
        }
        let bounds = super::slice::sequence_components(self, parts)?;
        let len = match self.heap.get(obj) {
            Value::Str(text) if self.heap.is_ascii_str(obj) => text.len(),
            Value::Str(text) => {
                let lower = text.chars().size_hint().0;
                // Only a stop strictly below this bound proves a proper prefix.
                // At equality the slice could contain the entire string.
                match bounds {
                    [start, Some(stop), step]
                        if start.unwrap_or(0) >= 0
                            && step.unwrap_or(1) > 0
                            && (0..lower as i64).contains(&stop) =>
                    {
                        lower
                    }
                    _ => text.chars().count(),
                }
            }
            Value::SurrogateStr(points) => points.len(),
            Value::Array(array) => array.len(),
            Value::Bytes(data) | Value::ByteArray(data) => data.len(),
            Value::List(items) | Value::Tuple(items) | Value::NamedTuple { values: items, .. } => {
                items.len()
            }
            _ => unreachable!("validated sequence"),
        };
        let [start, end, step] = super::slice::normalize(bounds, len as i64);
        let count = slice_count(start, end, step);
        let positions = slice_positions(start, end, step);
        let full_forward = start == 0 && step == 1 && count == len;
        let result = match self.heap.get(obj) {
            Value::Str(_) | Value::SurrogateStr(_) | Value::Tuple(_)
                if receiver == obj && full_forward =>
            {
                return Ok(obj);
            }
            Value::SurrogateStr(points) => {
                Value::from_codepoints(sliced_values(points, positions, count)?)
            }
            Value::Str(text) => {
                let stride = usize::try_from(step.unsigned_abs()).unwrap_or(usize::MAX);
                let value = if step == 1 && len == text.len() {
                    // The normalized length never exceeds the character count.
                    // Equality with the byte length proves every character is ASCII.
                    let start = start as usize;
                    let mut value = String::new();
                    value.try_reserve_exact(count).map_err(slice_memory_error)?;
                    value.push_str(&text[start..start + count]);
                    value
                } else if step > 0 {
                    sliced_text(text.chars().skip(start as usize), stride, count)?
                } else {
                    let skip = (len as i64 - 1 - start) as usize;
                    sliced_text(text.chars().rev().skip(skip), stride, count)?
                };
                Value::Str(value.into())
            }
            Value::Bytes(data) if step == 1 => {
                if receiver == obj && count == len {
                    return Ok(obj);
                }
                let start = start as usize;
                let mut bytes = Vec::new();
                bytes.try_reserve_exact(count).map_err(slice_memory_error)?;
                bytes.extend_from_slice(&data[start..start + count]);
                return Ok(self.heap.bytes_from_data(bytes));
            }
            Value::Bytes(data) => Value::Bytes(sliced_values(data, positions, count)?),
            Value::ByteArray(data) => Value::ByteArray(sliced_values(data, positions, count)?),
            Value::Array(array) => Value::Array(array.sliced(start, step, count)?),
            Value::List(items) => Value::List(sliced_values(items, positions, count)?),
            Value::Tuple(items) | Value::NamedTuple { values: items, .. } => {
                Value::Tuple(sliced_values(items, positions, count)?)
            }
            _ => unreachable!("validated sequence"),
        };
        // Extended slices and full subclass copies bypass the single-character cache.
        if count == 1 && (step != 1 || full_forward) && matches!(result, Value::Str(_)) {
            return self.heap.fresh_string(result);
        }
        Ok(self.alloc(result))
    }

    /// Borrow disjoint arrays directly and finish arbitrary iterators before normalizing.
    fn set_list_slice(&mut self, obj: Id, parts: [Id; 3], source: Option<Id>) -> Result<()> {
        let bounds = super::slice::sequence_components(self, parts)?;
        let owned;
        let (target, replacement, observe_removed) = if let Some(source) = source
            && source != obj
            && matches!(self.heap.get(source), Value::List(_) | Value::Tuple(_))
        {
            let observe = self.heap.has_weak_values;
            let (source, Value::List(target)) = self.heap.source_and_target(source, obj).unwrap()
            else {
                unreachable!("list slice receiver")
            };
            let (Value::List(items) | Value::Tuple(items)) = source else {
                unreachable!("exact replacement sequence")
            };
            (target, Some(items.as_slice()), observe)
        } else {
            owned = source
                .map(|source| {
                    self.fast_sequence_values(source, "must assign iterable to extended slice")
                })
                .transpose()?;
            let observe = self.heap.has_weak_values;
            let Value::List(target) = self.heap.get_mut(obj) else {
                unreachable!("list slice receiver")
            };
            (target, owned.as_deref(), observe)
        };
        let [start, end, step] = super::slice::normalize(bounds, target.len() as i64);
        let positions = slice_positions(start, end, step);
        let count = slice_count(start, end, step);
        if step != 1
            && let Some(values) = replacement
            && values.len() != count
        {
            return Err(Error::new(
                "ValueError",
                format!(
                    "attempt to assign sequence of size {} to extended slice of size {count}",
                    values.len()
                ),
            ));
        }
        // The heap only observes displaced weak edges. Most lists can be changed
        // directly without allocating and traversing a second set of slots.
        let removed = if observe_removed {
            sliced_values(target, positions.clone(), count)?
        } else {
            Vec::new()
        };
        if step == 1 {
            let replacement = replacement.unwrap_or_default();
            target
                .try_reserve(replacement.len().saturating_sub(count))
                .map_err(slice_memory_error)?;
            let range = start as usize..end.max(start) as usize;
            if count == replacement.len() {
                target[range].copy_from_slice(replacement);
            } else {
                target.splice(range, replacement.iter().copied());
            }
        } else if let Some(replacement) = replacement {
            for (i, &value) in positions.zip(replacement) {
                target[i] = value;
            }
        } else {
            remove_slice_positions(target, start, end, step);
        }
        for item in removed {
            self.heap.weak_edge_removed(item);
        }
        Ok(())
    }

    fn set_slice(&mut self, obj: Id, parts: [Id; 3], source: Option<Id>) -> Result<()> {
        let mut bounds = super::slice::sequence_components(self, parts)?;
        let data = match source {
            Some(source) => {
                let data = builtins::bytes_from_iterable(self, source)?;
                // CPython retries the assignment with a converted or aliased bytearray.
                if source == obj || !matches!(self.heap.get(source), Value::ByteArray(_)) {
                    bounds = super::slice::sequence_components(self, parts)?;
                }
                data
            }
            None => Vec::new(),
        };
        let len = self.bytes_like(obj)?.len() as i64;
        let [start, end, step] = super::slice::normalize(bounds, len);
        if (step == 1 && data.len() != (end - start).max(0) as usize)
            || (step != 1 && data.is_empty() && slice_positions(start, end, step).next().is_some())
        {
            self.check_bytearray_resize(obj)?;
        }
        let Value::ByteArray(target) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        if step == 1 {
            let range = start as usize..end.max(start) as usize;
            target
                .try_reserve(data.len().saturating_sub(range.len()))
                .map_err(|_| Error::new("MemoryError", "bytearray is too large"))?;
            target.splice(range, data);
        } else if data.is_empty() {
            // Assigning an empty buffer deletes even the non-contiguous positions.
            remove_slice_positions(target, start, end, step);
        } else {
            let positions = slice_positions(start, end, step);
            let count = positions.clone().count();
            if count != data.len() {
                return Err(Error::new(
                    "ValueError",
                    format!(
                        "attempt to assign bytes of size {} to extended slice of size {count}",
                        data.len()
                    ),
                ));
            }
            for (i, byte) in positions.zip(data) {
                target[i] = byte;
            }
        }
        Ok(())
    }

    /// Locate builtin sequence storage without reading its elements before the multiplier runs.
    fn repeat_storage(&self, object: Id) -> Option<Id> {
        let storage = self.scalar_value(object);
        matches!(
            self.heap.get(storage),
            Value::List(_)
                | Value::Deque(_)
                | Value::Tuple(_)
                | Value::Str(_)
                | Value::SurrogateStr(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
        )
        .then_some(storage)
    }

    pub(super) fn sequence_multiplier(
        &mut self,
        multiplier: Id,
        wrapper: bool,
    ) -> Result<(crate::Int, isize)> {
        let count = if wrapper {
            self.index_integer(multiplier)?
        } else {
            self.index_value(multiplier)?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "can't multiply sequence by non-int of type '{}'",
                        self.type_name(multiplier)
                    ),
                )
            })?
        };
        let n = count
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    format!(
                        "cannot fit '{}' into an index-sized integer",
                        self.type_name(multiplier)
                    ),
                )
            })?;
        Ok((count, n))
    }

    fn overrides_repeat(&self, owner: Id, name: &str) -> bool {
        self.slot(owner, name).is_some_and(|slot| {
            !matches!(self.heap.get(slot), Value::Native(symbol)
                if symbol.strip_prefix("collections.deque.") == Some(name))
        })
    }

    /// Repeat the current contents after converting the multiplier through its index slot.
    fn repeat_sequence(
        &mut self,
        owner: Id,
        storage: Id,
        multiplier: Id,
        mode: RepeatMode,
    ) -> Result<Id> {
        let (count, n) = self.sequence_multiplier(multiplier, mode == RepeatMode::Wrapper)?;
        Ok(match self.heap.get(storage) {
            Value::Deque(_) => return deque::repeat(self, owner, n, mode == RepeatMode::InPlace),
            Value::List(items) if mode == RepeatMode::InPlace => {
                if n <= 0 {
                    builtins::clear_container(self, storage);
                } else if n != 1 && !items.is_empty() {
                    let repeated = repeat_values(items, n)?;
                    let Value::List(items) = self.heap.get_mut(storage) else {
                        unreachable!()
                    };
                    *items = repeated;
                }
                owner
            }
            Value::List(items) => {
                let repeated = repeat_values(items, n)?;
                self.alloc(Value::List(repeated))
            }
            Value::Tuple(items) if items.is_empty() || (n == 1 && owner == storage) => storage,
            Value::Tuple(items) => {
                let repeated = repeat_values(items, n)?;
                self.alloc(Value::Tuple(repeated))
            }
            Value::Bytes(data) => match crate::objects::repeat_bytes(data, &count)? {
                Some(repeated) => self.alloc(Value::Bytes(repeated)),
                None => storage,
            },
            Value::ByteArray(data) if mode == RepeatMode::InPlace => {
                if n != 1 && !data.is_empty() {
                    let size = repeated_len(data.len(), n)?;
                    self.check_bytearray_resize(storage)?;
                    let Value::ByteArray(data) = self.heap.get_mut(storage) else {
                        unreachable!()
                    };
                    if size == 0 {
                        data.clear();
                    } else {
                        data.try_reserve_exact(size - data.len())
                            .map_err(render_allocation)?;
                        while data.len() < size {
                            data.extend_from_within(..data.len().min(size - data.len()));
                        }
                    }
                }
                owner
            }
            Value::ByteArray(data) => {
                let repeated = repeat_values(data, n)?;
                self.alloc(Value::ByteArray(repeated))
            }
            Value::Str(_) | Value::SurrogateStr(_) if n == 1 => storage,
            Value::SurrogateStr(points) => self.text_codepoints(repeat_values(points, n)?),
            Value::Str(text) => self.text(crate::objects::repeat_str(text, &count)?.into_string()),
            _ => unreachable!("sequence repetition payload"),
        })
    }

    /// Apply a native binary operator to two set payloads without invoking subclass overrides.
    pub(super) fn set_binary(
        &mut self,
        op: &str,
        left: Id,
        right: Id,
        inplace: bool,
    ) -> Result<Id> {
        if op == "^" {
            return self.set_xor(left, right, inplace);
        }
        // Both union and self-intersection start from the left set's native table.
        if op != "|" && !(op == "&" && left == right) {
            return self.set_algebra(op, left, right, inplace);
        }
        let Value::Set { entries, frozen } = self.heap.get(left) else {
            unreachable!()
        };
        let result = if inplace && !frozen {
            left
        } else {
            self.alloc(Value::Set {
                entries: entries.try_clone()?,
                frozen: *frozen,
            })
        };
        if left != right {
            builtins::update_set(self, result, right)?;
        }
        Ok(result)
    }

    /// Binary XOR starts with the right table and toggles left keys, so its right
    /// residents get the first comparison. In-place XOR toggles the left table.
    pub(super) fn set_xor(&mut self, left: Id, right: Id, inplace: bool) -> Result<Id> {
        let frozen = matches!(self.heap.get(left), Value::Set { frozen: true, .. });
        let (result, source) = if inplace && !frozen {
            if left == right {
                self.set_difference_update(left, right)?;
                return Ok(left);
            }
            (left, right)
        } else {
            let result = self.alloc(Value::Set {
                entries: Default::default(),
                frozen,
            });
            builtins::update_set(self, result, right)?;
            (result, left)
        };
        self.visit_set_operand(source, true, |ctx, key, hash| {
            let roots = ctx.heap.temporary_mark();
            ctx.heap.keep(key);
            let toggle = ctx
                .find_dictionary_key(result, key, hash)
                .and_then(|(hash, found)| {
                    if let Some(index) = found {
                        let Value::Set { entries, .. } = ctx.heap.get_mut(result) else {
                            unreachable!()
                        };
                        let (resident, _) = entries.remove(index)?;
                        let mark = ctx.heap.temporary_mark();
                        ctx.heap.keep(resident);
                        ctx.heap.weak_edge_removed(resident);
                        ctx.discard_truth(mark);
                    } else {
                        // Insertion probes again: a collision callback may change its answer.
                        ctx.set_dictionary_key(result, key, NONE, Some(hash), false)?;
                    }
                    Ok(())
                });
            // Comparison can clear the source. Release our key before advancing
            // the live cursor, and preserve any active exception during finalization.
            ctx.heap.release_temporaries_since(roots);
            if let Err(error) = &toggle {
                for id in error.roots() {
                    ctx.heap.keep(id);
                }
            }
            let mark = ctx.heap.temporary_mark();
            ctx.heap.keep(key);
            ctx.discard_truth(mark);
            toggle
        })?;
        Ok(result)
    }

    /// Visit stored set keys, and optionally exact dictionary keys, with their cached hashes.
    fn visit_set_operand(
        &mut self,
        source: Id,
        use_dict_hashes: bool,
        mut visit: impl FnMut(&mut Self, Id, Option<i64>) -> Result<()>,
    ) -> Result<()> {
        let iterator = if matches!(self.heap.get(source), Value::Set { .. })
            || (use_dict_hashes && matches!(self.heap.get(source), Value::Dict(_)))
        {
            None
        } else {
            let iterator = self.iterator(source)?;
            self.heap.keep(iterator);
            Some(iterator)
        };
        let mark = self.heap.temporary_mark();
        let mut limit = self.heap.streaming_collection_limit();
        let mut position = 0;
        loop {
            let (key, hash) = if let Some(iterator) = iterator {
                self.collect_streaming_garbage(&mut limit);
                let Some(key) = self.next_item(iterator)? else {
                    break;
                };
                (key, None)
            } else if let Some((at, key, _, hash)) = self.dictionary_pair(source, position) {
                position = at + 1;
                (key, hash)
            } else {
                break;
            };
            visit(self, key, hash)?;
            self.heap.release_temporaries_since(mark);
        }
        Ok(())
    }

    /// Subtract each source member in turn, preserving prior removals if iteration raises.
    pub(super) fn set_difference_update(&mut self, target: Id, source: Id) -> Result<()> {
        let storage = self.scalar_value(source);
        let source = if let Value::Set { entries, .. } = self.heap.get(storage) {
            if storage == target {
                let Value::Set { entries, .. } = self.heap.get_mut(target) else {
                    unreachable!()
                };
                for (key, _) in entries.clear() {
                    self.heap.weak_edge_removed(key);
                }
                return Ok(());
            }
            if entries.len() >> 3 > self.stored_entries(target).len() {
                self.set_algebra("&", target, storage, false)?
            } else {
                storage
            }
        } else {
            source
        };
        self.visit_set_operand(source, false, |ctx, key, hash| {
            ctx.discard_difference_key(target, key, hash)
        })
    }

    fn discard_difference_key(&mut self, target: Id, key: Id, hash: Option<i64>) -> Result<()> {
        if let Some(index) = self.find_dictionary_key(target, key, hash)?.1 {
            let Value::Set { entries, .. } = self.heap.get_mut(target) else {
                unreachable!()
            };
            let (removed, _) = entries.remove(index)?;
            self.heap.weak_edge_removed(removed);
        }
        Ok(())
    }

    fn filter_difference(&mut self, result: Id, left: Id, right: Id) -> Result<()> {
        self.visit_set_operand(left, false, |ctx, key, hash| {
            ctx.heap.keep(key);
            if ctx.find_dictionary_key(right, key, hash)?.1.is_none() {
                ctx.set_dictionary_key(result, key, NONE, hash, false)?;
            }
            Ok(())
        })
    }

    /// Return a fresh base set, preserving stored hashes and comparison direction
    /// for a first set or exact dictionary. Later operands are discarded in order.
    pub(super) fn set_difference(&mut self, receiver: Id, mut operands: &[Id]) -> Result<Id> {
        let frozen = matches!(self.heap.get(receiver), Value::Set { frozen: true, .. });
        let result = self.alloc(Value::Set {
            entries: Default::default(),
            frozen,
        });
        let indexed = operands.first().and_then(|&other| {
            let storage = self.scalar_value(other);
            if matches!(self.heap.get(storage), Value::Set { .. }) {
                Some(storage)
            } else if matches!(self.heap.get(other), Value::Dict(_)) {
                Some(other)
            } else {
                None
            }
        });
        if let Some(right) = indexed
            && self.stored_entries(receiver).len() >> 2 <= self.stored_entries(right).len()
        {
            self.filter_difference(result, receiver, right)?;
            operands = &operands[1..];
        } else {
            builtins::update_set(self, result, receiver)?;
        }
        for &source in operands {
            if matches!(self.heap.get(self.scalar_value(source)), Value::Set { .. }) {
                self.set_difference_update(result, source)?;
            } else {
                self.subtract_iterable(result, source)?;
            }
        }
        Ok(result)
    }

    /// Release temporary iterators before the next operand. A finalizer or a
    /// generator's finally clause may mutate that operand.
    fn subtract_iterable(&mut self, result: Id, source: Id) -> Result<()> {
        let roots = self.heap.temporary_mark();
        let iterator = self.iterator(source)?;
        self.heap.keep(iterator);
        let step = self.heap.temporary_mark();
        let mut failed_key = None;
        let scan: Result<()> = (|| {
            let mut limit = self.heap.streaming_collection_limit();
            loop {
                self.collect_streaming_garbage(&mut limit);
                let Some(key) = self.next_item(iterator)? else {
                    return Ok(());
                };
                self.heap.keep(key);
                failed_key = Some(key);
                self.discard_difference_key(result, key, None)?;
                failed_key = None;
                self.discard_truth(step);
            }
        })();
        self.heap.release_temporaries_since(roots);
        if let Err(error) = &scan {
            for id in error.roots() {
                self.heap.keep(id);
            }
        }
        let key_mark = self.heap.temporary_mark();
        if let Some(key) = failed_key {
            self.heap.keep(key);
        }
        let iterator_mark = self.heap.temporary_mark();
        self.heap.keep(iterator);
        self.discard_truth(iterator_mark);
        self.discard_truth(key_mark);
        scan
    }

    /// Compute set difference or intersection, including reflected dictionary-view operands.
    fn set_algebra(&mut self, op: &str, left: Id, right: Id, inplace: bool) -> Result<Id> {
        let sets = matches!(
            (self.heap.get(left), self.heap.get(right)),
            (Value::Set { .. }, Value::Set { .. })
        );
        let frozen = sets && matches!(self.heap.get(left), Value::Set { frozen: true, .. });
        let mutate = inplace && sets && !frozen;
        let result = if mutate && op == "-" {
            left
        } else {
            self.alloc(Value::Set {
                entries: Default::default(),
                frozen,
            })
        };
        if op == "-" {
            // For comparable sizes CPython probes the right set with each left key.
            // Copy-and-discard reverses the equality callback, and is only used
            // for a much larger left operand or for mutable difference updates.
            if sets
                && !mutate
                && self.stored_entries(left).len() >> 2 <= self.stored_entries(right).len()
            {
                self.filter_difference(result, left, right)?;
                return Ok(result);
            }
            if let Value::DictView {
                dict,
                kind: value::DictViewKind::Keys,
                ..
            } = *self.heap.get(left)
                && matches!(self.heap.get(dict), Value::Dict(_))
            {
                let mut position = 0;
                while let Some((at, key, _, hash)) = self.dictionary_pair(dict, position) {
                    self.set_dictionary_key(result, key, NONE, hash, false)?;
                    position = at + 1;
                }
            } else if result != left {
                builtins::update_set(self, result, left)?;
            }
            self.set_difference_update(result, right)?;
            return Ok(result);
        }
        let (container, source) = if sets {
            if self.stored_entries(left).len() < self.stored_entries(right).len() {
                (right, left)
            } else {
                (left, right)
            }
        } else {
            let view_len = |id| {
                self.set_view(id)
                    .map(|(dict, _)| self.stored_entries(dict).len())
            };
            let (view, size, other) = if let Some(size) = view_len(left) {
                (left, size, right)
            } else {
                (
                    right,
                    view_len(right).expect("set-like dictionary view"),
                    left,
                )
            };
            let scan_view = match self.heap.get(other) {
                Value::Set {
                    entries,
                    frozen: false,
                } => size <= entries.len(),
                _ => view_len(other).is_some_and(|n| size < n),
            };
            if scan_view {
                (other, view)
            } else {
                (view, other)
            }
        };
        self.visit_set_operand(source, false, |ctx, key, hash| {
            let present = if let Some(view) = ctx.set_view(container) {
                ctx.view_member(view, key, hash)?
            } else {
                ctx.find_dictionary_key(container, key, hash)?.1.is_some()
            };
            if present {
                ctx.set_dictionary_key(result, key, NONE, hash, false)?;
            }
            Ok(())
        })?;
        if mutate {
            self.replace_set_entries(left, result);
            return Ok(left);
        }
        Ok(result)
    }

    /// Install the completed intersection before releasing any of the receiver's old keys.
    pub(super) fn replace_set_entries(&mut self, receiver: Id, result: Id) {
        let Value::Set { entries, .. } = self.heap.get_mut(result) else {
            unreachable!()
        };
        let selected = std::mem::take(entries);
        let Value::Set { entries, .. } = self.heap.get_mut(receiver) else {
            unreachable!()
        };
        for (key, _) in std::mem::replace(entries, selected) {
            self.heap.weak_edge_removed(key);
        }
    }

    /// Identify the dictionary views with set-like membership and comparison slots.
    fn set_view(&self, id: Id) -> Option<(Id, DictViewKind)> {
        match *self.heap.get(id) {
            Value::DictView {
                dict,
                kind: kind @ (DictViewKind::Keys | DictViewKind::Items),
                ..
            } => Some((
                self.dict_storage(dict).expect("dictionary view backing"),
                kind,
            )),
            _ => None,
        }
    }

    /// A set's native slots precede Python mixins appearing later in the MRO.
    fn overrides_set_slot(&self, id: Id, name: &str) -> bool {
        self.heap.get(id).instance_class().and_then(|class| {
            self.find_in_mro(class, |base| match self.heap.get(base) {
                Value::Native(kind) if matches!(kind.as_str(), "set" | "frozenset") => Some(false),
                _ => self.own_class_attr(base, name).map(|_| true),
            })
        }) == Some(true)
    }

    fn view_member(
        &mut self,
        (dict, kind): (Id, DictViewKind),
        item: Id,
        hash: Option<i64>,
    ) -> Result<bool> {
        if matches!(kind, DictViewKind::Keys) {
            return Ok(self.find_dictionary_key(dict, item, hash)?.1.is_some());
        }
        let Some(parts) = self.tuple_items(item)? else {
            return Ok(false);
        };
        let &[key, expected] = parts.as_slice() else {
            return Ok(false);
        };
        let Some(index) = self.find_dictionary_key(dict, key, None)?.1 else {
            return Ok(false);
        };
        let actual = self.stored_entries(dict)[index].1;
        self.heap.keep(actual);
        self.checked_equal(actual, expected)
    }

    /// Probe a view from a live native set cursor. Hashing an element can resize the iterated set.
    fn set_contained_in_view(&mut self, set: Id, view: (Id, DictViewKind)) -> Result<bool> {
        let size = self.stored_entries(set).len();
        let step = self.heap.temporary_mark();
        let mut position = 0;
        loop {
            if self.stored_entries(set).len() != size {
                return Err(Error::new(
                    "RuntimeError",
                    "Set changed size during iteration",
                ));
            }
            let Some((at, item, _, _)) = self.dictionary_pair(set, position) else {
                return Ok(true);
            };
            self.heap.keep(item);
            let found = self.view_member(view, item, None)?;
            self.discard_truth(step);
            if !found {
                return Ok(false);
            }
            position = at + 1;
        }
    }

    /// Compare a key or item view using the subset operand's iterator and the superset's membership.
    fn compare_view(&mut self, slot: &str, view: Id, other: Id) -> Result<bool> {
        let roots = self.heap.temporary_mark();
        // A comparison callback can remove either view from its enclosing container.
        self.heap.keep(view);
        self.heap.keep(other);
        let result: Result<bool> = (|| {
            let size = self.stored_entries(self.set_view(view).unwrap().0).len();
            let target = self.set_view(other);
            let length_roots = self.heap.temporary_mark();
            let other_size = if let Some((dict, _)) = target {
                self.stored_entries(dict).len()
            } else if self.overrides_set_slot(other, "__len__") {
                self.instance_len(other)?.expect("set subclass length slot")
            } else {
                self.stored_entries(self.scalar_value(other)).len()
            };
            self.discard_truth(length_roots);
            let (subset, superset) = match slot {
                "__eq__" | "__ne__" if size == other_size => (view, other),
                "__lt__" if size < other_size => (view, other),
                "__le__" if size <= other_size => (view, other),
                "__gt__" if size > other_size => (other, view),
                "__ge__" if size >= other_size => (other, view),
                _ => return Ok(slot == "__ne__"),
            };
            let target = self.set_view(superset);
            let storage = self.scalar_value(subset);
            if let Some(target) = target
                && matches!(self.heap.get(storage), Value::Set { .. })
                && !self.overrides_set_slot(subset, "__iter__")
            {
                return self.set_contained_in_view(storage, target);
            }
            let it = self.iterator(subset)?;
            self.heap.keep(it);
            let step = self.heap.temporary_mark();
            while let Some(item) = self.next_item(it)? {
                self.heap.keep(item);
                let found = if let Some(target) = target {
                    self.view_member(target, item, None)?
                } else if self.overrides_set_slot(superset, "__contains__") {
                    self.contains(item, superset)?
                } else {
                    builtins::set_member(self, superset, item, true)?
                };
                self.discard_truth(step);
                if !found {
                    return Ok(slot == "__ne__");
                }
            }
            Ok(slot != "__ne__")
        })();
        let collect = self.heap.discard_truth_operands(roots, self.modules);
        if let Err(error) = &result {
            for id in error.roots() {
                self.heap.keep(id);
            }
        }
        if collect {
            self.collect_unreachable();
        }
        result
    }

    /// Return the original storage of an exact list or a list subclass.
    pub(super) fn list_storage(&self, obj: Id) -> Option<Id> {
        let storage = self.scalar_value(obj);
        matches!(self.heap.get(storage), Value::List(_)).then_some(storage)
    }

    /// A Python override replaces the inherited sequence-concatenation slot.
    fn overrides_add(&self, object: Id, builtin_add: &str) -> bool {
        self.slot(object, "__add__")
            .is_some_and(|slot| self.builtins.get(builtin_add) != Some(&slot))
    }

    /// Concatenate a string receiver directly, with no reflected operation on the other operand.
    pub(super) fn concat_str(&mut self, left: Id, right: Id) -> Result<Id> {
        if self.string_storage(right).is_none() {
            let name = self.type_name(right);
            let name = &name[..name.floor_char_boundary(200)];
            return Err(Error::new(
                "TypeError",
                format!("can only concatenate str (not \"{name}\") to str"),
            ));
        }
        match (self.heap.get(left), self.heap.get(right)) {
            (Value::Str(s), Value::Str(_) | Value::SurrogateStr(_)) if s.is_empty() => Ok(right),
            (Value::Str(_) | Value::SurrogateStr(_), Value::Str(s)) if s.is_empty() => Ok(left),
            _ => self.concat_strings(left, right),
        }
    }

    /// Concatenate the two reference arrays without invoking either operand's iterator.
    pub(super) fn concat_list(&mut self, storage: Id, other: Id) -> Result<Id> {
        let Value::List(left) = self.heap.get(storage) else {
            unreachable!("list payload")
        };
        let Value::List(right) = self.heap.get(self.scalar_value(other)) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "can only concatenate list (not \"{}\") to list",
                    self.type_name(other)
                ),
            ));
        };
        let items = concat_ids(left, right)?;
        Ok(self.alloc(Value::List(items)))
    }

    /// Concatenate tuple storage without consulting a subclass's iterator or item hooks.
    fn concat_tuple(&mut self, a: Id, b: Id) -> Result<Id> {
        let items = |value| match self.tuple_backing(value) {
            Value::Tuple(items) | Value::NamedTuple { values: items, .. } => {
                Ok(Some(Cow::Borrowed(items.as_slice())))
            }
            _ => self.tuple_items(value).map(|items| items.map(Cow::Owned)),
        };
        let Some(right) = items(b)? else {
            let name = self.type_name(b);
            let name = &name[..name.floor_char_boundary(200)];
            return Err(Error::new(
                "TypeError",
                format!("can only concatenate tuple (not \"{name}\") to tuple"),
            ));
        };
        let left = items(a)?.expect("tuple receiver");
        if left.is_empty() && matches!(self.heap.get(b), Value::Tuple(_)) {
            return Ok(b);
        }
        if right.is_empty() && matches!(self.heap.get(a), Value::Tuple(_)) {
            return Ok(a);
        }
        let values = concat_ids(&left, &right)?;
        Ok(self.alloc(Value::Tuple(values)))
    }

    /// Join the boundary strings and preserve the existing interpolation objects.
    fn concat_template(&mut self, left: Id, right: Id) -> Result<Id> {
        let Value::Template {
            strings: left_strings,
            interpolations: left_interpolations,
        } = *self.heap.get(left)
        else {
            unreachable!("template addition")
        };
        let Value::Template {
            strings: right_strings,
            interpolations: right_interpolations,
        } = *self.heap.get(right)
        else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "can only concatenate string.templatelib.Template (not \"{}\") to string.templatelib.Template",
                    self.type_name(right)
                ),
            ));
        };
        let (Value::Tuple(left), Value::Tuple(right)) =
            (self.heap.get(left_strings), self.heap.get(right_strings))
        else {
            unreachable!("template string tuples")
        };
        let boundary = left.len() - 1;
        let mut strings = concat_ids(left, &right[1..])?;
        let (last, first) = (strings[boundary], right[0]);
        strings[boundary] = self.concat_template_strings(last, first)?;
        let interpolations = self.binary("+", left_interpolations, right_interpolations)?;
        let strings = self.alloc(Value::Tuple(strings));
        Ok(self.alloc(Value::Template {
            strings,
            interpolations,
        }))
    }

    pub(super) fn binary(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        self.binary_operation(op, a, b, false)
    }

    /// Keep upgraded operands alive until the forwarded operator finishes calling Python.
    fn with_proxy_operands(
        &mut self,
        left: Id,
        right: Id,
        apply: impl FnOnce(&mut Self) -> Result<Id>,
    ) -> Result<Id> {
        self.heap.reserve_temporaries(2)?;
        let mark = self.heap.temporary_mark();
        self.heap.keep(left);
        self.heap.keep(right);
        let result = apply(self);
        self.heap.release_temporaries_since(mark);
        if let Ok(value) = result {
            self.heap.keep(value);
        }
        self.heap.weak_edge_removed(left);
        self.heap.weak_edge_removed(right);
        result
    }

    /// Dispatch one mapping proxy slot at a time. Left operands inspect the original
    /// right operand before a reflected proxy slot unwraps it.
    fn mapping_proxy_union(&mut self, mut left: Id, mut right: Id) -> Result<Id> {
        loop {
            if let Value::MappingProxy(mapping) = *self.heap.get(left) {
                left = mapping;
                if let Value::MappingProxy(mapping) = *self.heap.get(right) {
                    right = mapping;
                }
                continue;
            }
            if matches!(self.heap.get(left), Value::Weak(wr) if wr.kind != weakref::Kind::Reference)
            {
                return self.binary("|", left, right);
            }
            let next_right = match self.heap.get(right) {
                Value::MappingProxy(mapping) => Some(*mapping),
                Value::Weak(wr) if wr.kind != weakref::Kind::Reference => None,
                _ => return self.binary("|", left, right),
            };
            match self.heap.get(left) {
                Value::Union(_) | Value::TypeVar(_) => {
                    return super::union::make(self, &[left, right]);
                }
                Value::GenericAlias(alias)
                    if alias.kind.is_typing_alias()
                        || matches!(alias.kind, super::typing::AliasKind::Literal(_)) =>
                {
                    return super::union::make(self, &[left, right]);
                }
                Value::DictView {
                    kind: value::DictViewKind::Keys | value::DictViewKind::Items,
                    ..
                } => {
                    return self.dict_view_union(left, right);
                }
                _ => {}
            }
            if let Some(result) = self.operator_slot(left, "__or__", right)? {
                return Ok(result);
            }
            // Upgrade weak references only when their reflected slot actually wins.
            right = match next_right {
                Some(mapping) => mapping,
                None => {
                    let target = self.weak_proxy_target(right)?;
                    self.heap.keep(target);
                    target
                }
            };
        }
    }

    /// Dictionary keys and items accept any iterable on either side of a binary union.
    fn dict_view_union(&mut self, left: Id, right: Id) -> Result<Id> {
        let result = self.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        builtins::update_set(self, result, left)?;
        builtins::update_set(self, result, right)?;
        Ok(result)
    }

    /// Acquire contiguous native bytes using the sequence concatenation error convention.
    fn concat_buffer(&self, left: Id, right: Id) -> Result<&[u8]> {
        self.check_buffer_exporter(right)
            .and_then(|()| self.native_buffer_bytes(right))
            .map_err(|_| {
                let name = |id| {
                    let name = self.type_name(id);
                    &name[..name.floor_char_boundary(100)]
                };
                Error::new(
                    "TypeError",
                    format!("can't concat {} to {}", name(right), name(left)),
                )
            })
    }

    fn unsupported_binary(&self, op: &str, a: Id, b: Id, inplace: bool) -> Error {
        Error::new(
            "TypeError",
            format!(
                "unsupported operand type(s) for {op}{}: '{}' and '{}'",
                if inplace { "=" } else { "" },
                self.type_name(a),
                self.type_name(b)
            ),
        )
    }

    fn binary_operation(&mut self, op: &str, a: Id, b: Id, inplace: bool) -> Result<Id> {
        match op {
            "is" => return Ok(builtins::boolean(a == b)),
            "is not" => return Ok(builtins::boolean(a != b)),
            _ => {}
        }
        if let (Value::Int(left), Value::Int(right)) = (self.heap.get(a), self.heap.get(b)) {
            if matches!(op, "==" | "!=") {
                return Ok(builtins::boolean(left.cmp(right).is_eq() == (op == "==")));
            }
            if let Some(code) = integer_binary_code(op) {
                let result = crate::integer_binary(left, right, code)?;
                return Ok(self.alloc(Value::Int(result)));
            }
        }
        if matches!(a, FALSE | TRUE) && matches!(b, FALSE | TRUE) {
            match op {
                "&" => return Ok(builtins::boolean(a == TRUE && b == TRUE)),
                "|" => return Ok(builtins::boolean(a == TRUE || b == TRUE)),
                "^" => return Ok(builtins::boolean(a != b)),
                _ => {}
            }
        }
        if !inplace && op == "|" {
            let weak_proxy = |id| matches!(self.heap.get(id), Value::Weak(wr) if wr.kind != weakref::Kind::Reference);
            if matches!(self.heap.get(a), Value::MappingProxy(_))
                || (matches!(self.heap.get(b), Value::MappingProxy(_)) && !weak_proxy(a))
                || (weak_proxy(b) && self.dict_storage(a).is_some())
            {
                return self.with_proxy_operands(a, b, |ctx| ctx.mapping_proxy_union(a, b));
            }
        }
        if self.heap.has_weak_values {
            let left = self.weak_proxy_target(a)?;
            let right = self.weak_proxy_target(b)?;
            if left != a || right != b {
                return self.with_proxy_operands(left, right, |ctx| {
                    ctx.binary_operation(op, left, right, inplace)
                });
            }
        }
        if op == "|"
            && matches!(
                self.heap.get(a),
                Value::Union(_) | Value::TypeVar(_) | Value::ParamSpec(_)
            )
        {
            return super::union::make(self, &[a, b]);
        }
        if let Some(result) = self.dispatch_binary_slots(op, a, b, inplace)? {
            return Ok(result);
        }
        if inplace
            && op == "+"
            && let storage = self.scalar_value(a)
            && matches!(self.heap.get(storage), Value::ByteArray(_))
        {
            let additional = self.concat_buffer(a, b)?.len();
            if additional == 0 {
                return Ok(a);
            }
            // Reading the right-hand buffer exports it during the resize.
            if storage == self.scalar_value(b) {
                return Err(Self::exported_bytearray_error());
            }
            self.check_bytearray_resize(storage)?;
            let Value::ByteArray(left) = self.heap.get_mut(storage) else {
                unreachable!()
            };
            left.try_reserve(additional)
                .map_err(|_| Error::new("MemoryError", "bytes allocation failed"))?;
            // The resize check ruled out every view of the destination. Keep its
            // allocation while borrowing the source buffer from the same heap.
            let mut bytes = std::mem::take(left);
            let appended = self
                .native_buffer_bytes(b)
                .map(|right| bytes.extend_from_slice(right));
            let Value::ByteArray(left) = self.heap.get_mut(storage) else {
                unreachable!()
            };
            *left = bytes;
            appended?;
            return Ok(a);
        }
        if inplace && op == "+" && matches!(self.heap.get(a), Value::Deque(_)) {
            deque::extend(self, a, b, false)?;
            return Ok(a);
        }
        if op == "|"
            && let Some(result) = super::union::binary(self, a, b)
        {
            return result;
        }
        if let (Value::Slice(left), Value::Slice(right)) = (self.heap.get(a), self.heap.get(b))
            && matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
        {
            return super::slice::compare(self, op, *left, *right);
        }
        let truth = |v| if v { TRUE } else { FALSE };
        match op {
            "|" | "&" | "-"
                if [a, b].into_iter().any(|id| {
                    matches!(
                        self.heap.get(id),
                        Value::DictView {
                            kind: value::DictViewKind::Keys | value::DictViewKind::Items,
                            ..
                        }
                    )
                }) =>
            {
                if op != "|" {
                    return self.set_algebra(op, a, b, false);
                }
                return self.dict_view_union(a, b);
            }
            "|" | "&" | "-" | "^"
                if [a, b].into_iter().any(|value| {
                    let storage = if op == "^" {
                        self.scalar_value(value)
                    } else {
                        value
                    };
                    matches!(self.heap.get(storage), Value::Set { .. })
                        || (op == "|" && self.dict_storage(value).is_some())
                }) =>
            {
                if matches!(
                    (self.heap.get(a), self.heap.get(b)),
                    (Value::Set { .. }, Value::Set { .. })
                ) {
                    return self.set_binary(op, a, b, inplace);
                }
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for {op}{}: '{}' and '{}'",
                        if inplace { "=" } else { "" },
                        self.type_name(a),
                        self.type_name(b)
                    ),
                ));
            }
            "%" if let Value::Str(text) = self.heap.get(a) => {
                return builtins::percent(self, &text.clone(), b);
            }
            "%" if let Value::Bytes(bytes) = self.heap.get(a) => {
                return builtins::bytes_percent(self, &bytes.clone(), b);
            }
            "==" | "!=" => {
                if a == b
                    && let Value::GenericAlias(alias) = self.heap.get(a)
                    && matches!(alias.kind, super::typing::AliasKind::Literal(_))
                {
                    let args = alias.args;
                    let left = super::typing::literal_members(self, args)?;
                    let right = super::typing::literal_members(self, args)?;
                    return Ok(truth(self.dictionary_equal(left, right)? == (op == "==")));
                }
                let equal = self.checked_equal_impl(a, b, false)?
                    && !matches!(self.heap.get(a), Value::Float(n) if n.is_nan())
                    && !matches!(self.heap.get(a), Value::Complex(z) if z.real.is_nan() || z.imag.is_nan());
                return Ok(truth(equal == (op == "==")));
            }
            "in" => return Ok(truth(self.contains(a, b)?)),
            "not in" => return Ok(truth(!self.contains(a, b)?)),
            "<" | "<=" | ">" | ">=" => return self.order(op, a, b),
            "+" if matches!(self.heap.get(a), Value::Template { .. }) => {
                return self.concat_template(a, b);
            }
            "+" if self.string_storage(a).is_some() => {
                if self.overrides_add(a, "str.__add__") {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "unsupported operand type(s) for +{}: '{}' and '{}'",
                            if inplace { "=" } else { "" },
                            self.type_name(a),
                            self.type_name(b)
                        ),
                    ));
                }
                return self.concat_str(a, b);
            }
            "+" if self.builtin_slot_accepts(a, "tuple") => {
                if self.overrides_add(a, "tuple.__add__") {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "unsupported operand type(s) for +{}: '{}' and '{}'",
                            if inplace { "=" } else { "" },
                            self.type_name(a),
                            self.type_name(b)
                        ),
                    ));
                }
                return self.concat_tuple(a, b);
            }
            "+" | "-"
                if matches!(self.heap.get(a), Value::Complex(_))
                    || matches!(self.heap.get(b), Value::Complex(_)) =>
            {
                return self.complex_sum(op == "+", a, b);
            }
            "+" if matches!(self.heap.get(self.scalar_value(a)), Value::Deque(_)) => {
                if self.overrides_add(a, "collections.deque.__add__") {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "unsupported operand type(s) for +{}: '{}' and '{}'",
                            if inplace { "=" } else { "" },
                            self.type_name(a),
                            self.type_name(b)
                        ),
                    ));
                }
                return deque::concat(self, a, b);
            }
            "+" if let Some(storage) = self.list_storage(a) => {
                if inplace && storage == a {
                    builtins::call(self, "list.extend", &[storage, b], &[])?;
                    return Ok(a);
                }
                if self.overrides_add(a, "list.__add__") {
                    // A Python __add__ disables the native sequence-concatenation fallback.
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "unsupported operand type(s) for +{}: '{}' and '{}'",
                            if inplace { "=" } else { "" },
                            self.type_name(a),
                            self.type_name(b)
                        ),
                    ));
                }
                return self.concat_list(storage, b);
            }
            "+" => match (self.heap.get(a), self.heap.get(b)) {
                (Value::Bytes(x), _) => {
                    let y = self.concat_buffer(a, b)?;
                    if y.is_empty() {
                        return Ok(a);
                    }
                    if x.is_empty() && matches!(self.heap.get(b), Value::Bytes(_)) {
                        return Ok(b);
                    }
                    let bytes = crate::objects::concat_bytes(x, y)?;
                    return Ok(self.alloc(Value::Bytes(bytes)));
                }
                (Value::ByteArray(x), Value::Bytes(y) | Value::ByteArray(y)) => {
                    let bytes = crate::objects::concat_bytes(x, y)?;
                    return Ok(self.alloc(Value::ByteArray(bytes)));
                }
                (Value::ByteArray(_), _) => {
                    return Err(Error::new(
                        "TypeError",
                        format!("can't concat {} to bytearray", self.type_name(b)),
                    ));
                }
                _ => {}
            },
            "*" => {
                let mut skipped_repeat = false;
                for (owner, multiplier, left) in [(a, b, true), (b, a, false)] {
                    let Some(storage) = self.repeat_storage(owner) else {
                        continue;
                    };
                    let mode = if left
                        && inplace
                        && matches!(
                            self.heap.get(storage),
                            Value::List(_) | Value::ByteArray(_) | Value::Deque(_)
                        )
                        && !self.overrides_repeat(owner, "__imul__")
                    {
                        RepeatMode::InPlace
                    } else if !self.overrides_repeat(owner, "__mul__")
                        && !self.overrides_repeat(owner, "__rmul__")
                    {
                        RepeatMode::Sequence
                    } else {
                        // A Python multiplication slot has already declined the operation.
                        skipped_repeat = true;
                        continue;
                    };
                    return self.repeat_sequence(owner, storage, multiplier, mode);
                }
                if skipped_repeat {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "unsupported operand type(s) for *{}: '{}' and '{}'",
                            if inplace { "=" } else { "" },
                            self.type_name(a),
                            self.type_name(b)
                        ),
                    ));
                }
            }
            _ => {}
        }
        if op == "@" {
            return Err(Error::new(
                "TypeError",
                format!(
                    "unsupported operand type(s) for @: '{}' and '{}'",
                    self.type_name(a),
                    self.type_name(b)
                ),
            ));
        }
        let (left, right) = (self.scalar_value(a), self.scalar_value(b));
        if matches!(op, "*" | "/" | "//" | "%")
            && (matches!(self.heap.get(left), Value::Complex(_))
                || matches!(self.heap.get(right), Value::Complex(_)))
        {
            return Err(self.unsupported_binary(op, a, b, inplace));
        }
        let float_operand = matches!(self.heap.get(left), Value::Float(_))
            || matches!(self.heap.get(right), Value::Float(_));
        // After a float subtype declines division, its underlying value cannot retry it.
        if float_operand
            && (op != "/" || (a == left && b == right))
            && let (Some(x), Some(y)) = (self.real_component(left)?, self.real_component(right)?)
        {
            let result = match op {
                "+" => Some(x + y),
                "-" => Some(x - y),
                "*" => Some(x * y),
                "**" => {
                    if y == 0.0 {
                        Some(1.0)
                    } else if x.is_nan() {
                        Some(x)
                    } else if y.is_nan() {
                        Some(if x == 1.0 { 1.0 } else { y })
                    } else if y.is_infinite() {
                        let magnitude = x.abs();
                        Some(if magnitude == 1.0 {
                            1.0
                        } else if (y > 0.0) == (magnitude > 1.0) {
                            f64::INFINITY
                        } else {
                            0.0
                        })
                    } else {
                        let odd = y.fract() == 0.0 && (y % 2.0).abs() == 1.0;
                        if x.is_infinite() {
                            Some(if y > 0.0 {
                                if odd { x } else { x.abs() }
                            } else if odd {
                                0.0_f64.copysign(x)
                            } else {
                                0.0
                            })
                        } else if x == 0.0 {
                            if y < 0.0 {
                                return Err(Error::new(
                                    "ZeroDivisionError",
                                    "zero to a negative power",
                                ));
                            }
                            Some(if odd { x } else { 0.0 })
                        } else if x < 0.0 && y.fract() != 0.0 {
                            let base = super::complex::Complex::from(x);
                            return Ok(self.alloc(Value::Complex(base.power(y.into())?)));
                        } else {
                            let magnitude = x.abs().powf(y);
                            if magnitude.is_infinite() {
                                return Err(Error::new(
                                    "OverflowError",
                                    "(34, 'Numerical result out of range')",
                                ));
                            }
                            Some(if x < 0.0 && odd {
                                -magnitude
                            } else {
                                magnitude
                            })
                        }
                    }
                }
                "/" | "//" | "%" => {
                    if y == 0.0 {
                        return Err(Error::new("ZeroDivisionError", "division by zero"));
                    }
                    let operation = match op {
                        "/" => 0,
                        "//" => 1,
                        _ => 2,
                    };
                    Some(crate::floats::imre_float_divide(x, y, operation))
                }
                _ => None,
            };
            if let Some(result) = result {
                return Ok(self.alloc(Value::Float(result)));
            }
        }
        if op == "%"
            && [left, right]
                .into_iter()
                .any(|id| self.calendar_slot_class(id, false).is_some())
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "unsupported operand type(s) for %{}: '{}' and '{}'",
                    if inplace { "=" } else { "" },
                    stdlib::datetime::type_name(self, a),
                    stdlib::datetime::type_name(self, b)
                ),
            ));
        }
        if matches!(op, "+" | "**" | "/")
            && [left, right]
                .into_iter()
                .any(|id| !matches!(self.heap.get(id), Value::Int(_) | Value::Bool(_)))
        {
            let operator = match (op, inplace) {
                ("+", true) => "+=",
                ("+", false) => "+",
                ("/", true) => "/=",
                ("/", false) => "/",
                _ => "** or pow()",
            };
            let left = stdlib::diagnostic_type_name(self, a);
            let right = stdlib::diagnostic_type_name(self, b);
            return Err(Error::new(
                "TypeError",
                format!(
                    "unsupported operand type(s) for {operator}: '{}' and '{}'",
                    &left[..left.floor_char_boundary(100)],
                    &right[..right.floor_char_boundary(100)]
                ),
            ));
        }
        let (x, y) = (self.as_int(a)?, self.as_int(b)?);
        if op == "/" {
            return Ok(self.alloc(Value::Float(crate::floats::ratio(&x, &y)?)));
        }
        let result = if op == "**" {
            if y.cmp(&crate::Int::from(0)).is_lt() {
                let result = crate::integer_power::negative_power(&x, &y)?;
                return Ok(self.alloc(Value::Float(result)));
            }
            crate::integer_power::power(&x, &y)?
        } else {
            let code = integer_binary_code(op)
                .ok_or_else(|| Error::new("TypeError", format!("unsupported operator {op}")))?;
            crate::integer_binary(&x, &y, code)?
        };
        let class = if matches!(op, "&" | "|" | "^") {
            self.flag_class(a).or_else(|| self.flag_class(b))
        } else {
            None
        };
        self.flag_or_integer(class, result)
    }
}

/// Map an integer operator to the operation implemented by the integer runtime.
pub(super) fn integer_binary_code(op: &str) -> Option<u32> {
    Some(match op {
        "+" => 0,
        "-" => 1,
        "*" => 2,
        "//" => 3,
        "%" => 4,
        "&" => 5,
        "|" => 6,
        "^" => 7,
        "<<" => 8,
        ">>" => 9,
        _ => return None,
    })
}

// These operations implement both integer slots through integer_binary.
const INHERITED_INTEGER_OPS: [&str; 4] = ["//", "%", "<<", ">>"];

/// Match a numeric wrapper to its operation and operand direction.
pub(super) fn numeric_binary_slot(owner: &str, name: &str) -> Option<(&'static str, bool)> {
    let operations: &[&str] = match owner {
        "int" => &INHERITED_INTEGER_OPS,
        "complex" => &["*", "/", "**"],
        _ => return None,
    };
    operations.iter().find_map(|&op| {
        let (forward, reflected, _) = binary_slots(op)?;
        (name == forward || name == reflected).then_some((op, name == reflected))
    })
}

/// Return the comparison performed by a rich comparison slot.
pub(super) fn comparison_op(slot: &str) -> Option<&'static str> {
    Some(match slot {
        "__lt__" => "<",
        "__le__" => "<=",
        "__eq__" => "==",
        "__ne__" => "!=",
        "__gt__" => ">",
        "__ge__" => ">=",
        _ => return None,
    })
}

/// Special-method names for the two operands of a Python binary expression.
fn binary_slots(op: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match op {
        "+" => ("__add__", "__radd__", "__iadd__"),
        "-" => ("__sub__", "__rsub__", "__isub__"),
        "*" => ("__mul__", "__rmul__", "__imul__"),
        "@" => ("__matmul__", "__rmatmul__", "__imatmul__"),
        "/" => ("__truediv__", "__rtruediv__", "__itruediv__"),
        "//" => ("__floordiv__", "__rfloordiv__", "__ifloordiv__"),
        "%" => ("__mod__", "__rmod__", "__imod__"),
        "**" => ("__pow__", "__rpow__", "__ipow__"),
        "<<" => ("__lshift__", "__rlshift__", "__ilshift__"),
        ">>" => ("__rshift__", "__rrshift__", "__irshift__"),
        "&" => ("__and__", "__rand__", "__iand__"),
        "^" => ("__xor__", "__rxor__", "__ixor__"),
        "|" => ("__or__", "__ror__", "__ior__"),
        "<" => ("__lt__", "__gt__", ""),
        "<=" => ("__le__", "__ge__", ""),
        "==" => ("__eq__", "__eq__", ""),
        "!=" => ("__ne__", "__ne__", ""),
        ">=" => ("__ge__", "__le__", ""),
        ">" => ("__gt__", "__lt__", ""),
        _ => return None,
    })
}

impl Context {
    /// Exact native calendar operands participate in Python's reflected numeric slot ordering.
    fn calendar_slot_class(&self, value: Id, add_or_subtract: bool) -> Option<Id> {
        let Value::Calendar { value, .. } = self.heap.get(value) else {
            return None;
        };
        let class = match value.class() {
            "timedelta" => "datetime.timedelta",
            "date" if add_or_subtract => "datetime.date",
            "datetime" if add_or_subtract => "datetime.datetime",
            _ => return None,
        };
        self.builtins.get(class).copied()
    }

    /// An inherited object.__ne__ leaves ordinary comparison to the equality fallback.
    fn has_custom_ne(&self, value: Id) -> bool {
        self.slot(value, "__ne__")
            .is_some_and(|slot| Some(&slot) != self.builtins.get("object.__ne__"))
    }

    pub(super) fn operator_slot(&mut self, obj: Id, name: &str, other: Id) -> Result<Option<Id>> {
        // Rich comparison calls a native wrapper with an explicit receiver. In
        // particular, an alias on a foreign class raises the wrapper's call error.
        if comparison_op(name).is_some()
            && let Some(slot) = self.slot(obj, name)
            && let Value::Native(symbol) = self.heap.get(slot)
            && let Some(("int" | "float", method)) = symbol.split_once('.')
            && comparison_op(method).is_some()
        {
            let result = self.call(slot, vec![obj, other], vec![])?;
            return Ok((result != value::NOT_IMPLEMENTED).then_some(result));
        }
        if let Value::KeyWrapper { comparator, object } = *self.heap.get(obj) {
            return stdlib::functools::key_comparison(self, name, comparator, object, other);
        }
        // Defining only __radd__ also installs a forward numeric slot. In that case
        // the inherited tuple concatenation runs before an unrelated reflected slot.
        if name == "__add__"
            && self.builtin_slot_accepts(obj, "tuple")
            && !self.overrides_add(obj, "tuple.__add__")
        {
            return if self.slot(obj, "__radd__").is_some() {
                self.concat_tuple(obj, other).map(Some)
            } else {
                Ok(None)
            };
        }
        // Native deque repetition is a sequence fallback, after numeric hooks. A subtype
        // defining either multiply hook installs a numeric slot for both directions.
        if matches!(name, "__mul__" | "__rmul__" | "__imul__")
            && matches!(self.heap.get(self.scalar_value(obj)), Value::Deque(_))
            && !self.overrides_repeat(obj, name)
            && (name == "__imul__"
                || (!self.overrides_repeat(obj, "__mul__")
                    && !self.overrides_repeat(obj, "__rmul__")))
        {
            return Ok(None);
        }
        // Sequence concatenation runs after the right operand's reflected numeric slot.
        if name == "__add__" {
            let native = match self.heap.get(self.scalar_value(obj)) {
                Value::Str(_) | Value::SurrogateStr(_) => Some("str.__add__"),
                Value::List(_) => Some("list.__add__"),
                Value::Deque(_) => Some("collections.deque.__add__"),
                _ => None,
            };
            if native.is_some_and(|slot| !self.overrides_add(obj, slot))
                && self.slot(obj, "__radd__").is_none()
            {
                return Ok(None);
            }
        }
        let method = if matches!(
            (name, self.heap.get(obj)),
            (
                "__mul__" | "__rmul__" | "__truediv__" | "__rtruediv__" | "__pow__" | "__rpow__",
                Value::Complex(_)
            ) | ("__truediv__" | "__rtruediv__", Value::Float(_))
                | (
                    "__mod__" | "__rmod__",
                    Value::Str(_) | Value::SurrogateStr(_)
                )
                | ("__lt__" | "__le__" | "__gt__" | "__ge__", Value::Deque(_))
                | ("__xor__" | "__rxor__", Value::Set { .. })
                | (
                    "__or__" | "__ror__",
                    Value::Dict(_) | Value::DefaultDict { .. }
                )
        ) {
            Some(self.lookup_attr(obj, name)?)
        } else if name == "__ne__" && !self.has_custom_ne(obj) {
            None
        } else if matches!(
            name,
            "__add__"
                | "__radd__"
                | "__sub__"
                | "__rsub__"
                | "__mul__"
                | "__rmul__"
                | "__truediv__"
                | "__rtruediv__"
                | "__floordiv__"
                | "__rfloordiv__"
                | "__mod__"
                | "__rmod__"
                | "__divmod__"
                | "__rdivmod__"
        ) && self
            .calendar_slot_class(
                obj,
                matches!(name, "__add__" | "__radd__" | "__sub__" | "__rsub__"),
            )
            .is_some()
        {
            Some(self.lookup_attr(obj, name)?)
        } else {
            self.bound_slot(obj, name)?
        };
        let Some(method) = method else {
            // An inherited integer addition accepts any integer storage, including bool.
            if matches!(name, "__add__" | "__radd__") {
                let (left, right) = (self.scalar_value(obj), self.scalar_value(other));
                if [left, right]
                    .iter()
                    .all(|&id| matches!(self.heap.get(id), Value::Int(_) | Value::Bool(_)))
                {
                    let sum = self.as_int(left)?.add(&self.as_int(right)?);
                    return Ok(Some(self.heap.alloc(Value::Int(sum))));
                }
            }
            let union = match name {
                "__or__" => Some([obj, other]),
                "__ror__" => Some([other, obj]),
                _ => None,
            };
            if let Some([left, right]) = union
                && super::union::subclass_slot_pair(self, left, right)
            {
                return super::union::make(self, &[left, right]).map(Some);
            }
            // Overriding one multiplication method installs a numeric slot for both directions.
            if let Some(peer) = match name {
                "__mul__" => Some("__rmul__"),
                "__rmul__" => Some("__mul__"),
                _ => None,
            } && self.slot(obj, peer).is_some()
                && let Some(storage) = self.repeat_storage(obj)
            {
                return self
                    .repeat_sequence(obj, storage, other, RepeatMode::Wrapper)
                    .map(Some);
            }

            if name == "__ne__"
                && let Some(equal) = self.operator_slot(obj, "__eq__", other)?
            {
                return Ok(Some(if self.truth(equal)? { FALSE } else { TRUE }));
            }
            return Ok(None);
        };
        self.call_operator_slot(method, other)
    }

    pub(super) fn inherited_integer_slot(&mut self, a: Id, b: Id, code: u32) -> Result<Option<Id>> {
        let (Ok(a), Ok(b)) = (self.borrow_int(a), self.borrow_int(b)) else {
            return Ok(None);
        };
        let result = crate::integer_binary(&a, &b, code)?;
        Ok(Some(self.alloc(Value::Int(result))))
    }

    fn call_operator_slot(&mut self, method: Id, other: Id) -> Result<Option<Id>> {
        let result = self.call(method, vec![other], vec![])?;
        Ok((result != value::NOT_IMPLEMENTED).then_some(result))
    }

    /// Try divmod's numeric slots in order, including the inherited integer and float slots.
    pub(super) fn divmod(&mut self, a: Id, b: Id) -> Result<Id> {
        let class = |value| {
            self.heap
                .get(value)
                .instance_class()
                .or_else(|| self.calendar_slot_class(value, false))
                .or_else(|| {
                    self.builtin_type_name(value)
                        .and_then(|name| self.builtins.get(name).copied())
                })
        };
        let (left, right) = (class(a), class(b));
        let python_slot = |value| {
            self.calendar_slot_class(value, false).is_some()
                || self.slot(value, "__divmod__").is_some()
                || self.slot(value, "__rdivmod__").is_some()
        };
        let reflected_first = match (left, right) {
            (Some(left), Some(right))
                if left != right && self.direct_subclass(right, left) && python_slot(b) =>
            {
                !python_slot(a) || self.reflected_slot_overridden(left, right, "__rdivmod__")?
            }
            _ => false,
        };
        for reflected in [reflected_first, !reflected_first] {
            if reflected && left == right {
                continue;
            }
            let (receiver, hook, other) = if reflected {
                (b, "__rdivmod__", a)
            } else {
                (a, "__divmod__", b)
            };
            if self.calendar_slot_class(receiver, false).is_some()
                || self.slot(receiver, hook).is_some()
            {
                if let Some(result) = self.operator_slot(receiver, hook, other)? {
                    return Ok(result);
                }
            } else if let Value::Float(x) = *self.heap.get(self.scalar_value(receiver))
                && let Some(y) = self.real_component(self.scalar_value(other))?
            {
                let (left, right) = if reflected { (y, x) } else { (x, y) };
                if right == 0.0 {
                    return Err(Error::new("ZeroDivisionError", "division by zero"));
                }
                let (quotient, remainder) = crate::floats::divmod_nonzero(left, right);
                let quotient = self.alloc(Value::Float(quotient));
                let remainder = self.alloc(Value::Float(remainder));
                return Ok(self.alloc(Value::Tuple(vec![quotient, remainder])));
            } else if let (Ok(x), Ok(y)) = (self.as_int(a), self.as_int(b)) {
                let (quotient, remainder) = x.divmod(&y)?;
                let quotient = self.alloc(Value::Int(quotient));
                let remainder = self.alloc(Value::Int(remainder));
                return Ok(self.alloc(Value::Tuple(vec![quotient, remainder])));
            }
        }
        let type_name = |id| match self.heap.get(id) {
            Value::Calendar { .. } => stdlib::datetime::type_name(self, id),
            _ => self.type_name(id).into(),
        };
        Err(Error::new(
            "TypeError",
            format!(
                "unsupported operand type(s) for divmod(): '{}' and '{}'",
                type_name(a),
                type_name(b)
            ),
        ))
    }

    /// A Python implementation of either method installs a shared forward/reflected slot.
    fn has_python_binary_slot(&self, obj: Id, forward: &str, reflected: &str) -> bool {
        [forward, reflected].iter().any(|name| {
            self.slot(obj, name)
                .is_some_and(|slot| !matches!(self.heap.get(slot), Value::Native(_)))
        })
    }

    /// Order a subtype's reflected integer override against the effective slot on the left.
    fn integer_reflected_first(
        &mut self,
        left: Id,
        right: Id,
        forward: &str,
        reflected: &str,
    ) -> Result<bool> {
        if self.numeric_slot_override(right, reflected).is_none() {
            return Ok(false);
        }
        let owner = if self.numeric_slot_override(left, reflected).is_some() {
            left
        } else if self.numeric_slot_override(left, forward).is_some() {
            // Both classes have Python slots. Compare against the inherited int
            // method, even when another base after int declares the same name.
            self.builtin("int")
        } else {
            // A Python subtype slot precedes the built-in implementation.
            return Ok(true);
        };
        self.reflected_slot_overridden(owner, right, reflected)
    }

    /// Compare the attributes on the two classes after applying descriptor binding.
    pub(super) fn reflected_slot_overridden(
        &mut self,
        left: Id,
        right: Id,
        name: &str,
    ) -> Result<bool> {
        let right = match self.lookup_attr(right, name) {
            Ok(slot) => slot,
            Err(error) if self.is_attribute_error(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        let left = match self.lookup_attr(left, name) {
            Ok(slot) => slot,
            Err(error) if self.is_attribute_error(&error) => return Ok(true),
            Err(error) => return Err(error),
        };
        if left == right {
            return Ok(false);
        }
        let different = self.binary("!=", left, right)?;
        self.truth(different)
    }

    /// Compare through the native int or float slot, bypassing the receiver’s override.
    /// Float accepts either real payload. Int declines floats so they can reflect.
    pub(super) fn real_compare_slot(
        &mut self,
        op: &str,
        receiver: Id,
        other: Id,
    ) -> Result<Option<Id>> {
        let payload = self.scalar_value(other);
        if other != payload
            && let (Value::Float(number), Value::Int(integer)) =
                (self.heap.get(receiver), self.heap.get(payload))
            && number.is_finite()
            && (49..=1024).contains(&integer.bits())
        {
            // CPython compares equal-width, same-sign magnitudes above 48 bits
            // as integers. The original int subtype can observe that comparison.
            let whole = crate::floats::float_to_integer(*number)?;
            if whole.bits() == integer.bits() && whole.sign() == integer.big().sign() {
                let op = if number.fract() == 0.0 {
                    op
                } else {
                    match op {
                        "==" | "!=" => return Ok(Some(builtins::boolean(op == "!="))),
                        "<" | "<=" if *number > 0.0 => "<",
                        "<" | "<=" => "<=",
                        _ if *number > 0.0 => ">=",
                        _ => ">",
                    }
                };
                let left = self.alloc(Value::Int(crate::Int::new(whole)));
                let result = self.binary(op, left, other)?;
                return Ok(Some(builtins::boolean(self.truth(result)?)));
            }
        }
        match (self.heap.get(receiver), self.heap.get(payload)) {
            (Value::Int(_) | Value::Bool(_) | Value::Float(_), Value::Int(_) | Value::Bool(_))
            | (Value::Float(_), Value::Float(_)) => self.binary(op, receiver, payload).map(Some),
            _ => Ok(None),
        }
    }

    /// Call the first Python or native comparison slot in the receiver's MRO.
    fn builtin_compare_slot(
        &mut self,
        name: &str,
        a: Id,
        b: Id,
        native_comparison: bool,
    ) -> Result<Option<Id>> {
        if native_comparison {
            let receiver = self.heap.get(self.scalar_value(a));
            let other = self.heap.get(self.scalar_value(b));
            let compatible = match receiver {
                Value::Complex(_) => matches!(other, Value::Float(_)),
                Value::Set { .. } => matches!(other, Value::Set { .. }),
                Value::Calendar { value, .. } if value.class() != "timezone" => {
                    matches!(other, Value::Calendar { value: right, .. } if value.class() == right.class())
                }
                _ => return self.operator_slot(a, name, b),
            };
            let class = self.exact_type(a)?;
            let native = self.find_in_mro(class, |base| match self.heap.get(base) {
                Value::Native(kind)
                    if matches!(kind.as_str(), "complex" | "set" | "frozenset")
                        || stdlib::datetime::members(kind).is_some() =>
                {
                    Some(true)
                }
                _ => self.own_class_attr(base, name).map(|_| false),
            });
            if native != Some(true) {
                return self.operator_slot(a, name, b);
            }
            if !compatible {
                return Ok(None);
            }
        } else if let Some(slot) = self.slot(a, name) {
            // A class may bind int.__gt__ as __lt__. The descriptor selects the operation.
            if let Value::Native(symbol) = self.heap.get(slot)
                && let Some((kind, member)) = symbol.split_once('.')
                && matches!(kind, "int" | "float")
                && let Some(op) = comparison_op(member)
                && self.builtin_slot_accepts(a, kind)
            {
                return self.real_compare_slot(op, self.scalar_value(a), b);
            }
            return self.operator_slot(a, name, b);
        } else if matches!(
            self.heap.get(a),
            Value::Int(_) | Value::Bool(_) | Value::Float(_)
        ) {
            return self.real_compare_slot(
                comparison_op(name).expect("rich comparison slot"),
                a,
                b,
            );
        }
        let value = match name {
            "__eq__" | "__ne__" => {
                builtins::boolean(self.checked_equal_impl(a, b, false)? == (name == "__eq__"))
            }
            "__lt__" => self.order("<", a, b)?,
            "__le__" => self.order("<=", a, b)?,
            "__gt__" => self.order(">", a, b)?,
            "__ge__" => self.order(">=", a, b)?,
            _ => unreachable!("rich comparison slot"),
        };
        Ok(Some(value))
    }

    pub(super) fn binary_special(&mut self, op: &str, a: Id, b: Id) -> Result<Option<Id>> {
        self.dispatch_binary_slots(op, a, b, false)
    }

    fn dispatch_binary_slots(
        &mut self,
        op: &str,
        a: Id,
        b: Id,
        assignment: bool,
    ) -> Result<Option<Id>> {
        // Keep inherited dict comparisons on the iterative container walk, including cycles.
        if op == "=="
            && [a, b].into_iter().all(|value| {
                self.dict_storage(value).is_some()
                    && self
                        .slot(value, "__eq__")
                        .is_none_or(|slot| Some(&slot) == self.builtins.get("dict.__eq__"))
            })
        {
            return Ok(None);
        }
        if matches!(op, "==" | "!=")
            && let (Value::ParamSpecMarker(left, _), Value::ParamSpecMarker(right, _)) =
                (self.heap.get(a), self.heap.get(b))
            && left == right
        {
            return super::paramspec::compare(self, op, a, b).map(Some);
        }
        if matches!(op, "==" | "!=")
            && let Value::MemoryView { owner, .. } = self.heap.get(a)
        {
            if owner.is_some() {
                self.check_buffer_exporter(b)?;
            }
            if owner.is_none()
                || matches!(
                    self.heap.get(self.scalar_value(b)),
                    Value::Bytes(_)
                        | Value::ByteArray(_)
                        | Value::MemoryView { .. }
                        | Value::Array(_)
                )
            {
                // A live floating-point view compares its elements even with itself:
                // a NaN is unequal. Container identity shortcuts still apply separately.
                let equal = self.memoryview_equal(a, b);
                return Ok(Some(builtins::boolean(equal == (op == "=="))));
            }
        }
        if matches!(op, "==" | "!=")
            && matches!(self.heap.get(a), Value::Deque(_))
            && matches!(self.heap.get(b), Value::Deque(_))
        {
            return deque::compare(self, op, a, b).map(|result| Some(builtins::boolean(result)));
        }
        if matches!(op, "<" | "<=" | ">" | ">=")
            && (matches!(self.heap.get(a), Value::MappingProxy(_))
                || matches!(self.heap.get(b), Value::MappingProxy(_)))
        {
            return self
                .with_proxy_operands(a, b, |ctx| {
                    let left = ctx.mapping_proxy_target(a);
                    let right = ctx.mapping_proxy_target(b);
                    if b == right {
                        return ctx.binary(op, left, b);
                    }
                    let (forward, reflected, _) = binary_slots(op).unwrap();
                    if let Some(result) = ctx.operator_slot(left, forward, b)? {
                        return Ok(result);
                    }
                    ctx.binary(comparison_op(reflected).unwrap(), right, left)
                })
                .map(Some);
        }
        let (a, b) = if matches!(op, "==" | "!=") {
            let left = self.mapping_proxy_target(a);
            let right = self.mapping_proxy_target(b);
            if left != a || right != b {
                // A mapping hook can drop the containers that own either original proxy.
                self.heap.reserve_temporaries(2)?;
                self.heap.keep(a);
                self.heap.keep(b);
            }
            if right != b {
                // A left-hand mapping or Python hook first sees the right proxy itself.
                let slot = if op == "==" { "__eq__" } else { "__ne__" };
                if let Some(result) = self.operator_slot(left, slot, b)? {
                    return Ok(Some(result));
                }
                // Reflected mappingproxy comparison starts from its own backing mapping.
                (right, left)
            } else {
                (left, b)
            }
        } else {
            (a, b)
        };
        if self.heap.has_weak_values
            && matches!(op, "==" | "!=")
            && let (Value::Weak(left), Value::Weak(right)) = (self.heap.get(a), self.heap.get(b))
            && left.kind == weakref::Kind::Reference
            && right.kind == weakref::Kind::Reference
            && let (Some(left), Some(right)) = (left.target, right.target)
        {
            self.heap.keep(left);
            self.heap.keep(right);
            if op == "!=" && !self.has_custom_ne(left) && !self.has_custom_ne(right) {
                let equal = self.binary("==", left, right)?;
                return Ok(Some(if self.truth(equal)? { FALSE } else { TRUE }));
            }
            return self.binary(op, left, right).map(Some);
        }
        let Some((left, right, inplace)) = binary_slots(op) else {
            return Ok(None);
        };
        if matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
            let set = |id| matches!(self.heap.get(self.scalar_value(id)), Value::Set { .. });
            let operands = if self.set_view(a).is_some() && self.set_view(b).is_some() {
                let left_type = self.exact_type(a)?;
                let right_type = self.exact_type(b)?;
                // A right-hand OrderedDict view inherits the ordinary view's reflected slot.
                if left_type != right_type && self.direct_subclass(right_type, left_type) {
                    Some((b, a, right))
                } else {
                    Some((a, b, left))
                }
            } else if self.set_view(a).is_some() && set(b) {
                Some((a, b, left))
            } else if self.set_view(b).is_some() && set(a) {
                // A set subclass can handle the comparison before the view's reflected slot.
                if self.overrides_set_slot(a, left)
                    && let Some(result) = self.operator_slot(a, left, b)?
                {
                    return Ok(Some(result));
                }
                Some((b, a, right))
            } else {
                None
            };
            if let Some((view, other, slot)) = operands {
                let result = self
                    .with_slot_depth("maximum recursion depth exceeded in comparison", |ctx| {
                        ctx.compare_view(slot, view, other)
                    })?;
                return Ok(Some(builtins::boolean(result)));
            }
        }
        let class = |id| {
            self.heap.get(id).instance_class().or_else(|| {
                if matches!(op, "<" | "<=" | ">" | ">=")
                    && matches!(self.heap.get(id), Value::Deque(_))
                {
                    self.builtins.get(deque::CLASS).copied()
                } else if matches!(op, "+" | "-" | "*" | "/" | "//" | "%") {
                    self.calendar_slot_class(id, matches!(op, "+" | "-"))
                } else {
                    None
                }
            })
        };
        let (mut ac, mut bc) = (class(a), class(b));
        let mixed_division = op == "/" && (ac.is_some() || bc.is_some());
        let set_xor = op == "^"
            && [a, b]
                .into_iter()
                .any(|id| matches!(self.heap.get(self.scalar_value(id)), Value::Set { .. }));
        let dict_union = op == "|" && [a, b].into_iter().any(|id| self.dict_storage(id).is_some());
        // An exact str has a numeric remainder slot. Order it against Python methods,
        // but leave pairs of exact builtins on the direct formatting path.
        let mixed_remainder = op == "%" && (ac.is_some() || bc.is_some());
        let shift = matches!(op, "<<" | ">>");
        let integer_slot = integer_binary_code(op).filter(|_| INHERITED_INTEGER_OPS.contains(&op));
        let mixed_integer_slots = (op == "+" || integer_slot.is_some())
            && (ac.is_some() || bc.is_some())
            && [a, b]
                .into_iter()
                .all(|id| self.builtin_slot_accepts(id, "int"));
        if set_xor
            || dict_union
            || mixed_remainder
            || mixed_integer_slots
            || shift
            || matches!(op, "*" | "/" | "**")
        {
            for (value, class) in [(a, &mut ac), (b, &mut bc)] {
                if matches!(
                    (op, self.heap.get(value)),
                    ("*" | "/" | "**", Value::Complex(_))
                        | ("%", Value::Str(_) | Value::SurrogateStr(_))
                        | ("^", Value::Set { .. })
                        | ("|", Value::Dict(_) | Value::DefaultDict { .. })
                ) || ((op == "+" || integer_slot.is_some())
                    && matches!(self.heap.get(value), Value::Int(_) | Value::Bool(_)))
                    || (mixed_division && matches!(self.heap.get(value), Value::Float(_)))
                {
                    *class = Some(self.exact_type(value)?);
                }
            }
        }
        if ac.is_none()
            && bc.is_none()
            && !shift
            && !(inplace.is_empty()
                && [a, b]
                    .iter()
                    .any(|id| matches!(self.heap.get(*id), Value::KeyWrapper { .. })))
        {
            return Ok(None);
        }
        let native_comparison = match op {
            // Complex accepts float equality before an unrelated float subtype can reflect.
            "==" | "!=" => matches!(
                (
                    self.heap.get(self.scalar_value(a)),
                    self.heap.get(self.scalar_value(b))
                ),
                (Value::Complex(_), Value::Float(_)) | (Value::Float(_), Value::Complex(_))
            ),
            "<" | "<=" | ">" | ">=" => {
                [a, b]
                    .into_iter()
                    .any(|id| match self.heap.get(self.scalar_value(id)) {
                        Value::Set { .. } | Value::Deque(_) => true,
                        Value::Calendar { value, .. } => value.class() != "timezone",
                        _ => false,
                    })
            }
            _ => false,
        };
        let real_pair = inplace.is_empty()
            && [a, b].into_iter().all(|value| {
                matches!(
                    self.heap.get(self.scalar_value(value)),
                    Value::Int(_) | Value::Bool(_) | Value::Float(_)
                )
            });
        // Once both string equality slots decline, distinct objects compare unequal
        // even when their underlying spellings are identical.
        let text_equality = matches!(op, "==" | "!=")
            && self.string_storage(a).is_some()
            && self.string_storage(b).is_some();
        if real_pair
            || native_comparison
            || text_equality
            || (inplace.is_empty() && self.same_sequence_family(a, b))
        {
            let ac = self.exact_type(a)?;
            let bc = self.exact_type(b)?;
            // The proper subtype's reflected slot has priority, even when it is inherited.
            let slots = if ac != bc && self.direct_subclass(bc, ac) {
                [(right, b, a), (left, a, b)]
            } else {
                [(left, a, b), (right, b, a)]
            };
            for (name, left, right) in slots {
                if let Some(result) =
                    self.builtin_compare_slot(name, left, right, native_comparison)?
                {
                    return Ok(Some(result));
                }
            }
            return match op {
                "==" => Ok(Some(builtins::boolean(a == b))),
                "!=" => Ok(Some(builtins::boolean(a != b))),
                _ => Err(self.unsupported_order(op, a, b)),
            };
        }
        let reflected_first = if let (Some(ac), Some(bc)) = (ac, bc) {
            let complex_pair = matches!(op, "*" | "/" | "**")
                && [a, b]
                    .into_iter()
                    .all(|id| matches!(self.heap.get(self.scalar_value(id)), Value::Complex(_)));
            let float_operands = [a, b].map(|id| {
                mixed_division && matches!(self.heap.get(self.scalar_value(id)), Value::Float(_))
            });
            let strict_subclass = ac != bc
                && if set_xor
                    || dict_union
                    || complex_pair
                    || mixed_integer_slots
                    || float_operands.contains(&true)
                {
                    self.direct_subclass(bc, ac)
                } else {
                    self.subclass(bc, ac)
                };
            strict_subclass
                && if mixed_integer_slots && integer_slot.is_some() {
                    self.integer_reflected_first(ac, bc, left, right)?
                } else if float_operands == [true, true] {
                    // Restoring both original descriptors restores the native division slot.
                    let float = self.exact_type(self.scalar_value(a))?;
                    let [left_native, right_native] = [ac, bc].map(|class| {
                        [left, right].into_iter().all(|name| {
                            self.numeric_slot_override(class, name)
                                .is_none_or(|method| {
                                    self.own_class_attr(float, name) == Some(method)
                                })
                        })
                    });
                    if left_native || right_native {
                        left_native != right_native
                    } else {
                        self.reflected_slot_overridden(ac, bc, right)?
                    }
                } else if (dict_union || complex_pair)
                    && self.has_python_binary_slot(a, left, right)
                    && self.has_python_binary_slot(b, left, right)
                {
                    self.reflected_slot_overridden(ac, bc, right)?
                } else {
                    inplace.is_empty() || self.class_attr(bc, right) != self.class_attr(ac, right)
                }
        } else {
            false
        };
        let slot = |ctx: &mut Self, receiver, name: &str, other| {
            if let Some(code) = integer_slot
                && ctx.builtin_slot_accepts(receiver, "int")
            {
                if let Some(method) = ctx.bound_numeric_slot(receiver, name)? {
                    return ctx.call_operator_slot(method, other);
                }
                // In either direction the inherited int slot uses the original operand order.
                return ctx.inherited_integer_slot(a, b, code);
            }
            ctx.operator_slot(receiver, name, other)
        };
        if reflected_first && let Some(result) = slot(self, b, right, a)? {
            return Ok(Some(result));
        }
        if let Some(result) = slot(self, a, left, b)? {
            return Ok(Some(result));
        }
        if matches!(op, "==" | "!=")
            && matches!(self.heap.get(b), Value::MemoryView { owner: Some(_), .. })
        {
            self.check_buffer_exporter(a)?;
        }
        if !reflected_first
            && (ac != bc || inplace.is_empty())
            && let Some(result) = slot(self, b, right, a)?
        {
            return Ok(Some(result));
        }
        if mixed_integer_slots || shift {
            return Err(self.unsupported_binary(op, a, b, assignment));
        }
        // Both rich comparison methods declined. Keep the explicit numeric boundary,
        // but let unrelated operands use the normal identity or ordering fallback.
        if matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
            && [(ac, b), (bc, a)].into_iter().any(|(class, other)| {
                class.is_some_and(|class| {
                    self.is_bundled_decimal(class) && self.decimal_numeric_peer(class, other)
                })
            })
        {
            return Err(Error::new(
                "NotImplementedError",
                "native Decimal numeric comparisons are unsupported",
            ));
        }
        Ok(None)
    }

    fn decimal_numeric_peer(&self, decimal: Id, other: Id) -> bool {
        if matches!(
            self.heap.get(self.scalar_value(other)),
            Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Complex(_)
        ) {
            return true;
        }
        let Some(class) = self.heap.get(other).instance_class() else {
            return false;
        };
        if self.is_bundled_decimal(class) || self.direct_subclass(class, decimal) {
            return true;
        }
        if let Some(numbers) = self.dict_named_item(self.modules, "numbers")
            && let Value::Module { scope, .. } = *self.heap.get(numbers)
            && let Some(rational) = self.scope_name(scope, "Rational")
        {
            return self.instance_of(other, rational);
        }
        false
    }

    pub(super) fn inplace(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        let target = self.weak_proxy_target(a)?;
        if target != a {
            let right = self.weak_proxy_target(b)?;
            return self.with_proxy_operands(target, right, |ctx| ctx.inplace(op, target, right));
        }
        if op == "|" && matches!(self.heap.get(a), Value::MappingProxy(_)) {
            return builtins::call(self, "mappingproxy.__ior__", &[a, b], &[]);
        }
        if let Some((_, _, slot)) = binary_slots(op)
            && let Some(result) = self.operator_slot(a, slot, b)?
        {
            return Ok(result);
        }
        // A mutable set's in-place slot runs before the right operand's reflected slot.
        if matches!(op, "|" | "-" | "&" | "^")
            && matches!(self.heap.get(a), Value::Set { frozen: false, .. })
            && let right = self.scalar_value(b)
            && matches!(self.heap.get(right), Value::Set { .. })
        {
            return self.set_binary(op, a, right, true);
        }
        if op == "|" && matches!(self.heap.get(a), Value::Dict(_) | Value::DefaultDict { .. }) {
            builtins::update_dict(self, a, Some(b), &[])?;
            return Ok(a);
        }
        self.binary_operation(op, a, b, true)
    }

    pub(super) fn finalize_removed_dict_value(&mut self, removed: Id) {
        let collect = self.heap.tracks_finalizers()
            && self.heap.could_own_finalizer(removed, true)
            && self.heap.has_unbound_finalizer(NONE);
        self.release_list_owner(removed);
        if collect {
            self.collect_unreachable();
        }
    }

    pub(super) fn delete_item(&mut self, obj: Id, key: Id) -> Result<()> {
        if matches!(self.heap.get(obj), Value::MemoryView { .. }) {
            return self.memoryview_delete(obj);
        }
        let target = self.weak_proxy_target(obj)?;
        if target != obj {
            return self.with_weak_target(target, |ctx| ctx.delete_item(target, key));
        }
        if matches!(self.heap.get(obj), Value::Deque(_)) {
            return deque::IndexOp::Delete
                .apply(self, obj, key, false)
                .map(|_| ());
        }
        if let Some(method) = self.bound_slot(obj, "__delitem__")? {
            self.call(method, vec![key], vec![])?;
            return Ok(());
        }
        if let Some(storage) = self.mutable_sequence_storage(obj) {
            return self.delete_item(storage, key);
        }
        if let Some(dict) = self.dict_storage(obj) {
            let removed = builtins::remove_dict_item(self, dict, key)?
                .ok_or_else(|| self.lookup_error("KeyError", key))?;
            self.finalize_removed_dict_value(removed);
            return Ok(());
        }
        match self.heap.get(obj) {
            Value::ByteArray(_) => {
                if let Value::Slice(parts) = *self.heap.get(key) {
                    return self.set_slice(obj, parts, None);
                }
                let i = self.mutation_index(key, "bytearray")?;
                let len = self.bytes_like(obj)?.len() as i64;
                let i = if i < 0 { i + len } else { i };
                if !(0..len).contains(&i) {
                    return Err(Error::new("IndexError", "bytearray index out of range"));
                }
                self.check_bytearray_resize(obj)?;
                let Value::ByteArray(data) = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                data.remove(i as usize);
            }
            Value::List(_) => {
                if let Value::Slice(parts) = *self.heap.get(key) {
                    return self.set_list_slice(obj, parts, None);
                }
                let i = self.list_mutation_position(obj, key)?;
                let Value::List(entries) = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                let value = entries.remove(i);
                self.heap.weak_edge_removed(value);
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "'{}' object doesn't support item deletion",
                        self.type_name(obj)
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Compute the new sequence length before allocating or asking a bytearray to resize.
fn repeated_len(len: usize, times: isize) -> Result<usize> {
    len.checked_mul(times.max(0) as usize)
        .filter(|&size| size <= isize::MAX as usize)
        .ok_or_else(|| Error::new("MemoryError", ""))
}

fn repeat_values<T: Copy>(values: &[T], times: isize) -> Result<Vec<T>> {
    let size = repeated_len(values.len(), times)?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", ""))?;
    for _ in 0..times {
        output.extend_from_slice(values);
    }
    Ok(output)
}
