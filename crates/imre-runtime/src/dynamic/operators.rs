use std::cmp::Ordering;

use num_traits::ToPrimitive;

use super::*;

fn slice_positions(start: i64, end: i64, step: i64) -> impl Iterator<Item = usize> + Clone {
    std::iter::successors(Some(start), move |&i| i.checked_add(step))
        .take_while(move |&i| if step > 0 { i < end } else { i > end })
        .map(|i| i as usize)
}

impl Context {
    fn sequence(&self, id: Id) -> Option<Vec<Id>> {
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
                .string_storage(id)
                .map(|text| self.str_value(text).unwrap().chars().count()));
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
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) | Value::ByteArray(b) => !b.is_empty(),
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => !v.is_empty(),
            Value::Set { entries: v, .. }
            | Value::Dict(v)
            | Value::DefaultDict { entries: v, .. } => !v.is_empty(),
            Value::DictView { dict, .. } => !self
                .dict_entries(*dict)
                .expect("dict view backing")
                .is_empty(),
            Value::Context(bindings)
            | Value::Iterator {
                context: Some(bindings),
                ..
            } => !bindings.is_empty(),
            Value::Range { start, stop, step } => {
                if *step > 0 {
                    start < stop
                } else {
                    start > stop
                }
            }
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
    fn method_equality_operands(&self, mut a: Id, mut b: Id) -> Option<(Id, Id, bool)> {
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
        while pair.is_some() || !frames.is_empty() {
            if let Some((a, b)) = pair.take()
                && a != b
            {
                if check_special
                    && (matches!(self.heap.get(a), Value::Instance { .. })
                        || matches!(self.heap.get(b), Value::Instance { .. }))
                {
                    self.heap.keep(a);
                    self.heap.keep(b);
                }
                if check_special && let Some(result) = self.binary_special("==", a, b)? {
                    if !self.truth(result)? {
                        return Ok(false);
                    }
                    continue;
                }
                if matches!(self.heap.get(a), Value::Method { .. })
                    && matches!(self.heap.get(b), Value::Method { .. })
                {
                    let Some((left, right, same_receivers)) = self.method_equality_operands(a, b)
                    else {
                        return Ok(false);
                    };
                    if left != right {
                        self.heap.keep(a);
                        self.heap.keep(b);
                        if !self.checked_equal(left, right)? {
                            return Ok(false);
                        }
                    }
                    if !same_receivers {
                        return Ok(false);
                    }
                    continue;
                }
                let a = self.dict_storage(a).unwrap_or(a);
                let b = self.dict_storage(b).unwrap_or(b);
                if let (Value::GenericAlias(left), Value::GenericAlias(right)) =
                    (self.heap.get(a), self.heap.get(b))
                {
                    if left.kind != right.kind || left.origin != right.origin {
                        return Ok(false);
                    }
                    pair = Some((left.args, right.args));
                    check_special = true;
                    continue;
                }
                let children = match (self.heap.get(a), self.heap.get(b)) {
                    (Value::List(_), right) if !matches!(right, Value::List(_)) => {
                        return Ok(false);
                    }
                    (left, Value::List(_)) if !matches!(left, Value::List(_)) => return Ok(false),
                    (
                        Value::Dict(x) | Value::DefaultDict { entries: x, .. },
                        Value::Dict(y) | Value::DefaultDict { entries: y, .. },
                    ) => {
                        if x.len() != y.len() {
                            return Ok(false);
                        }
                        Some(Children::Dict {
                            position: 0,
                            roots: None,
                        })
                    }
                    (Value::Context(x), Value::Context(y)) => {
                        if x.len() != y.len() {
                            return Ok(false);
                        }
                        (!std::rc::Rc::ptr_eq(x, y)).then_some(Children::Context(0))
                    }
                    _ => match (self.sequence_at(a, 0), self.sequence_at(b, 0)) {
                        (Some((x, _)), Some((y, _))) => {
                            if x != y {
                                return Ok(false);
                            }
                            Some(Children::Sequence(0))
                        }
                        _ if self.equal(a, b) => None,
                        _ => return Ok(false),
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
            }
            check_special = true;
            while let Some((parents, children)) = frames.last_mut() {
                let (a, b) = *parents;
                pair = match children {
                    Children::Sequence(index) => {
                        let (xlen, x) = self.sequence_at(a, *index).expect("sequence parent");
                        let (ylen, y) = self.sequence_at(b, *index).expect("sequence parent");
                        if xlen != ylen {
                            return Ok(false);
                        }
                        if let (Some(x), Some(y)) = (x, y) {
                            *index += 1;
                            Some((x, y))
                        } else {
                            None
                        }
                    }
                    Children::Dict { position, roots } => {
                        // Retain one entry across reentrant key and value comparisons.
                        let roots = *roots.get_or_insert_with(|| self.heap.temporary_mark());
                        self.heap.release_temporaries_since(roots);
                        if let Some((at, key, left, hash)) = self.dictionary_pair(a, *position) {
                            *position = at + 1;
                            self.heap.keep(left);
                            let Some(index) = self.find_dictionary_key(b, key, hash)?.1 else {
                                return Ok(false);
                            };
                            let right = self.dict_entries(b).expect("dict parent")[index].1;
                            self.heap.keep(right);
                            Some((left, right))
                        } else {
                            None
                        }
                    }
                    Children::Context(index) => {
                        let (Value::Context(x), Value::Context(y)) =
                            (self.heap.get(a), self.heap.get(b))
                        else {
                            unreachable!()
                        };
                        if x.len() != y.len() {
                            return Ok(false);
                        }
                        if let Some((&key, &value)) = x.iter().nth(*index) {
                            *index += 1;
                            let Some(&right) = y.get(&key) else {
                                return Ok(false);
                            };
                            Some((value, right))
                        } else {
                            None
                        }
                    }
                };
                if pair.is_some() {
                    break;
                }
                active.remove(parents);
                frames.pop();
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
                a.kind == b.kind && a.origin == b.origin && equal(a.args, b.args)
            }
            (Value::Slice(x), Value::Slice(y)) => x.iter().zip(y).all(|(&i, &j)| equal(i, j)),
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
            (Value::Set { entries: x, .. }, Value::Set { entries: y, .. }) => {
                x.len() == y.len() && x.iter().all(|&(k, _)| y.iter().any(|&(j, _)| equal(k, j)))
            }
            _ => match (self.sequence(a), self.sequence(b)) {
                (Some(x), Some(y)) => {
                    x.len() == y.len() && x.iter().zip(y).all(|(&i, j)| equal(i, j))
                }
                _ => false,
            },
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
        self.render_containers(id, false, false)
            .expect("unlimited rendering")
    }

    pub(super) fn repr(&self, id: Id) -> String {
        self.render_containers(id, true, false)
            .expect("unlimited rendering")
    }

    /// Render container ancestors iteratively so recursive values do not consume the native stack.
    pub(super) fn checked_format(&self, root: Id, quoted: bool) -> Result<String> {
        self.render_containers(root, quoted, true)
    }

    fn render_containers(&self, root: Id, quoted: bool, checked: bool) -> Result<String> {
        enum Part<'a> {
            Value(Id, bool),
            Text(&'a str),
            Sequence(&'a [Id], usize),
            Entries(&'a [(Id, Id)], usize, bool),
            Fields(
                &'a [String],
                &'a std::collections::HashMap<String, Id>,
                usize,
            ),
            Leave(Id, &'static str),
            ContextEnd(Id),
            SliceEnd,
            NamedTupleEnd,
        }
        let mut output = String::new();
        let mut work = vec![Part::Value(root, quoted)];
        let mut active = Vec::new();
        let mut ancestors = std::collections::HashSet::new();
        // CPython guards nested slice formatting against the thread's native stack.
        // The pinned x86-64 Linux reference build uses 624 bytes per slice
        // and keeps two native-stack margins before raising RecursionError.
        const SLICE_FRAME_WORDS: usize = 78;
        const STACK_MARGIN_WORDS: usize = 2048;
        let slice_budget = if checked {
            stacker::remaining_stack().map(|bytes| {
                (bytes / size_of::<usize>()).saturating_sub(2 * STACK_MARGIN_WORDS)
                    / SLICE_FRAME_WORDS
            })
        } else {
            None
        };
        let mut slice_depth = 0usize;
        let mut namedtuple_depth = 0usize;
        while let Some(part) = work.pop() {
            match part {
                Part::Text(text) => output.push_str(text),
                Part::ContextEnd(id) => output.push_str(&format!(" at {id:#x}>")),
                Part::NamedTupleEnd => {
                    namedtuple_depth -= 1;
                    output.push(')');
                }
                Part::SliceEnd => {
                    slice_depth -= 1;
                    output.push(')');
                }
                Part::Leave(id, end) => {
                    ancestors.remove(&id);
                    let last = active.pop();
                    debug_assert_eq!(last, Some(id));
                    output.push_str(end);
                }
                Part::Sequence(items, index) => {
                    if let Some(&item) = items.get(index) {
                        if index != 0 {
                            output.push_str(", ");
                        }
                        work.push(Part::Sequence(items, index + 1));
                        work.push(Part::Value(item, true));
                    }
                }
                Part::Fields(names, fields, index) => {
                    if let Some(name) = names.get(index) {
                        if index != 0 {
                            output.push_str(", ");
                        }
                        output.push_str(name);
                        output.push('=');
                        work.push(Part::Fields(names, fields, index + 1));
                        work.push(Part::Value(fields[name], true));
                    }
                }
                Part::Entries(items, index, include_values) => {
                    if let Some(&(key, value)) = items.get(index) {
                        if index != 0 {
                            output.push_str(", ");
                        }
                        work.push(Part::Entries(items, index + 1, include_values));
                        if include_values {
                            work.push(Part::Value(value, true));
                            work.push(Part::Text(": "));
                        }
                        work.push(Part::Value(key, true));
                    }
                }
                Part::Value(id, repr) => {
                    match self.heap.get(id) {
                        Value::Slice(items) => {
                            if slice_budget.is_some_and(|limit| slice_depth >= limit) {
                                return Err(Error::new(
                                    "RecursionError",
                                    "maximum recursion depth exceeded while getting the repr of an object",
                                ));
                            }
                            slice_depth += 1;
                            output.push_str("slice(");
                            work.push(Part::SliceEnd);
                            work.push(Part::Sequence(items, 0));
                            continue;
                        }
                        Value::Instance {
                            scalar: None,
                            dictionary: Some(backing),
                            ..
                        } if self.enum_member(id).is_none() => {
                            work.push(Part::Value(*backing, repr));
                            continue;
                        }
                        Value::Instance { class, fields, .. }
                            if let Value::Class {
                                name,
                                namedtuple_fields: Some(names),
                                ..
                            } = self.heap.get(*class)
                                && names.iter().all(|field| fields.contains_key(field)) =>
                        {
                            if checked {
                                self.check_call_depth(namedtuple_depth)?;
                            }
                            namedtuple_depth += 1;
                            output.push_str(name);
                            output.push('(');
                            work.push(Part::NamedTupleEnd);
                            work.push(Part::Fields(names, fields, 0));
                            continue;
                        }
                        Value::ContextVar { name, default, .. } => {
                            output.push_str("<ContextVar name=");
                            work.push(Part::ContextEnd(id));
                            if let Some(default) = default {
                                work.push(Part::Value(*default, true));
                                work.push(Part::Text(" default="));
                            }
                            work.push(Part::Value(*name, true));
                            continue;
                        }
                        Value::ContextToken { variable, used, .. } => {
                            output.push_str(if *used {
                                "<Token used var="
                            } else {
                                "<Token var="
                            });
                            work.push(Part::ContextEnd(id));
                            work.push(Part::Value(*variable, true));
                            continue;
                        }
                        _ => {}
                    }
                    let (start, end, placeholder, sequence, entries) = match self.heap.get(id) {
                        Value::List(items) => ("[", "]", "[...]", Some(items.as_slice()), None),
                        Value::Tuple(items) => (
                            "(",
                            if items.len() == 1 { ",)" } else { ")" },
                            "(...)",
                            Some(items.as_slice()),
                            None,
                        ),
                        Value::Dict(items) => ("{", "}", "{...}", None, Some((&items[..], true))),
                        Value::Set { entries, frozen } => {
                            if entries.is_empty() {
                                output.push_str(if *frozen { "frozenset()" } else { "set()" });
                                continue;
                            }
                            (
                                if *frozen { "frozenset({" } else { "{" },
                                if *frozen { "})" } else { "}" },
                                "set(...)",
                                None,
                                Some((&entries[..], false)),
                            )
                        }
                        _ => {
                            output.push_str(&self.format_value(id, repr, &mut active));
                            continue;
                        }
                    };
                    if !ancestors.insert(id) {
                        output.push_str(placeholder);
                        continue;
                    }
                    active.push(id);
                    output.push_str(start);
                    work.push(Part::Leave(id, end));
                    if let Some(items) = sequence {
                        work.push(Part::Sequence(items, 0));
                    }
                    if let Some((items, include_values)) = entries {
                        work.push(Part::Entries(items, 0, include_values));
                    }
                }
            }
        }
        Ok(output)
    }

    /// Renders positional values inside tuples and exception representations.
    fn format_elements(&self, values: &[Id], active: &mut Vec<Id>) -> String {
        values
            .iter()
            .map(|&arg| self.format_value(arg, true, active))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Formats the current arguments, preserving native diagnostic text when present.
    pub(super) fn exception_message(&self, error: &Error, active: &mut Vec<Id>) -> String {
        match error.args.as_deref() {
            None => error.message.clone(),
            Some([]) => String::new(),
            Some([arg]) => {
                self.format_value(*arg, self.is_builtin_error(error, "KeyError"), active)
            }
            Some(args) => format!("({})", self.format_elements(args, active)),
        }
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
                let default =
                    default.map_or_else(String::new, |v| format!(" default={}", self.repr(v)));
                let repr = format!("<ContextVar name={}{default} at {id:#x}>", self.repr(*name));
                repr_active.set(false);
                repr
            }
            Value::ContextToken { variable, used, .. } => {
                let state = if *used { " used" } else { "" };
                format!("<Token{state} var={} at {id:#x}>", self.repr(*variable))
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
            Value::ByteArray(bytes) => {
                let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
                    '"'
                } else {
                    '\''
                };
                let mut output = format!("bytearray(b{quote}");
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
            Value::Raised(error) if repr => {
                let kind = error.kind.rsplit('.').next().unwrap_or(&error.kind);
                let args = if let Some(os) = &error.os_args {
                    format!(
                        "{}, {}",
                        os.errno,
                        crate::objects::exceptions::string_repr(&os.description)
                    )
                } else if let Some(args) = &error.args {
                    self.format_elements(args, active)
                } else if error.message.is_empty() {
                    String::new()
                } else {
                    crate::objects::exceptions::string_repr(&error.message).into_string()
                };
                format!("{kind}({args})")
            }
            Value::Raised(error) => self.exception_message(error, active),
            Value::GenericAlias(alias) => alias.repr(self),
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
            Value::Class { name, .. } | Value::Native(name) => format!("<class '{name}'>"),
            Value::Module { name, .. } => format!("<module '{name}'>"),
            Value::StaticMethod { callable, .. } => {
                format!("<staticmethod({})>", self.repr(*callable))
            }
            Value::Function { fields, .. } => {
                format!("<function {}>", self.display(fields.qualname))
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
            let (_, index) = self.find_dictionary_key(obj, key, None)?;
            if let Some(index) = index {
                return Ok(self.dict_entries(obj).expect("dictionary")[index].1);
            }
            if let Value::DefaultDict { factory, .. } = self.heap.get(obj)
                && *factory != NONE
            {
                let v = self.call(*factory, Vec::new(), Vec::new())?;
                self.put_item(obj, key, v)?;
                return Ok(v);
            }
            return Err(self.lookup_error("KeyError", key));
        }
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
            Value::Range { .. } => "range",
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
        let offset = if kind == "range" {
            index.big().to_i128().ok_or_else(bounds_error)?
        } else {
            i128::from(index.to_i64().ok_or_else(|| {
                Error::new(
                    "IndexError",
                    format!(
                        "cannot fit '{}' into an index-sized integer",
                        self.type_name(key)
                    ),
                )
            })?)
        };
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
            Value::Range { start, stop, step } => {
                let i = position(builtins::range_len(*start, *stop, *step)?)? as i128;
                self.int((i128::from(*start) + i * i128::from(*step)) as i64)
            }
            _ => unreachable!(),
        })
    }
    pub(super) fn put_item(&mut self, obj: Id, key: Id, val: Id) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
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
        if let Value::Str(hay) = self.heap.get(container) {
            return Ok(hay.contains(self.str_value(item)?));
        }
        if matches!(self.heap.get(container), Value::Bytes(_)) {
            // CPython falls back to the buffer protocol when integer conversion fails.
            let number = self.index_value(item).ok().flatten();
            let Value::Bytes(data) = self.heap.get(container) else {
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
        if let Value::Range { start, stop, step } = *self.heap.get(obj) {
            return super::slice::range(self, parts, start, stop, step);
        }
        if !matches!(
            self.heap.get(obj),
            Value::Str(_)
                | Value::Bytes(_)
                | Value::ByteArray(_)
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

    pub(super) fn binary(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        self.binary_operation(op, a, b, false)
    }

    fn binary_operation(&mut self, op: &str, a: Id, b: Id, inplace: bool) -> Result<Id> {
        let (a, b) = if !self.heap.has_weak_values || matches!(op, "is" | "is not") {
            (a, b)
        } else {
            (self.weak_proxy_target(a)?, self.weak_proxy_target(b)?)
        };
        if let Some(result) = self.binary_special(op, a, b)? {
            return Ok(result);
        }
        if let (Value::Slice(left), Value::Slice(right)) = (self.heap.get(a), self.heap.get(b))
            && matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
        {
            return super::slice::compare(self, op, *left, *right);
        }
        let truth = |v| if v { TRUE } else { FALSE };
        match op {
            "|" if [a, b].into_iter().any(|id| {
                matches!(
                    self.heap.get(id),
                    Value::DictView {
                        kind: value::DictViewKind::Keys | value::DictViewKind::Items,
                        ..
                    }
                )
            }) =>
            {
                // Dictionary keys and items implement reflected union, returning a fresh set.
                let result = self.alloc(Value::Set {
                    entries: Default::default(),
                    frozen: false,
                });
                builtins::update_set(self, result, a)?;
                builtins::update_set(self, result, b)?;
                return Ok(result);
            }
            "|" if matches!(self.heap.get(a), Value::Set { .. })
                || matches!(self.heap.get(b), Value::Set { .. }) =>
            {
                if matches!(
                    (self.heap.get(a), self.heap.get(b)),
                    (Value::Set { .. }, Value::Set { .. })
                ) {
                    return self.set_union(a, b, inplace);
                }
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for {}: '{}' and '{}'",
                        if inplace { "|=" } else { "|" },
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
        if let Some(method) = self.bound_slot(obj, "__delitem__")? {
            self.call(method, vec![key], vec![])?;
            return Ok(());
        }
        if let Some(dict) = self.dict_storage(obj) {
            builtins::remove_dict_item(self, dict, key)?
                .ok_or_else(|| self.lookup_error("KeyError", key))?;
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
