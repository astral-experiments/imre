use super::{Context, Error, Id, Result, Value};
use std::ops::RangeInclusive;

fn check_args(
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
    arity: RangeInclusive<usize>,
) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes no keyword arguments"),
        ));
    }
    if !arity.contains(&args.len()) {
        return Err(Error::new(
            "TypeError",
            format!("{name} expected 1 argument, got {}", args.len()),
        ));
    }
    Ok(())
}

pub(super) fn attrgetter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    check_args("attrgetter", args, kwargs, 1..=usize::MAX)?;
    let paths = args
        .iter()
        .map(|&id| {
            let Value::Str(name) = ctx.heap.get(id) else {
                return Err(Error::new("TypeError", "attribute name must be a string"));
            };
            Ok(name.split('.').map(str::to_owned).collect())
        })
        .collect::<Result<_>>()?;
    Ok(ctx.alloc(Value::AttrGetter(paths)))
}

pub(super) fn itemgetter(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    check_args("itemgetter", args, kwargs, 1..=usize::MAX)?;
    let mut keys = Vec::new();
    keys.try_reserve_exact(args.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    keys.extend_from_slice(args);
    let keys = ctx.alloc(Value::Tuple(keys));
    Ok(ctx.alloc(Value::ItemGetter(keys)))
}

/// Reconstruct the getter with the original positional key objects, including a single tuple key.
pub(super) fn reduce_itemgetter(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&getter, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "unbound method itemgetter.__reduce__() needs an argument",
        ));
    };
    let Value::ItemGetter(keys) = *ctx.heap.get(getter) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '__reduce__' for 'operator.itemgetter' objects doesn't apply to a '{}' object",
                ctx.type_name(getter)
            ),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "itemgetter.__reduce__() takes no keyword arguments",
        ));
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "itemgetter.__reduce__() takes no arguments ({} given)",
                rest.len()
            ),
        ));
    }
    let args = match *ctx.itemgetter_keys(keys) {
        [key] => ctx.alloc(Value::Tuple(vec![key])),
        _ => keys,
    };
    let class = ctx.builtin("operator.itemgetter");
    Ok(ctx.alloc(Value::Tuple(vec![class, args])))
}

impl Context {
    pub(in super::super) fn itemgetter_keys(&self, keys: Id) -> &[Id] {
        let Value::Tuple(items) = self.heap.get(keys) else {
            unreachable!()
        };
        items
    }
    pub(in super::super) fn apply_attrgetter(
        &mut self,
        paths: &[Vec<String>],
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        self.apply_getter("attrgetter", paths.len(), args, kwargs, |ctx, obj, i| {
            paths[i]
                .iter()
                .try_fold(obj, |obj, name| ctx.lookup_attr(obj, name))
        })
    }

    pub(in super::super) fn apply_itemgetter(
        &mut self,
        keys: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        self.apply_getter(
            "itemgetter",
            self.itemgetter_keys(keys).len(),
            args,
            kwargs,
            |ctx, obj, i| ctx.get_item(obj, ctx.itemgetter_keys(keys)[i]),
        )
    }

    /// Retain each result across later lookups, which can run arbitrary Python.
    fn apply_getter(
        &mut self,
        name: &str,
        count: usize,
        args: &[Id],
        kwargs: &[(String, Id)],
        lookup: impl Fn(&mut Self, Id, usize) -> Result<Id>,
    ) -> Result<Id> {
        check_args(name, args, kwargs, 1..=1)?;
        if count == 1 {
            return lookup(self, args[0], 0);
        }
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| Error::new("MemoryError", ""))?;
        for i in 0..count {
            let value = lookup(self, args[0], i)?;
            self.heap.keep(value);
            values.push(value);
        }
        Ok(self.alloc(Value::Tuple(values)))
    }
}

use super::super::{builtins, value::NONE};

