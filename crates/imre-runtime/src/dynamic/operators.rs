use std::cmp::Ordering;

use num_traits::Signed;

use super::*;

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
    BuiltinContainer { object: Id, storage: Id },
    Exception(&'a Error),
}

enum FormatPart {
    Value(Id, bool),
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
    DefaultDictEnd(String),
    SetMembers(Vec<Id>, usize),
    Fields(Id, usize),
    Leave(Id, &'static str),
    ContextEnd(Id),
    SliceEnd,
    NamedTupleEnd,
}

impl FormatPart {
    /// Snapshot the member identities before any element repr can mutate the set.
    fn set_members(entries: &[(Id, Id)]) -> Result<Self> {
        let mut members = Vec::new();
        members
            .try_reserve_exact(entries.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        members.extend(entries.iter().map(|&(key, _)| key));
        Ok(Self::SetMembers(members, 0))
    }

    /// Values a suspended renderer will read after a Python callback returns.
    fn keep_alive(&mut self, heap: &mut Heap) {
        match self {
            Self::Value(id, _)
            | Self::TypeArgument(id, _)
            | Self::TypeList(id, _, _)
            | Self::TypeEnd(id, _, _)
            | Self::SyntaxMessage(id)
            | Self::SyntaxEnd(id)
            | Self::Sequence(id, _)
            | Self::Entries(id, _, _)
            | Self::DefaultDictFactory(id, _)
            | Self::Fields(id, _)
            | Self::Leave(id, _)
            | Self::ContextEnd(id) => heap.keep(*id),
            Self::SetMembers(members, index) => {
                // One traced owner keeps the snapshot alive across every following callback.
                let tuple = heap.alloc(Value::Tuple(std::mem::take(members)));
                *self = Self::Sequence(tuple, *index);
            }
            Self::Suffix(_)
            | Self::Text(_)
            | Self::Closing(_)
            | Self::SliceEnd
            | Self::NamedTupleEnd
            | Self::DefaultDictEnd(_) => {}
        }
    }
}

fn slice_positions(start: i64, end: i64, step: i64) -> impl Iterator<Item = usize> + Clone {
    std::iter::successors(Some(start), move |&i| i.checked_add(step))
        .take_while(move |&i| if step > 0 { i < end } else { i > end })
        .map(|i| i as usize)
}

impl Context {
    pub(super) fn sequence(&self, id: Id) -> Option<Vec<Id>> {
        match self.heap.get(id) {
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => {
                Some(v.clone())
            }
            Value::Instance { class, fields, .. } => Some(
                self.namedtuple_fields(*class)?
                    .iter()
                    .map(|k| fields[k])
                    .collect(),
            ),
            _ => None,
        }
    }
    fn sequence_at(&self, id: Id, index: usize) -> Option<(usize, Option<Id>)> {
        match self.heap.get(id) {
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => {
                Some((v.len(), v.get(index).copied()))
            }
            Value::Instance { class, fields, .. } => {
                let names = self.namedtuple_fields(*class)?;
                Some((names.len(), names.get(index).map(|key| fields[key])))
            }
            _ => None,
        }
    }

    pub(super) fn optional_truth(&mut self, value: Option<Id>, default: bool) -> Result<bool> {
        value.map_or(Ok(default), |id| self.truth(id))
    }

    pub(super) fn index_value(&mut self, value: Id) -> Result<Option<crate::Int>> {
        if let Ok(integer) = self.as_int(value) {
            return Ok(Some(integer));
        }
        let Some(index) = self.bound_slot(value, "__index__")? else {
            return Ok(None);
        };
        let result = self.call(index, Vec::new(), Vec::new())?;
        self.as_int(result).map(Some).map_err(|_| {
            Error::new(
                "TypeError",
                format!(
                    "__index__ returned non-int (type {})",
                    self.type_name(result)
                ),
            )
        })
    }

