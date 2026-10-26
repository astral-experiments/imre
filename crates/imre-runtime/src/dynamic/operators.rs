use std::cmp::Ordering;

use num_traits::{Signed, ToPrimitive, Zero};

use super::*;

impl Context {
    fn sequence(&self, id: Id) -> Option<Vec<Id>> {
        match self.heap.get(id) {
            Value::List(v) | Value::Tuple(v) | Value::NamedTuple { values: v, .. } => {
                Some(v.clone())
            }
            Value::Instance { class, fields } => Some(
                self.namedtuple_fields(*class)?
                    .iter()
                    .map(|k| fields[k])
                    .collect(),
            ),
            _ => None,
        }
    }
    pub(super) fn truth(&self, id: Id) -> bool {
        match self.heap.get(id) {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Int(i) => i.as_small() != Some(0),
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::List(v)
            | Value::Tuple(v)
            | Value::Set(v)
            | Value::NamedTuple { values: v, .. } => !v.is_empty(),
            Value::Dict(v) | Value::DefaultDict { entries: v, .. } => !v.is_empty(),
            Value::DictView { dict, .. } => self.truth(*dict),
            Value::Context(bindings) => !bindings.is_empty(),
            Value::Range { start, stop, step } => {
                if *step > 0 {
                    start < stop
                } else {
                    start > stop
                }
            }
            _ => true,
        }
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
        let mut equal = |x, y| self.equal_inner(x, y, remaining - 1, exhausted);
        match (self.heap.get(a), self.heap.get(b)) {
            (Value::None, Value::None) => true,
            (Value::Int(_) | Value::Bool(_), Value::Int(_) | Value::Bool(_)) => self
                .as_int(a)
                .unwrap()
                .cmp(&self.as_int(b).unwrap())
                .is_eq(),
            (Value::Float(x), Value::Float(y)) => x == y,
            (Value::Float(f), Value::Int(_) | Value::Bool(_)) => {
                crate::floats::compare(&self.as_int(b).unwrap(), *f) == 0
            }
            (Value::Int(_) | Value::Bool(_), Value::Float(f)) => {
                crate::floats::compare(&self.as_int(a).unwrap(), *f) == 0
            }
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Native(a), Value::Native(b)) => a == b,
            (Value::List(x), Value::List(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(&i, &j)| equal(i, j))
            }
            (Value::List(_), _) | (_, Value::List(_)) => false,
            (Value::Dict(x), Value::Dict(y)) => {
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
            (Value::Set(x), Value::Set(y)) => {
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
    pub(super) fn compare(&self, a: Id, b: Id) -> Result<Option<Ordering>> {
        let int_float = |integer, float| {
            let order = crate::floats::compare(&self.as_int(integer)?, float);
            Ok((order != 2).then(|| order.cmp(&0)))
        };
        match (self.heap.get(a), self.heap.get(b)) {
            (Value::Int(_) | Value::Bool(_), Value::Int(_) | Value::Bool(_)) => {
                Ok(Some(self.as_int(a)?.cmp(&self.as_int(b)?)))
            }
            (Value::Float(x), Value::Float(y)) => Ok(x.partial_cmp(y)),
            (Value::Int(_) | Value::Bool(_), Value::Float(f)) => int_float(a, *f),
            (Value::Float(f), Value::Int(_) | Value::Bool(_)) => {
                Ok(int_float(b, *f)?.map(Ordering::reverse))
            }
            (Value::Str(x), Value::Str(y)) => Ok(Some(x.cmp(y))),
            (Value::Bytes(x), Value::Bytes(y)) => Ok(Some(x.cmp(y))),
            _ => {
                if matches!(self.heap.get(a), Value::List(_))
                    != matches!(self.heap.get(b), Value::List(_))
                {
                    return Err(Error::new("TypeError", "incomparable sequence types"));
                }
                if let (Some(x), Some(y)) = (self.sequence(a), self.sequence(b)) {
                    for (&i, &j) in x.iter().zip(&y) {
                        if !self.checked_equal(i, j)? {
                            return self.compare(i, j);
                        }
                    }
                    Ok(Some(x.len().cmp(&y.len())))
                } else {
                    Err(Error::new("TypeError", "objects do not support ordering"))
                }
            }
        }
    }
    pub(super) fn display(&self, id: Id) -> String {
        match self.heap.get(id) {
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
            Value::None => "None".into(),
            Value::Ellipsis => "Ellipsis".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) => crate::floats::render(*n).to_string(),
            Value::Str(s) => s.clone(),
            Value::Bytes(bytes) => format!(
                "b'{}'",
                bytes
                    .iter()
                    .flat_map(|&x| std::ascii::escape_default(x))
                    .map(char::from)
                    .collect::<String>()
            ),
            Value::Dict(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", self.repr(*k), self.repr(*v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Value::List(v) | Value::Tuple(v) | Value::Set(v) => {
                let body = v
                    .iter()
                    .map(|&v| self.repr(v))
                    .collect::<Vec<_>>()
                    .join(", ");
                match self.heap.get(id) {
                    Value::List(_) => format!("[{body}]"),
                    Value::Set(_) if v.is_empty() => "set()".into(),
                    Value::Set(_) => format!("{{{body}}}"),
                    _ if v.len() == 1 => format!("({body},)"),
                    _ => format!("({body})"),
                }
            }
            Value::Raised(Error { message, .. }) => message.clone(),
            Value::Class { name, .. } | Value::Native(name) => format!("<class '{name}'>"),
            Value::Module { name, .. } => format!("<module '{name}'>"),
            Value::Function { name, .. } => format!("<function {name}>"),
            _ => "<object>".to_owned(),
        }
    }
    pub(super) fn repr(&self, id: Id) -> String {
        if let Value::Str(s) = self.heap.get(id) {
            let mut quoted = String::new();
            crate::objects::exceptions::write_string_repr(s, &mut quoted)
                .expect("writing into a string cannot fail");
            quoted
        } else {
            self.display(id)
        }
    }
    pub(super) fn get_item(&mut self, obj: Id, key: Id) -> Result<Id> {
        if let Value::Context(bindings) = self.heap.get(obj) {
            self.context_key(key)?;
            return bindings
                .get(&key)
                .copied()
                .ok_or_else(|| self.context_lookup_error("KeyError", key));
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
            return Err(Error::new("KeyError", self.repr(key)));
        }
        if matches!(self.heap.get(obj), Value::DictView { .. } | Value::Set(_)) {
            return Err(Error::new("TypeError", "object is not subscriptable"));
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
        } else {
            return Err(Error::new(
                "TypeError",
                "object does not support item assignment",
            ));
        }
        Ok(())
    }
    pub(super) fn contains(&mut self, item: Id, container: Id) -> Result<bool> {
        if let Value::Context(bindings) = self.heap.get(container) {
            self.context_key(item)?;
            return Ok(bindings.contains_key(&item));
        }
        if let Value::Str(hay) = self.heap.get(container) {
            return Ok(hay.contains(self.str_value(item)?));
        }
        if matches!(
            self.heap.get(container),
            Value::Dict(_) | Value::DefaultDict { .. } | Value::Set(_)
        ) && !builtins::hashable(self, item)
        {
            return Err(Error::new("TypeError", "unhashable key"));
        }
        for value in self.items(container)? {
            if self.checked_equal(value, item)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn slice(&mut self, obj: Id, spec: Id) -> Result<Id> {
        let indices = self.items(spec)?;
        let vals = self.items(obj)?;
        let len = vals.len() as i64;
        let index = |at: usize| -> Result<Option<i64>> {
            indices
                .get(at)
                .copied()
                .filter(|&v| v != NONE)
                .map(|v| self.as_integer(v))
                .transpose()
        };
        let step = index(2)?.unwrap_or(1);
        if step == 0 {
            return Err(Error::new("ValueError", "slice step cannot be zero"));
        }
        let norm = |v: i64| {
            let v = if v < 0 { len.saturating_add(v) } else { v };
            if step > 0 {
                v.clamp(0, len)
            } else {
                v.clamp(-1, len - 1)
            }
        };
        let mut i = index(0)?
            .map(norm)
            .unwrap_or(if step > 0 { 0 } else { len - 1 });
        let end = index(1)?
            .map(norm)
            .unwrap_or(if step > 0 { len } else { -1 });
        let mut out = Vec::new();
        while if step > 0 { i < end } else { i > end } {
            out.push(vals[i as usize]);
            i = i.saturating_add(step);
        }
        match self.heap.get(obj) {
            Value::Str(_) => {
                let s = out
                    .iter()
                    .map(|&i| self.str_value(i))
                    .collect::<Result<Vec<_>>>()?
                    .concat();
                Ok(self.text(s))
            }
            Value::Bytes(_) => {
                let bytes = out
                    .iter()
                    .map(|&i| self.as_integer(i).map(|x| x as u8))
                    .collect::<Result<_>>()?;
                Ok(self.alloc(Value::Bytes(bytes)))
            }
            Value::List(_) => Ok(self.alloc(Value::List(out))),
            _ => Ok(self.alloc(Value::Tuple(out))),
        }
    }
    pub(super) fn binary(&mut self, op: &str, a: Id, b: Id) -> Result<Id> {
        let truth = |v| if v { TRUE } else { FALSE };
        match op {
            "%" if let Value::Str(text) = self.heap.get(a) => {
                return builtins::percent(self, &text.clone(), b);
            }
            "==" | "!=" => {
                let equal = self.checked_equal(a, b)?
                    && !matches!(self.heap.get(a), Value::Float(n) if n.is_nan());
                return Ok(truth(equal == (op == "==")));
            }
            "is" => return Ok(truth(a == b)),
            "is not" => return Ok(truth(a != b)),
            "in" => return Ok(truth(self.contains(a, b)?)),
            "not in" => return Ok(truth(!self.contains(a, b)?)),
            "<" | "<=" | ">" | ">=" => {
                let Some(c) = self.compare(a, b)? else {
                    return Ok(FALSE);
                };
                return Ok(truth(match op {
                    "<" => c.is_lt(),
                    "<=" => c.is_le(),
                    ">" => c.is_gt(),
                    _ => c.is_ge(),
                }));
            }
            "+" => match (self.heap.get(a), self.heap.get(b)) {
                (Value::Str(x), Value::Str(y)) => return Ok(self.text(format!("{x}{y}"))),
                (Value::List(x), Value::List(y)) => {
                    return Ok(self.alloc(Value::List([x.as_slice(), y.as_slice()].concat())));
                }
                (Value::Tuple(x), Value::Tuple(y)) => {
                    return Ok(self.alloc(Value::Tuple([x.as_slice(), y.as_slice()].concat())));
                }
                _ => {}
            },
            _ => {}
        }
        let (x, y) = (self.as_int(a)?, self.as_int(b)?);
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
