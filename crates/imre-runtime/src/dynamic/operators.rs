use std::cmp::Ordering;

use num_traits::{Signed, ToPrimitive, Zero};

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
    pub(super) fn optional_truth(&mut self, value: Option<Id>, default: bool) -> Result<bool> {
        value.map_or(Ok(default), |id| self.truth(id))
    }

    pub(super) fn index_value(&mut self, value: Id) -> Result<Option<crate::Int>> {
        self.index_value_with_bool_result(value, true)
    }

    /// Some native operations reject a boolean returned by a user-defined index hook.
    pub(super) fn index_value_with_bool_result(
        &mut self,
        value: Id,
        allow_bool_result: bool,
    ) -> Result<Option<crate::Int>> {
        if let Ok(integer) = self.as_int(value) {
            return Ok(Some(integer));
        }
        let Some(index) = self.bound_slot(value, "__index__")? else {
            return Ok(None);
        };
        let result = self.call(index, Vec::new(), Vec::new())?;
        if !allow_bool_result && matches!(self.heap.get(result), Value::Bool(_)) {
            return Err(Error::new(
                "TypeError",
                "__index__ returned non-int (type bool)",
            ));
        }
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
            Value::List(v)
            | Value::Tuple(v)
            | Value::Set { items: v, .. }
            | Value::NamedTuple { values: v, .. } => !v.is_empty(),
            Value::Dict(v) | Value::DefaultDict { entries: v, .. } => !v.is_empty(),
            Value::DictView { dict, .. } => !self
                .dict_entries(*dict)
                .expect("dict view backing")
                .is_empty(),
            Value::Context(bindings) => !bindings.is_empty(),
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
    pub(super) fn equal(&self, a: Id, b: Id) -> bool {
        // Hashable dictionary, set and cache keys cannot contain cyclic containers.
        self.equal_inner(a, b, usize::MAX, &mut false)
    }

    /// Compare arbitrary boxed values and surface recursive context comparisons.
    fn checked_equal(&self, a: Id, b: Id) -> Result<bool> {
        let mut exhausted = false;
        let result = self.equal_inner(a, b, usize::MAX, &mut exhausted);
        if exhausted {
            return Err(Error::new(
                "RecursionError",
                "maximum comparison depth exceeded",
            ));
        }
        Ok(result)
    }

    /// Compare child values within the caller's remaining recursion budget.
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
            (Value::Set { items: x, .. }, Value::Set { items: y, .. }) => {
                x.len() == y.len() && x.iter().all(|&k| y.iter().any(|&j| equal(k, j)))
            }
            _ => match (self.sequence(a), self.sequence(b)) {
                (Some(x), Some(y)) => {
                    x.len() == y.len() && x.iter().zip(y).all(|(&i, j)| equal(i, j))
                }
                _ => false,
            },
        }
    }
    pub(super) fn compare(&self, op: &str, a: Id, b: Id) -> Result<Option<Ordering>> {
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
        self.format_value(id, false, &mut Vec::new())
    }

    pub(super) fn repr(&self, id: Id) -> String {
        self.format_value(id, true, &mut Vec::new())
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
            Value::Float(n) => crate::floats::render(*n).to_string(),
            Value::Complex(z) => z.repr(),
            Value::Repeat(repeat) => {
                let value = self.repr(repeat.value);
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
            Value::List(v) | Value::Tuple(v) | Value::Set { items: v, .. } => {
                let body = self.format_elements(v, active);
                match self.heap.get(id) {
                    Value::List(_) => format!("[{body}]"),
                    Value::Set { frozen: true, .. } if v.is_empty() => "frozenset()".into(),
                    Value::Set { frozen: true, .. } => format!("frozenset({{{body}}})"),
                    Value::Set { .. } if v.is_empty() => "set()".into(),
                    Value::Set { .. } => format!("{{{body}}}"),
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
            _ => "<object>".to_owned(),
        };
        if recursive.is_some() {
            active.pop();
        }
        result
    }
    pub(super) fn get_item(&mut self, obj: Id, key: Id) -> Result<Id> {
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
        if let Value::Dict(entries) | Value::DefaultDict { entries, .. } = self.heap.get(obj) {
            if !builtins::hashable(self, key) {
                return Err(Error::new("TypeError", "unhashable dictionary key"));
            }
            if let Some(&(_, v)) = entries.iter().find(|(k, _)| self.equal(*k, key)) {
                return Ok(v);
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
        if matches!(self.heap.get(obj), Value::Set { .. }) {
            return Err(Error::new(
                "TypeError",
                format!("'{}' object is not subscriptable", self.type_name(obj)),
            ));
        }
        if matches!(self.heap.get(obj), Value::DictView { .. }) {
            return Err(Error::new("TypeError", "object is not subscriptable"));
        }
        if let Value::Slice(parts) = *self.heap.get(key) {
            return self.slice(obj, parts);
        }
        let i = self.as_integer(key)?;
        let vals = self.items(obj)?;
        let i = if i < 0 { vals.len() as i64 + i } else { i };
        vals.get(i as usize)
            .copied()
            .ok_or_else(|| Error::new("IndexError", "index out of range"))
    }
    pub(super) fn put_item(&mut self, obj: Id, key: Id, val: Id) -> Result<()> {
        if let Value::Dict(entries) | Value::DefaultDict { entries, .. } = self.heap.get(obj) {
            if !builtins::hashable(self, key) {
                return Err(Error::new("TypeError", "unhashable dictionary key"));
            }
            let index = entries.iter().position(|(k, _)| self.equal(*k, key));
            let (Value::Dict(entries) | Value::DefaultDict { entries, .. }) =
                self.heap.get_mut(obj)
            else {
                unreachable!()
            };
            if let Some(i) = index {
                entries[i].1 = val
            } else {
                entries.push((key, val));
            }
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
            *items
                .get_mut(i as usize)
                .ok_or_else(|| Error::new("IndexError", "list assignment index out of range"))? =
                val;
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
        ) && !builtins::hashable(self, item)
        {
            return Err(Error::new("TypeError", "unhashable key"));
        }
        let iterator = self.iterator(container)?;
        self.heap.keep(iterator);
        while let Some(value) = self.next_item(iterator)? {
            if self.checked_equal(value, item)? {
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

    pub(super) fn binary(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
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
            "%" if let Value::Str(text) = self.heap.get(a) => {
                return builtins::percent(self, &text.clone(), b);
            }
            "%" if let Value::Bytes(bytes) = self.heap.get(a) => {
                return builtins::bytes_percent(self, &bytes.clone(), b);
            }
            "==" | "!=" => {
                let equal = self.checked_equal(a, b)?
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
        let (x, y) = (self.as_int(a)?, self.as_int(b)?);
        if op == "/" {
            return Ok(self.alloc(Value::Float(crate::floats::ratio(&x, &y)?)));
        }
        let result = if op == "**" {
            let (x, y) = (x.big(), y.big());
            if y.is_negative() {
                if x.is_zero() {
                    return Err(Error::new(
                        "ZeroDivisionError",
                        "0.0 cannot be raised to a negative power",
                    ));
                }
                return Err(Error::new(
                    "TypeError",
                    "negative powers require floating-point results",
                ));
            }
            if let Some(exp) = y.to_u32() {
                crate::Int::new(x.pow(exp))
            } else if x.is_zero() || x.magnitude().bits() == 1 {
                crate::Int::from(if x.is_zero() {
                    0
                } else if x.is_negative() && y.bit(0) {
                    -1
                } else {
                    1
                })
            } else {
                return Err(Error::new("OverflowError", "exponent is too large"));
            }
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
        Ok(self.alloc(Value::Int(result)))
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
            return Ok(None);
        };
        let result = self.call(method, vec![other], vec![])?;
        Ok((result != value::NOT_IMPLEMENTED).then_some(result))
    }

    fn binary_special(&mut self, op: &str, a: Id, b: Id) -> Result<Option<Id>> {
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
        if let Some((_, _, slot)) = binary_slots(op)
            && let Some(result) = self.operator_slot(a, slot, b)?
        {
            return Ok(result);
        }
        if op == "+" && matches!(self.heap.get(a), Value::List(_)) {
            let extra = self.items(b)?;
            let Value::List(values) = self.heap.get_mut(a) else {
                unreachable!()
            };
            values.extend(extra);
            return Ok(a);
        }
        if op == "*"
            && let Value::List(values) = self.heap.get(a)
        {
            let values = repeat_values(values, self.as_integer(b)?)?;
            let Value::List(items) = self.heap.get_mut(a) else {
                unreachable!()
            };
            *items = values;
            return Ok(a);
        }
        self.binary(op, a, b)
    }

    pub(super) fn delete_item(&mut self, obj: Id, key: Id) -> Result<()> {
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
                entries.remove(i as usize);
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