    /// Resolve Python's integer-index protocol without rounding or truncating a numeric value.
    pub(super) fn index_integer(&mut self, value: Id) -> Result<crate::Int> {
        self.index_value(value)?.ok_or_else(|| {
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
            return Ok(self
                .sequence_payload(id)
                .map(|(_, items)| items.len())
                .or_else(|| {
                    self.string_storage(id)
                        .map(|text| self.str_value(text).unwrap().chars().count())
                }));
        };
        let value = self.call(hook, Vec::new(), Vec::new())?;
        let length = self.index_integer(value)?;
        let length = length
            .to_i64()
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                )
            })?;
        if length < 0 {
            return Err(Error::new("ValueError", "__len__() should return >= 0"));
        }
        Ok(Some(length as usize))
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
            Value::Bytes(b) | Value::ByteArray(b) => !b.is_empty(),
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => !v.is_empty(),
            Value::Set { entries: v, .. }
            | Value::Dict(v)
            | Value::DefaultDict { entries: v, .. } => !v.is_empty(),
            Value::MappingView { mapping, .. } => {
                let mapping = *mapping;
                return self.truth(mapping);
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
            Value::Range(range) => range.len.as_small() != Some(0),
            Value::Instance { .. } => {
                if let Some(hook) = self.bound_slot(id, "__bool__")? {
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

    /// Unwrap proxy layers before the existing mapping comparison or reflected Python slot.
    fn mapping_comparison_target(&self, mut value: Id) -> Id {
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
                    && (matches!(self.heap.get(a), Value::Instance { .. })
                        || matches!(self.heap.get(b), Value::Instance { .. }))
                {
                    self.heap.keep(a);
                    self.heap.keep(b);
                }
                if check_special && let Some(result) = self.binary_special("==", a, b)? {
                    break 'current self.truth(result)?;
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
                let left = self.mapping_comparison_target(a);
                let right = self.mapping_comparison_target(b);
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
                    (self.heap.get(a), self.heap.get(b)),
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
            (Value::Float(f), Value::Int(_) | Value::Bool(_)) => {
                crate::floats::compare(&self.as_int(b).unwrap(), *f) == 0
            }
            (Value::Int(_) | Value::Bool(_), Value::Float(f)) => {
                crate::floats::compare(&self.as_int(a).unwrap(), *f) == 0
            }
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a) | Value::ByteArray(a), Value::Bytes(b) | Value::ByteArray(b)) => {
                a == b
            }
            (Value::Method { .. }, Value::Method { .. }) => self
                .method_equality_operands(a, b)
                .is_some_and(|(x, y, same)| same && equal(x, y)),
            (Value::Native(a), Value::Native(b)) => a == b,
            (Value::GenericAlias(a), Value::GenericAlias(b)) => {
                a.comparison_parts(b).is_some_and(|(x, y)| equal(x, y))
            }
            (Value::Slice(x), Value::Slice(y)) => x.iter().zip(y).all(|(&i, &j)| equal(i, j)),
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
    pub(super) fn compare(&mut self, op: &str, a: Id, b: Id) -> Result<Option<Ordering>> {
        let unsupported = || {
            Error::new(
                "TypeError",
                format!(
                    "'{op}' not supported between instances of '{}' and '{}'",
                    self.type_name(a),
                    self.type_name(b)
                ),
            )
        };
        let (a, b) = (self.scalar_value(a), self.scalar_value(b));
        let int_float = |integer, float| {
            let order = crate::floats::compare(&self.as_int(integer)?, float);
            Ok((order != 2).then(|| order.cmp(&0)))
        };
        match (self.heap.get(a), self.heap.get(b)) {
            (Value::Int(_) | Value::Bool(_), Value::Int(_) | Value::Bool(_)) => {
                Ok(Some(self.as_int(a)?.cmp(&self.as_int(b)?)))
            }
            (Value::Float(x), Value::Float(y)) => Ok(x.partial_cmp(y)),
            (Value::Calendar { value: a, .. }, Value::Calendar { value: b, .. })
                if a.class() == "time" && b.class() == "time" =>
            {
                Ok(Some(a.clock_order(b)?))
            }
            (Value::Int(_) | Value::Bool(_), Value::Float(f)) => int_float(a, *f),
            (Value::Float(f), Value::Int(_) | Value::Bool(_)) => {
                Ok(int_float(b, *f)?.map(Ordering::reverse))
            }
            (Value::Str(x), Value::Str(y)) => Ok(Some(x.cmp(y))),
            (Value::Bytes(x) | Value::ByteArray(x), Value::Bytes(y) | Value::ByteArray(y)) => {
                Ok(Some(x.cmp(y)))
            }
            _ => {
                if matches!(self.heap.get(a), Value::List(_))
                    != matches!(self.heap.get(b), Value::List(_))
                {
                    return Err(unsupported());
                }
                if let (Some(x), Some(y)) = (self.sequence(a), self.sequence(b)) {
                    for (&i, &j) in x.iter().zip(&y) {
                        if !self.checked_equal(i, j)? {
                            return self.compare(op, i, j);
                        }
                    }
                    Ok(Some(x.len().cmp(&y.len())))
                } else {
                    Err(unsupported())
                }
            }
        }
    }
    pub(super) fn display(&self, id: Id) -> String {
        Self::render_containers(
            self,
            RenderRoot::Object(id),
            false,
            false,
            |_, _, _, _, _| Ok(None),
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
            |_, _, _, _, _| Ok(None),
        )
        .unwrap_or_else(|_| "<exception repr() failed>".to_owned())
    }

    /// Keep the boxed result of str(arg) when an exception has one positional argument.
    pub(super) fn single_exception_str(&mut self, root: Id) -> Result<Option<Id>> {
        let mut value = root;
        let mut seen = std::collections::HashSet::new();
        while let Value::Raised(error) = self.heap.get(value) {
            if error.os_args.is_some()
                || error.trace.as_ref().is_some_and(|trace| {
                    trace
                        .import_message
                        .is_some_and(|msg| matches!(self.heap.get(msg), Value::Str(_)))
                        || trace.source_syntax.is_some()
                })
                || self.is_builtin_error(error, "KeyError")
                || error
                    .class
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
        self.checked_render(RenderRoot::BuiltinContainer { object, storage }, true)
    }

    fn checked_render(&mut self, root: RenderRoot<'_>, quoted: bool) -> Result<String> {
        Self::render_containers(
            self,
            root,
            quoted,
            true,
            |ctx, id, quoted, work, ancestors| ctx.element_format(id, quoted, work, ancestors),
        )
    }

    /// Resolve formatting methods on heap instances and exceptions that retain their Python class.
    fn format_slot(&self, id: Id, name: &str) -> Option<(Id, Id)> {
        let class = match self.heap.get(id) {
            Value::Instance { class, .. }
            | Value::Raised(Error {
                class: Some(class), ..
            }) => *class,
            _ => return None,
        };
        self.class_attr(class, name).map(|slot| (class, slot))
    }

    /// Invoke the special method once and preserve an accepted string subclass.
    pub(super) fn repr_override(&mut self, id: Id) -> Result<Option<Id>> {
        let Some((class, slot)) = self.format_slot(id, "__repr__") else {
            return Ok(None);
        };
        let hook = self.bind_attribute(slot, id, class)?;
        let result = self.call(hook, Vec::new(), Vec::new())?;
        if self.str_value(result).is_err() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__repr__ returned non-string (type {})",
                    self.type_name(result)
                ),
            ));
        }
        Ok(Some(result))
    }