/// Operations captured by Werkzeug's LocalProxy descriptor declarations.
/// Binary and item operations share the boxed expression dispatch.
#[derive(Clone, Copy)]
pub(in crate::dynamic) enum Operation {
    Binary(&'static str),
    InPlace(&'static str),
    Unary(&'static str),
    Contains,
    GetItem,
    SetItem,
    DelItem,
    Index,
    LengthHint,
}

macro_rules! operator_functions {
    ($($name:literal => $operation:expr),* $(,)?) => {
        impl Operation {
            const NAMES: &[&str] = &[$($name),*];

            pub(in crate::dynamic) fn named(name: &str) -> Option<Self> {
                Some(match name {
                    $($name => $operation,)*
                    _ => return None,
                })
            }
        }
    };
}

include!("../../../native/operator-functions.rs");

/// Install the same callable objects in the private extension and its public wrapper.
pub(in crate::dynamic) fn install(ctx: &mut Context, owner: Id, scope: Id, module: &str) {
    if module == "_operator" {
        super::hmac::bind_compare(ctx, owner, scope, "_operator._compare_digest");
    }
    for &name in Operation::NAMES {
        let function = ctx.builtin(&format!("operator.{name}"));
        ctx.bind(scope, name, function);
        if module == "operator" && name != "length_hint" {
            let alias = format!("__{}__", name.trim_end_matches('_'));
            ctx.bind(scope, &alias, function);
        }
    }
    for name in ["attrgetter", "itemgetter"] {
        let class = ctx.builtin(&format!("operator.{name}"));
        ctx.bind(scope, name, class);
    }
}

impl Operation {
    pub(super) fn call(
        self,
        ctx: &mut Context,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("_operator.{name}() takes no keyword arguments"),
            ));
        }
        let actual = args.len();
        let arity = match self {
            Self::Unary(_) | Self::Index => 1,
            Self::SetItem => 3,
            _ => 2,
        };
        if matches!(self, Self::LengthHint) {
            if !(1..=2).contains(&actual) {
                let expected = if actual == 0 {
                    "at least 1 argument"
                } else {
                    "at most 2 arguments"
                };
                return Err(Error::new(
                    "TypeError",
                    format!("{name} expected {expected}, got {actual}"),
                ));
            }
        } else if actual != arity {
            let message = if arity == 1 {
                format!("_operator.{name}() takes exactly one argument ({actual} given)")
            } else {
                format!("{name} expected {arity} arguments, got {actual}")
            };
            return Err(Error::new("TypeError", message));
        }
        let obj = args[0];
        match self {
            Self::Binary(op) => ctx.binary(op, obj, args[1]),
            Self::InPlace(op) => ctx.inplace(op, obj, args[1]),
            Self::Unary(op) => ctx.op(op, obj, NONE, NONE),
            Self::Contains => ctx.binary("in", args[1], obj),
            Self::GetItem => ctx.get_item(obj, args[1]),
            Self::SetItem => {
                ctx.put_item(obj, args[1], args[2])?;
                Ok(NONE)
            }
            Self::DelItem => {
                ctx.delete_item(obj, args[1])?;
                Ok(NONE)
            }
            Self::Index if matches!(ctx.heap.get(obj), Value::Int(_)) => Ok(obj),
            Self::Index => {
                let value = ctx.index_integer(obj)?;
                Ok(ctx.alloc(Value::Int(value)))
            }
            Self::LengthHint => ctx.length_hint(obj, args.get(1).copied()),
        }
    }
}

impl Context {
    pub(in crate::dynamic) fn length_hint(&mut self, value: Id, default: Option<Id>) -> Result<Id> {
        // PyObject_LengthHint parses the default before consulting either protocol.
        let size_overflow = || {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        };
        let default = if let Some(id) = default {
            self.index_integer(id)?.to_i64().ok_or_else(size_overflow)?
        } else {
            0
        };
        match builtins::call(self, "len", &[value], &[]) {
            Ok(length) => return Ok(length),
            Err(error) if self.is_builtin_error(&error, "TypeError") => {}
            Err(error) => return Err(error),
        }
        if let Value::SequenceIterator { source, index } = *self.heap.get(value) {
            let hint = match source {
                None => 0,
                Some(seq) => match builtins::call(self, "len", &[seq], &[]) {
                    Ok(length) => {
                        (self.as_int(length)?.to_i64().ok_or_else(size_overflow)? - index).max(0)
                    }
                    Err(error) if self.is_builtin_error(&error, "TypeError") => default,
                    Err(error) => return Err(error),
                },
            };
            return Ok(self.int(hint));
        }
        if let Value::ReverseSequence { source, remaining } = *self.heap.get(value) {
            let hint = match source {
                Some(seq) if remaining != 0 && remaining <= self.sequence_len(seq)? => {
                    remaining
                }
                _ => 0,
            };
            return Ok(self.int(hint as i64));
        }
        if matches!(self.heap.get(value), Value::Iterator { .. }) {
            let length = i64::try_from(self.iterator_remaining(value)?.len())
                .map_err(|_| size_overflow())?;
            return Ok(self.int(length));
        }
        if let Value::DequeIterator { remaining, .. } = *self.heap.get(value) {
            return Ok(self.int(remaining as i64));
        }
        if let Value::RangeIterator(iter) = self.heap.get(value) {
            let n = iter.remaining.to_i64().ok_or_else(size_overflow)?;
            return Ok(self.int(n));
        }
        if let Value::DictIterator(iter) = self.heap.get(value) {
            return Ok(self.int(iter.length_hint(self, default)));
        }
        if let Value::Repeat(repeat) = self.heap.get(value) {
            return Ok(self.int(repeat.remaining.map_or(default, |n| n as i64)));
        }
        if let Some(hint) = self.bound_slot(value, "__length_hint__")? {
            let hint = match self.call(hint, vec![], vec![]) {
                Ok(hint) => hint,
                Err(error) if self.is_builtin_error(&error, "TypeError") => {
                    return Ok(self.int(default));
                }
                Err(error) => return Err(error),
            };
            if hint == super::super::value::NOT_IMPLEMENTED {
                return Ok(self.int(default));
            }
            let integer = self.as_int(hint).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "__length_hint__ must be an integer, not {}",
                        self.type_name(hint)
                    ),
                )
            })?;
            let n = integer.to_i64().ok_or_else(size_overflow)?;
            if n < 0 {
                return Err(Error::new(
                    "ValueError",
                    "__length_hint__() should return >= 0",
                ));
            }
            return Ok(self.int(n));
        }
        Ok(self.int(default))
    }
}