    /// Keep pending children alive while a hook mutates and reenters its container.
    fn element_format(
        &mut self,
        id: Id,
        quoted: bool,
        work: &mut [FormatPart],
        ancestors: &[Id],
    ) -> Result<Option<String>> {
        if self.format_slot(id, "__repr__").is_none()
            && (quoted || self.format_slot(id, "__str__").is_none())
        {
            return Ok(None);
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
        let converted = if quoted {
            self.repr_override(id)
        } else {
            builtins::call(self, "str", &[id], &[]).map(Some)
        };
        let result = converted.and_then(|value| {
            value
                .map(|s| {
                    let mut text = String::new();
                    append_rendered_text(&mut text, self.str_value(s)?)?;
                    Ok(text)
                })
                .transpose()
        });
        self.repr_ancestors.truncate(depth);
        self.heap.release_temporaries_since(mark);
        result
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
            if repr {
                append_rendered_text(output, error.kind.rsplit('.').next().unwrap_or(&error.kind))?;
            } else if let Some(message) =
                error.trace.as_ref().and_then(|trace| trace.import_message)
                && let Value::Str(text) = self.heap.get(message)
            {
                append_rendered_text(output, text)?;
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
            bool,
            &mut [FormatPart],
            &[Id],
        ) -> Result<Option<String>>,
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
            RenderRoot::BuiltinContainer { object, storage } => {
                let (open, end, cycle, contents) = match ctx.heap.get(storage) {
                    Value::Set { entries, frozen } => {
                        let name = ctx.type_name(object);
                        let recursive = ancestors.contains(&object);
                        if recursive || entries.is_empty() {
                            append_rendered_text(&mut output, name)?;
                            append_rendered_text(
                                &mut output,
                                if recursive { "(...)" } else { "()" },
                            )?;
                            return Ok(output);
                        }
                        let wrapped = *frozen || object != storage;
                        if wrapped {
                            append_rendered_text(&mut output, name)?;
                            append_rendered_text(&mut output, "(")?;
                        }
                        (
                            "{",
                            if wrapped { "})" } else { "}" },
                            "set(...)",
                            Part::set_members(entries)?,
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
                if !enter_ancestor(&mut ancestors, object)? {
                    append_rendered_text(&mut output, cycle)?;
                    return Ok(output);
                }
                append_rendered_text(&mut output, open)?;
                push_frame(&mut active, object)?;
                push_frame(&mut work, Part::Leave(object, end))?;
                push_frame(&mut work, contents)?;
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
        const STACK_MARGIN_WORDS: usize = 2048;
        let stack_words = checked
            .then(stacker::remaining_stack)
            .flatten()
            .map(|bytes| (bytes / size_of::<usize>()).saturating_sub(2 * STACK_MARGIN_WORDS));
        let slice_budget = stack_words.map(|words| words / SLICE_FRAME_WORDS);
        let sequence_budget = stack_words.map(|words| words / SEQUENCE_FRAME_WORDS);
        let mut slice_depth = 0usize;
        let mut namedtuple_depth = 0usize;
        // Pinned CPython 3.14 uses 192 native bytes per nested builtin alias with
        // an exact list of arguments. A Literal or Final adds three Python frames
        // and 1,264 native bytes when formatting the same nesting on x86-64.
        let type_budget = stack_words.unwrap_or(1024 * 1024);
        let mut type_words = 0usize;
        let mut typing_frames = 0usize;
        while let Some(part) = work.pop() {
            match part {
                Part::Closing(count) => close_parens(&mut output, count)?,
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
                Part::TypeArgument(id, direct_list) => {
                    if direct_list && let Value::List(items) = ctx.heap.get(id) {
                        append_rendered_text(&mut output, "[")?;
                        push_frame(&mut work, Part::Text("]"))?;
                        push_frame(&mut work, Part::TypeList(id, 0, items.len()))?;
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
                    let Value::Instance { class, fields, .. } = ctx.heap.get(id) else {
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
                        push_frame(
                            &mut work,
                            Part::Value(
                                *fields
                                    .get(name)
                                    .ok_or_else(|| Error::new("AttributeError", name.clone()))?,
                                true,
                            ),
                        )?;
                    }
                }
                Part::SetMembers(members, index) => {
                    if let Some(&member) = members.get(index) {
                        if index != 0 {
                            append_rendered_text(&mut output, ", ")?;
                        }
                        push_frame(&mut work, Part::SetMembers(members, index + 1))?;
                        push_frame(&mut work, Part::Value(member, true))?;
                    }
                }
                Part::DefaultDictFactory(id, start) => {
                    // A key or value repr may have replaced the factory while rendering the table.
                    let Value::DefaultDict { factory, .. } = *ctx.heap.get(id) else {
                        unreachable!()
                    };
                    let mut contents = String::new();
                    append_rendered_text(&mut contents, &output[start..])?;
                    output.truncate(start);
                    append_rendered_text(&mut output, "defaultdict(")?;
                    push_frame(&mut work, Part::DefaultDictEnd(contents))?;
                    if enter_ancestor(&mut ancestors, factory)? {
                        push_frame(&mut active, factory)?;
                        push_frame(&mut work, Part::Leave(factory, ""))?;
                        push_frame(&mut work, Part::Value(factory, true))?;
                    } else {
                        append_rendered_text(&mut output, "...")?;
                    }
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
                    if let Some(text) = element_repr(&mut ctx, id, false, &mut work, &active)? {
                        append_rendered_text(&mut output, &text)?;
                    } else if let Some(storage) = ctx.string_storage(id) {
                        let Value::Str(message) = ctx.heap.get(storage) else {
                            unreachable!()
                        };
                        append_rendered_text(&mut output, message)?;
                    } else {
                        push_frame(&mut work, Part::Value(id, false))?;
                    }
                }
                Part::SyntaxEnd(id) => {
                    syntax.remove(&id);
                }
                Part::Suffix(suffix) => append_rendered_text(&mut output, &suffix)?,
                Part::Value(id, repr) => {
                    if checked
                        && repr
                        && let Some(text) = element_repr(&mut ctx, id, true, &mut work, &active)?
                    {
                        append_rendered_text(&mut output, &text)?;
                        continue;
                    }
                    match ctx.heap.get(id) {
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
                            let callable =
                                matches!(alias.kind, AliasKind::Callable | AliasKind::AbcCallable)
                                    && args.first().is_some_and(|&arg| {
                                        !matches!(ctx.heap.get(arg), Value::Ellipsis)
                                    });
                            if args.is_empty() {
                                work.push(Part::Text("()"));
                            }
                            for (index, &arg) in args.iter().enumerate().rev() {
                                work.push(Part::TypeArgument(
                                    arg,
                                    alias.kind == AliasKind::Builtin,
                                ));
                                if callable && index + 1 == args.len() {
                                    work.push(Part::Text("], "));
                                } else if index != 0 {
                                    work.push(Part::Text(", "));
                                }
                            }
                            work.push(Part::Text(if callable { "[[" } else { "[" }));
                            work.push(match alias.kind {
                                AliasKind::Callable => Part::Text("typing.Callable"),
                                AliasKind::Literal(_) => Part::Text("typing.Literal"),
                                _ => Part::TypeArgument(alias.origin, false),
                            });
                            continue;
                        }
                        Value::Raised(error) => {
                            if !repr
                                && let Some(fields) = error
                                    .trace
                                    .as_ref()
                                    .and_then(|trace| trace.source_syntax.as_ref())
                            {
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
                                let suffix = ctx.syntax_suffix(fields[1].get(), fields[2].get())?;
                                push_frame(&mut work, Part::SyntaxEnd(id))?;
                                push_frame(&mut work, Part::Suffix(suffix))?;
                                push_frame(&mut work, Part::SyntaxMessage(fields[0].get()))?;
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
                        Value::Instance { class, fields, .. }
                            if let Value::Class {
                                name,
                                namedtuple_fields: Some(names),
                                ..
                            } = ctx.heap.get(*class)
                                && names.iter().all(|field| fields.contains_key(field)) =>
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
                    let (start, end, placeholder, contents) = match ctx.heap.get(id) {
                        Value::Int(value) => {
                            let text = crate::integer_text::try_decimal_coefficient(value)
                                .ok_or_else(|| Error::new("MemoryError", ""))?;
                            append_rendered_text(&mut output, &text)?;
                            continue;
                        }
                        Value::List(_) => ("[", "]", "[...]", Part::Sequence(id, 0)),
                        Value::Tuple(items) => (
                            "(",
                            if items.len() == 1 { ",)" } else { ")" },
                            "(...)",
                            Part::Sequence(id, 0),
                        ),
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
                        Value::Dict(_) => ("{", "}", "{...}", Part::Entries(id, 0, true)),
                        Value::DefaultDict { .. } => {
                            push_frame(&mut work, Part::DefaultDictFactory(id, output.len()))?;
                            ("{", "}", "{...}", Part::Entries(id, 0, true))
                        }
                        Value::Set { entries, frozen } => {
                            if entries.is_empty() {
                                append_rendered_text(
                                    &mut output,
                                    if *frozen { "frozenset()" } else { "set()" },
                                )?;
                                continue;
                            }
                            (
                                if *frozen { "frozenset({" } else { "{" },
                                if *frozen { "})" } else { "}" },
                                if *frozen {
                                    "frozenset(...)"
                                } else {
                                    "set(...)"
                                },
                                Part::Entries(id, 0, false),
                            )
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
                    if matches!(ctx.heap.get(id), Value::List(_) | Value::Tuple(_))
                        && sequence_budget.is_some_and(|limit| active.len() >= limit)
                    {
                        return Err(Error::new(
                            "RecursionError",
                            "maximum recursion depth exceeded while getting the repr of an object",
                        ));
                    }
                    push_frame(&mut active, id)?;
                    append_rendered_text(&mut output, start)?;
                    push_frame(&mut work, Part::Leave(id, end))?;
                    match contents {
                        Part::Entries(_, _, false) if checked => {
                            // CPython renders a snapshot so a hook can mutate the original set.
                            let Value::Set { entries, .. } = ctx.heap.get(id) else {
                                unreachable!()
                            };
                            push_frame(&mut work, Part::set_members(entries)?)?;
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
            |_, _, _, _, _| Ok(None),
        )
        .unwrap_or_else(|_| "<exception str() failed>".to_owned())
    }

    /// Render a class using its Python module and qualified name.
    fn class_repr(&self, name: &str, scope: Id) -> String {
        let scope = self.scope_data(scope);
        let attr = |key: &str| {
            scope
                .names
                .get(key)
                .and_then(|&id| match self.heap.get(id) {
                    Value::Str(s) => Some(s.as_str()),
                    _ => None,
                })
        };
        let module = attr("__module__").unwrap_or(&scope.module);
        let name = attr("__qualname__").unwrap_or(name);
        if module == "builtins" {
            format!("<class '{name}'>")
        } else {
            format!("<class '{module}.{name}'>")
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
            Value::Repeat(repeat) => {
                let value = self.format_value(repeat.value, true, active);
                match repeat.remaining {
                    Some(n) => format!("repeat({value}, {n})"),
                    None => format!("repeat({value})"),
                }
            }
            Value::Count { current, step } => {
                let current = self.repr(*current);
                if matches!(self.heap.get(*step), Value::Bool(true))
                    || matches!(self.heap.get(*step), Value::Int(n) if n.as_small() == Some(1))
                {
                    format!("count({current})")
                } else {
                    format!("count({current}, {})", self.repr(*step))
                }
            }
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
            Value::Str(s) if repr => crate::objects::exceptions::string_repr(s).into_string(),
            Value::Str(s) => s.clone(),
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
                |_, _, _, _, _| Ok(None),
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
            Value::WeakrefDescriptor(owner) => format!(
                "<attribute '__weakref__' of '{}' objects>",
                self.weakref_descriptor_owner(*owner)
            ),
            Value::Class { name, scope, .. } => self.class_repr(name, *scope),
            Value::Native(name) => format!("<class '{name}'>"),
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
            Value::Function { fields, .. } => {
                format!(
                    "<function {}>",
                    self.format_value(fields.qualname, false, active)
                )
            }
            Value::AttrGetter(paths) => {
                let arguments = paths
                    .iter()
                    .map(|path| {
                        let mut quoted = String::new();
                        crate::objects::exceptions::write_string_repr(&path.join("."), &mut quoted)
                            .expect("writing into a string cannot fail");
                        quoted
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("operator.attrgetter({arguments})")
            }
            Value::ItemGetter(keys) => format!(
                "operator.itemgetter({})",
                self.format_elements(self.itemgetter_keys(*keys), active)
            ),
            _ => "<object>".to_owned(),
        };
        if recursive.is_some() {
            active.pop();
        }
        result
    }
    pub(super) fn get_item(&mut self, obj: Id, key: Id) -> Result<Id> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::Deque(_)) {
            return deque::IndexOp::Get.apply(self, obj, key, false);
        }
        if let Some(value) = super::union::subscribe(self, obj, key) {
            return value;
        }
        if let Some(value) = super::typing::subscribe(self, obj, key) {
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
        if let Value::MappingProxy(dict) = *self.heap.get(obj) {
            return self.get_item(dict, key);
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
        let storage = self.scalar_value(obj);
        if let Value::Slice(parts) = *self.heap.get(key) {
            return self.slice(obj, parts);
        }
        let kind = match self.heap.get(storage) {
            Value::List(_) => "list",
            Value::Tuple(_) | Value::NamedTuple { .. } => "tuple",
            Value::Instance { class, .. } if self.namedtuple_fields(*class).is_some() => "tuple",
            Value::Str(_) => "string",
            Value::Bytes(_) => "byte",
            Value::ByteArray(_) => "bytearray",
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
            Value::Instance { .. } => {
                let v = self.sequence(storage).unwrap();
                v[position(v.len())?]
            }
            Value::Str(s) => {
                let c = s.chars().nth(position(s.chars().count())?).unwrap();
                self.text(c.to_string())
            }
            Value::Bytes(v) | Value::ByteArray(v) => self.int(i64::from(v[position(v.len())?])),
            _ => unreachable!(),
        })
    }
    /// Call the current factory, preserving any value it inserts for this key.
    /// Look up a builtin mapping key before dispatching the original object's missing hook.
    pub(super) fn dictionary_item(&mut self, object: Id, key: Id) -> Result<Id> {
        let storage = self.dict_storage(object).expect("dictionary");
        let (_, index) = self.find_dictionary_key(storage, key, None)?;
        if let Some(index) = index {
            return Ok(self.dict_entries(storage).expect("dictionary")[index].1);
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

    pub(super) fn put_item(&mut self, obj: Id, key: Id, val: Id) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
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
        } else if let Value::ByteArray(data) = self.heap.get(obj) {
            if let Value::Slice(parts) = *self.heap.get(key) {
                return self.set_slice(obj, parts, Some(val));
            }
            let i = self.as_integer(key)?;
            let i = if i < 0 { data.len() as i64 + i } else { i };
            let byte = u8::try_from(self.as_integer(val)?)
                .map_err(|_| Error::new("ValueError", "byte must be in range(0, 256)"))?;
            let Value::ByteArray(data) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            *data
                .get_mut(i as usize)
                .ok_or_else(|| Error::new("IndexError", "bytearray index out of range"))? = byte;
        } else if let Value::List(items) = self.heap.get(obj) {
            let i = self.as_integer(key)?;
            let i = if i < 0 { items.len() as i64 + i } else { i };
            let Value::List(items) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            let entry = items
                .get_mut(i as usize)
                .ok_or_else(|| Error::new("IndexError", "list assignment index out of range"))?;
            let previous = std::mem::replace(entry, val);
            if previous != val {
                self.heap.weak_edge_removed(previous);
            }
        } else if let Some(setter) = self.bound_slot(obj, "__setitem__")? {
            self.call(setter, vec![key, val], Vec::new())?;
        } else if let Some((storage, _)) = self.sequence_payload(obj)
            && matches!(self.heap.get(storage), Value::List(_))
        {
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
        if let Some(hook) = self.bound_slot(container, "__contains__")? {
            let result = self.call(hook, vec![item], Vec::new())?;
            return self.truth(result);
        }
        if let Value::Context(bindings) = self.heap.get(container) {
            self.context_key(item)?;
            return Ok(bindings.contains_key(&item));
        }
        let scalar = self.scalar_value(container);
        if let Value::Str(hay) = self.heap.get(scalar) {
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
        while let Some(value) = self.next_item(iterator)? {
            if value == item {
                return Ok(true);
            }
            // Equality can resize the source and collect a removed candidate.
            self.heap.keep(value);
            let equal = self.binary("==", value, item)?;
            if self.truth(equal)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn slice(&mut self, obj: Id, parts: [Id; 3]) -> Result<Id> {
        let obj = self.scalar_value(obj);
        if let Value::Range(range) = self.heap.get(obj) {
            return super::slice::range(self, parts, range.clone());
        }
        if !matches!(
            self.heap.get(obj),
            Value::Str(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
                | Value::List(_)
                | Value::Tuple(_)
                | Value::NamedTuple { .. }
        ) && !matches!(self.heap.get(obj), Value::Instance { class, .. } if self.namedtuple_fields(*class).is_some())
        {
            return Err(Error::new(
                "TypeError",
                format!("'{}' object is not subscriptable", self.type_name(obj)),
            ));
        }
        let bounds = super::slice::sequence_components(self, parts)?;
        let vals = self.items(obj)?;
        let [start, end, step] = super::slice::normalize(bounds, vals.len() as i64);
        let out: Vec<_> = slice_positions(start, end, step).map(|i| vals[i]).collect();
        match self.heap.get(obj) {
            Value::Str(_) => {
                let s = out
                    .iter()
                    .map(|&i| self.str_value(i))
                    .collect::<Result<Vec<_>>>()?
                    .concat();
                Ok(self.text(s))
            }
            Value::Bytes(_) | Value::ByteArray(_) => {
                let bytes = out
                    .iter()
                    .map(|&i| self.as_integer(i).map(|x| x as u8))
                    .collect::<Result<_>>()?;
                Ok(
                    self.alloc(if matches!(self.heap.get(obj), Value::Bytes(_)) {
                        Value::Bytes(bytes)
                    } else {
                        Value::ByteArray(bytes)
                    }),
                )
            }
            Value::List(_) => Ok(self.alloc(Value::List(out))),
            _ => Ok(self.alloc(Value::Tuple(out))),
        }
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

    /// Returns the immutable sequence payload used by builtin repetition.
    fn repeat_storage(&self, id: Id) -> Option<Id> {
        if matches!(self.heap.get(id), Value::Bytes(_)) {
            Some(id)
        } else {
            self.string_storage(id)
        }
    }

    /// Uses the index protocol for either operand order.
    fn repeat_immutable(&mut self, sequence: Id, multiplier: Id) -> Result<Id> {
        let count = self.index_value(multiplier)?.ok_or_else(|| {
            Error::new(
                "TypeError",
                format!(
                    "can't multiply sequence by non-int of type '{}'",
                    self.type_name(multiplier)
                ),
            )
        })?;
        Ok(match self.heap.get(sequence) {
            Value::Bytes(data) => match crate::objects::repeat_bytes(data, &count)? {
                Some(repeated) => self.alloc(Value::Bytes(repeated)),
                None => sequence,
            },
            Value::Str(_) if count.as_small() == Some(1) => sequence,
            Value::Str(text) => self.text(crate::objects::repeat_str(text, &count)?.into_string()),
            _ => unreachable!("immutable repetition payload"),
        })
    }

    /// Union of set operands preserves mutable left-hand storage for augmented assignment.
    fn set_union(&mut self, a: Id, b: Id, inplace: bool) -> Result<Id> {
        let Value::Set { entries, frozen } = self.heap.get(a) else {
            unreachable!()
        };
        let result = if inplace && !frozen {
            a
        } else {
            self.alloc(Value::Set {
                entries: entries.clone(),
                frozen: *frozen,
            })
        };
        if a != b {
            builtins::update_set(self, result, b)?;
        }
        Ok(result)
    }

    /// Visit an operand with cached hashes when its keys already belong to a set.
    fn visit_set_operand(
        &mut self,
        source: Id,
        mut visit: impl FnMut(&mut Self, Id, Option<i64>) -> Result<()>,
    ) -> Result<()> {
        let iterator = if matches!(self.heap.get(source), Value::Set { .. }) {
            None
        } else {
            let iterator = self.iterator(source)?;
            self.heap.keep(iterator);
            Some(iterator)
        };
        let mark = self.heap.temporary_mark();
        let mut position = 0;
        loop {
            let (key, hash) = if let Some(iterator) = iterator {
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
            self.visit_set_operand(right, |ctx, key, hash| {
                if let Some(index) = ctx.find_dictionary_key(result, key, hash)?.1 {
                    let Value::Set { entries, .. } = ctx.heap.get_mut(result) else {
                        unreachable!()
                    };
                    let (removed, _) = entries.remove(index);
                    ctx.heap.weak_edge_removed(removed);
                }
                Ok(())
            })?;
            return Ok(result);
        }
        let (container, source) = if sets {
            if self.stored_entries(left).len() < self.stored_entries(right).len() {
                (right, left)
            } else {
                (left, right)
            }
        } else {
            let view_len = |id| match self.heap.get(id) {
                Value::DictView {
                    dict,
                    kind: value::DictViewKind::Keys | value::DictViewKind::Items,
                    ..
                } => Some(
                    self.dict_entries(*dict)
                        .expect("dictionary view backing")
                        .len(),
                ),
                _ => None,
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
        self.visit_set_operand(source, |ctx, key, hash| {
            let present = match ctx.heap.get(container) {
                Value::Set { .. } => ctx.find_dictionary_key(container, key, hash)?.1.is_some(),
                Value::DictView {
                    dict,
                    kind: value::DictViewKind::Keys,
                    ..
                } => {
                    let storage = ctx.dict_storage(*dict).expect("dictionary view backing");
                    ctx.find_dictionary_key(storage, key, hash)?.1.is_some()
                }
                Value::DictView {
                    dict,
                    kind: value::DictViewKind::Items,
                    ..
                } => {
                    let storage = ctx.dict_storage(*dict).expect("dictionary view backing");
                    let Value::Tuple(parts) = ctx.heap.get(key) else {
                        return Ok(());
                    };
                    let &[lookup, expected] = parts.as_slice() else {
                        return Ok(());
                    };
                    let Some(index) = ctx.find_dictionary_key(storage, lookup, None)?.1 else {
                        return Ok(());
                    };
                    let actual = ctx.stored_entries(storage)[index].1;
                    actual == expected || ctx.checked_equal(actual, expected)?
                }
                _ => unreachable!("intersection with a set or dictionary view"),
            };
            if present {
                ctx.set_dictionary_key(result, key, NONE, hash, false)?;
            }
            Ok(())
        })?;
        if mutate {
            let Value::Set { entries, .. } = self.heap.get_mut(result) else {
                unreachable!()
            };
            let selected = std::mem::take(entries);
            let Value::Set { entries, .. } = self.heap.get_mut(left) else {
                unreachable!()
            };
            for (key, _) in std::mem::replace(entries, selected) {
                self.heap.weak_edge_removed(key);
            }
            return Ok(left);
        }
        Ok(result)
    }

    pub(super) fn binary(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        self.binary_operation(op, a, b, false)
    }

    fn binary_operation(&mut self, op: &str, a: Id, b: Id, inplace: bool) -> Result<Id> {
        let (a, b) = if !self.heap.has_weak_values || matches!(op, "is" | "is not") {
            (a, b)
        } else {
            (self.weak_proxy_target(a)?, self.weak_proxy_target(b)?)
        };
        if op == "|" && matches!(self.heap.get(a), Value::Union(_) | Value::TypeVar(_)) {
            return super::union::make(self, &[a, b]);
        }
        if let Some(result) = self.binary_special(op, a, b)? {
            return Ok(result);
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
                // Dictionary keys and items implement reflected union, returning a fresh set.
                let result = self.alloc(Value::Set {
                    entries: Default::default(),
                    frozen: false,
                });
                builtins::update_set(self, result, a)?;
                builtins::update_set(self, result, b)?;
                return Ok(result);
            }
            "|" | "&" | "-"
                if matches!(self.heap.get(a), Value::Set { .. })
                    || matches!(self.heap.get(b), Value::Set { .. }) =>
            {
                if matches!(
                    (self.heap.get(a), self.heap.get(b)),
                    (Value::Set { .. }, Value::Set { .. })
                ) {
                    return if op == "|" {
                        self.set_union(a, b, inplace)
                    } else {
                        self.set_algebra(op, a, b, inplace)
                    };
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
            "is" => return Ok(truth(a == b)),
            "is not" => return Ok(truth(a != b)),
            "in" => return Ok(truth(self.contains(a, b)?)),
            "not in" => return Ok(truth(!self.contains(a, b)?)),
            "<" | "<=" | ">" | ">=" => {
                let Some(c) = self.compare(op, a, b)? else {
                    return Ok(FALSE);
                };
                return Ok(truth(match op {
                    "<" => c.is_lt(),
                    "<=" => c.is_le(),
                    ">" => c.is_gt(),
                    _ => c.is_ge(),
                }));
            }
            "+" | "-"
                if matches!(self.heap.get(a), Value::Complex(_))
                    || matches!(self.heap.get(b), Value::Complex(_)) =>
            {
                return self.complex_sum(op == "+", a, b);
            }
            "+" => match (self.heap.get(a), self.heap.get(b)) {
                (Value::Str(x), Value::Str(y)) => return Ok(self.text(format!("{x}{y}"))),
                (Value::Bytes(x), Value::Bytes(y) | Value::ByteArray(y)) => {
                    if y.is_empty() {
                        return Ok(a);
                    }
                    if x.is_empty() && matches!(self.heap.get(b), Value::Bytes(_)) {
                        return Ok(b);
                    }
                    let bytes = crate::objects::concat_bytes(x, y)?;
                    return Ok(self.alloc(Value::Bytes(bytes)));
                }
                (Value::Bytes(_), _) => {
                    return Err(Error::new(
                        "TypeError",
                        format!("can't concat {} to bytes", self.type_name(b)),
                    ));
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
                (Value::List(x), Value::List(y)) => {
                    return Ok(self.alloc(Value::List([x.as_slice(), y.as_slice()].concat())));
                }
                (Value::Tuple(x), Value::Tuple(y)) => {
                    return Ok(self.alloc(Value::Tuple([x.as_slice(), y.as_slice()].concat())));
                }
                _ => {}
            },
            "*" if let Some(sequence) = self.repeat_storage(a) => {
                return self.repeat_immutable(sequence, b);
            }
            "*" if let Some(sequence) = self.repeat_storage(b) => {
                return self.repeat_immutable(sequence, a);
            }
            _ => {}
        }
        if op == "*" {
            let (sequence, count) = if matches!(self.heap.get(a), Value::List(_) | Value::Tuple(_))
            {
                (a, b)
            } else {
                (b, a)
            };
            if let Value::List(items) | Value::Tuple(items) = self.heap.get(sequence) {
                let out = repeat_values(items, self.as_integer(count)?)?;
                return Ok(
                    self.alloc(if matches!(self.heap.get(sequence), Value::List(_)) {
                        Value::List(out)
                    } else {
                        Value::Tuple(out)
                    }),
                );
            }
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
        if matches!(self.heap.get(left), Value::Float(_))
            || matches!(self.heap.get(right), Value::Float(_))
        {
            let number = |value| -> Result<Option<f64>> {
                match self.heap.get(value) {
                    Value::Float(number) => Ok(Some(*number)),
                    Value::Int(_) | Value::Bool(_) => {
                        Ok(Some(crate::floats::int_to_float(&self.as_int(value)?)?))
                    }
                    _ => Ok(None),
                }
            };
            if let (Some(x), Some(y)) = (number(left)?, number(right)?) {
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
                                let magnitude = (-x).powf(y);
                                if magnitude.is_infinite() {
                                    return Err(Error::new(
                                        "OverflowError",
                                        "complex exponentiation",
                                    ));
                                }
                                let phase = std::f64::consts::PI * y;
                                return Ok(self.alloc(Value::Complex(super::complex::Complex {
                                    real: magnitude * phase.cos(),
                                    imag: magnitude * phase.sin(),
                                })));
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
        }
        if op == "**"
            && [left, right]
                .into_iter()
                .any(|id| !matches!(self.heap.get(id), Value::Int(_) | Value::Bool(_)))
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "unsupported operand type(s) for ** or pow(): '{}' and '{}'",
                    self.type_name(a),
                    self.type_name(b)
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
            let code = match op {
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
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("unsupported operator {op}"),
                    ));
                }
            };
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
    fn operator_slot(&mut self, obj: Id, name: &str, other: Id) -> Result<Option<Id>> {
        let Some(method) = self.bound_slot(obj, name)? else {
            if name == "__ne__"
                && let Some(equal) = self.operator_slot(obj, "__eq__", other)?
            {
                return Ok(Some(if self.truth(equal)? { FALSE } else { TRUE }));
            }
            return Ok(None);
        };
        let result = self.call(method, vec![other], vec![])?;
        Ok((result != value::NOT_IMPLEMENTED).then_some(result))
    }

    pub(super) fn binary_special(&mut self, op: &str, a: Id, b: Id) -> Result<Option<Id>> {
        if matches!(op, "==" | "!=")
            && matches!(self.heap.get(a), Value::Deque(_))
            && matches!(self.heap.get(b), Value::Deque(_))
        {
            return deque::compare(self, op, a, b).map(|result| Some(builtins::boolean(result)));
        }
        let (a, b) = if matches!(op, "==" | "!=") {
            let left = self.mapping_comparison_target(a);
            let right = self.mapping_comparison_target(b);
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
            if op == "!="
                && self.slot(left, "__ne__").is_none()
                && self.slot(right, "__ne__").is_none()
            {
                let equal = self.binary("==", left, right)?;
                return Ok(Some(if self.truth(equal)? { FALSE } else { TRUE }));
            }
            return self.binary(op, left, right).map(Some);
        }
        let Some((left, right, inplace)) = binary_slots(op) else {
            return Ok(None);
        };
        let class = |id| match self.heap.get(id) {
            Value::Instance { class, .. } => Some(*class),
            _ => None,
        };
        let (ac, bc) = (class(a), class(b));
        if ac.is_none() && bc.is_none() {
            return Ok(None);
        }
        if matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
            && let Some(&decimal) = self.classes.get("decimal.Decimal")
            && (ac == Some(decimal) || bc == Some(decimal))
        {
            return Err(Error::new(
                "NotImplementedError",
                "native Decimal numeric comparisons are unsupported",
            ));
        }
        let reflected_first = if let (Some(ac), Some(bc)) = (ac, bc) {
            ac != bc
                && self.subclass(bc, ac)
                && (inplace.is_empty() || self.class_attr(bc, right) != self.class_attr(ac, right))
        } else {
            false
        };
        if reflected_first && let Some(result) = self.operator_slot(b, right, a)? {
            return Ok(Some(result));
        }
        if let Some(result) = self.operator_slot(a, left, b)? {
            return Ok(Some(result));
        }
        if !reflected_first && (ac != bc || inplace.is_empty()) {
            return self.operator_slot(b, right, a);
        }
        Ok(None)
    }

    pub(super) fn inplace(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        let target = self.weak_proxy_target(a)?;
        if target != a {
            return self.inplace(op, target, self.weak_proxy_target(b)?);
        }
        if let Some((_, _, slot)) = binary_slots(op)
            && let Some(result) = self.operator_slot(a, slot, b)?
        {
            return Ok(result);
        }
        if op == "+"
            && let Value::ByteArray(left) = self.heap.get(a)
            && let Value::Bytes(right) | Value::ByteArray(right) = self.heap.get(b)
        {
            if right.is_empty() {
                return Ok(a);
            }
            // Reading the right-hand buffer exports it during the resize.
            if a == b {
                return Err(Self::exported_bytearray_error());
            }
            self.check_bytearray_resize(a)?;
            let bytes = crate::objects::concat_bytes(left, right)?;
            let Value::ByteArray(left) = self.heap.get_mut(a) else {
                unreachable!()
            };
            *left = bytes;
            return Ok(a);
        }
        if op == "+" && matches!(self.heap.get(a), Value::List(_)) {
            builtins::call(self, "list.extend", &[a, b], &[])?;
            return Ok(a);
        }
        if op == "*"
            && let Value::List(values) = self.heap.get(a)
        {
            let values = repeat_values(values, self.as_integer(b)?)?;
            let Value::List(items) = self.heap.get_mut(a) else {
                unreachable!()
            };
            if values.is_empty() && self.sorting_lists.contains(&a) {
                // The allocated callback buffer records a mutation until the sort restores it.
                items.clear();
            } else {
                *items = values;
            }
            return Ok(a);
        }
        self.binary_operation(op, a, b, true)
    }

    pub(super) fn delete_item(&mut self, obj: Id, key: Id) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::Deque(_)) {
            return deque::IndexOp::Delete
                .apply(self, obj, key, false)
                .map(|_| ());
        }
        if let Some(method) = self.bound_slot(obj, "__delitem__")? {
            self.call(method, vec![key], vec![])?;
            return Ok(());
        }
        if let Some((storage, _)) = self.sequence_payload(obj)
            && matches!(self.heap.get(storage), Value::List(_))
        {
            return self.delete_item(storage, key);
        }
        if let Some(dict) = self.dict_storage(obj) {
            let removed = builtins::remove_dict_item(self, dict, key)?
                .ok_or_else(|| self.lookup_error("KeyError", key))?;
            if self.heap.has_file_writers()
                && self.heap.could_own_writer(removed, true)
                && self.heap.has_unbound_writer(NONE)
            {
                self.collect_unreachable();
            }
            return Ok(());
        }
        match self.heap.get(obj) {
            Value::ByteArray(_) => {
                if let Value::Slice(parts) = *self.heap.get(key) {
                    return self.set_slice(obj, parts, None);
                }
                let index = self.index_value(key)?.ok_or_else(|| {
                    Error::new(
                        "TypeError",
                        format!(
                            "bytearray indices must be integers or slices, not {}",
                            self.type_name(key)
                        ),
                    )
                })?;
                let i = index.to_i64().ok_or_else(|| {
                    Error::new(
                        "IndexError",
                        format!(
                            "cannot fit '{}' into an index-sized integer",
                            self.type_name(key)
                        ),
                    )
                })?;
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
            Value::List(entries) => {
                let i = self.as_integer(key)?;
                let i = if i < 0 { entries.len() as i64 + i } else { i };
                if !(0..entries.len() as i64).contains(&i) {
                    return Err(Error::new(
                        "IndexError",
                        "list assignment index out of range",
                    ));
                }
                let Value::List(entries) = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                let value = entries.remove(i as usize);
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

fn repeat_values(values: &[Id], times: i64) -> Result<Vec<Id>> {
    if values.is_empty() || times <= 0 {
        return Ok(Vec::new());
    }
    let size = usize::try_from(times)
        .ok()
        .and_then(|n| values.len().checked_mul(n))
        .ok_or_else(|| Error::new("MemoryError", ""))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", ""))?;
    for _ in 0..times {
        output.extend_from_slice(values);
    }
    Ok(output)
}
