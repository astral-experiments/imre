//! A boxed selector holds Python keys and shares the native descriptor poller with typed Imre.
use super::super::{
    Context, builtins,
    value::{Error, Id, NONE, Result, Value},
};
use crate::objects::readiness::{Poll, PollError};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

const READ: i64 = 1;
const WRITE: i64 = 2;

#[derive(Clone, Copy)]
struct Registration {
    key: Id,
    fileobj: Id,
    interests: i64,
    data: Id,
}

#[derive(Default)]
pub(in crate::dynamic) struct Selector {
    poll: Poll,
    keys: HashMap<i32, Registration>,
    closed: bool,
}

/// Install both constructor names and the concrete key class in one module scope.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    let selector = ctx.alloc(Value::Native("selectors.PollSelector".into()));
    ctx.bind(scope, "PollSelector", selector);
    ctx.bind(scope, "DefaultSelector", selector);
    // Construct an ordinary named tuple class so that indexing, attribute access and
    // isinstance all refer to the same public SelectorKey type.
    let class_scope = ctx.scope("selectors", Some(scope));
    let class_name = ctx.text("SelectorKey");
    let module_name = ctx.text("selectors");
    ctx.bind(class_scope, "__name__", class_name);
    ctx.bind(class_scope, "__module__", module_name);
    let names = ["fileobj", "fd", "events", "data"].map(|field| ctx.text(field));
    let annotations = ctx.dict(names.iter().map(|&id| (id, NONE)).collect());
    let fields = ctx.alloc(Value::Tuple(names.to_vec()));
    ctx.bind(class_scope, "__annotations__", annotations);
    ctx.bind(class_scope, "_fields", fields);
    let base = ctx.builtin("typing.NamedTuple");
    let key = ctx
        .allocate_class("SelectorKey".into(), class_scope, vec![base])
        .expect("a single named tuple base has a linearization");
    ctx.bind(scope, "SelectorKey", key);
}

/// A positive Python descriptor may be wider than the kernel's descriptor type.
/// Such a value is an ordinary missing key until an operation actually calls poll.
fn fd(ctx: &mut Context, obj: Id) -> Result<Option<i32>> {
    let n = if matches!(ctx.heap.get(obj), Value::Int(_) | Value::Bool(_)) {
        ctx.as_int(obj)?
    } else {
        let number = ctx
            .lookup_attr(obj, "fileno")
            .and_then(|f| ctx.call(f, Vec::new(), Vec::new()))
            .and_then(|v| builtins::call(ctx, "int", &[v], &[]))
            .and_then(|v| ctx.as_int(v));
        number.map_err(|e| {
            if matches!(
                e.kind.as_str(),
                "AttributeError" | "TypeError" | "ValueError"
            ) {
                Error::new(
                    "ValueError",
                    format!("Invalid file object: {}", ctx.repr(obj)),
                )
            } else {
                e
            }
        })?
    };
    if n.cmp(&crate::Int::from(0)).is_lt() {
        return Err(Error::new(
            "ValueError",
            format!("Invalid file descriptor: {n}"),
        ));
    }
    Ok(n.to_i64().and_then(|n| i32::try_from(n).ok()))
}

fn reg_fd(ctx: &mut Context, handle: &RefCell<Selector>, obj: Id) -> Result<Option<i32>> {
    match fd(ctx, obj) {
        Err(e) if e.kind == "ValueError" => {
            // Lookup by identity still permits unregistering a socket that was closed.
            for (&fd, key) in &handle.borrow().keys {
                if key.fileobj == obj {
                    return Ok(Some(fd));
                }
            }
            Err(e)
        }
        result => result,
    }
}

fn poll_mask(n: i64) -> i16 {
    (if n & READ != 0 { libc::POLLIN } else { 0 })
        | (if n & WRITE != 0 { libc::POLLOUT } else { 0 })
}

/// Convert the selector's seconds to poll's milliseconds, rounding up as CPython does.
fn timeout(ctx: &Context, value: Id) -> Result<Option<Duration>> {
    if value == NONE {
        return Ok(None);
    }
    let s = match ctx.heap.get(value) {
        Value::Float(value) => *value,
        _ => {
            let integer = ctx.as_int(value)?;
            if integer.cmp(&crate::Int::from(0)).is_le() {
                return Ok(Some(Duration::ZERO));
            }
            crate::floats::int_to_float(&integer)
                .map_err(|_| Error::new("OverflowError", "int too large to convert to float"))?
        }
    };
    if s.is_nan() {
        return Err(Error::new(
            "ValueError",
            "cannot convert float NaN to integer",
        ));
    }
    let ms = (s.max(0.0) * 1000.0).ceil();
    if ms.is_infinite() {
        return Err(Error::new(
            "OverflowError",
            "cannot convert float infinity to integer",
        ));
    }
    if ms > f64::from(i32::MAX) {
        return Err(Error::new("OverflowError", "timeout is too large"));
    }
    Ok(Some(Duration::from_millis(ms as u64)))
}

fn registered(
    ctx: &Context,
    selector: &Selector,
    obj: Id,
    fd: Option<i32>,
) -> Result<(i32, Registration)> {
    fd.and_then(|fd| selector.keys.get(&fd).copied().map(|key| (fd, key)))
        .ok_or_else(|| Error::new("KeyError", format!("{} is not registered", ctx.repr(obj))))
}

impl Selector {
    pub(in crate::dynamic) fn trace(&self, roots: &mut Vec<Id>) {
        for entry in self.keys.values() {
            roots.extend([entry.key, entry.fileobj, entry.data]);
        }
    }

    /// Each selected pair reuses the key returned by register or modify.
    fn select(&mut self, ctx: &mut Context, seconds: Id) -> Result<Id> {
        let (count, events) =
            self.poll
                .ready(timeout(ctx, seconds)?)
                .map_err(|error| match error {
                    PollError::Overflow => Error::new("OverflowError", "too many file descriptors"),
                    PollError::Io(error) => Error::new("OSError", error.to_string()),
                })?;
        let mut results = Vec::new();
        results
            .try_reserve(count)
            .map_err(|_| Error::new("MemoryError", "out of memory"))?;
        for (fd, flags) in events {
            let Some(&key) = self.keys.get(&fd) else {
                continue;
            };
            let mut ready = 0;
            if flags & !(libc::POLLIN as u16) != 0 {
                ready |= WRITE;
            }
            if flags & !(libc::POLLOUT as u16) != 0 {
                ready |= READ;
            }
            let mask = ctx.int(ready & key.interests);
            results.push(ctx.alloc(Value::Tuple(vec![key.key, mask])));
        }
        Ok(ctx.alloc(Value::List(results)))
    }

    fn close(&mut self) {
        self.keys.clear();
        self.closed = true;
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let bad = || {
        Error::new(
            "TypeError",
            format!("invalid arguments to selectors.{name}"),
        )
    };
    if name == "PollSelector" {
        builtins::bind(name, args, kwargs, [])?;
        return Ok(ctx.alloc(Value::Selector(Rc::new(RefCell::new(Selector::default())))));
    }
    let (&receiver, args) = args.split_first().ok_or_else(bad)?;
    let Value::Selector(handle) = ctx.heap.get(receiver).clone() else {
        return Err(bad());
    };
    match name {
        "register" | "modify" => {
            let [obj, events, data] =
                builtins::bind(name, args, kwargs, ["fileobj", "events", "data"])?;
            let (Some(obj), Some(events)) = (obj, events) else {
                return Err(bad());
            };
            let data = data.unwrap_or(NONE);
            let bits = ctx.as_integer(events)?;
            if name == "register" && (bits == 0 || bits & !(READ | WRITE) != 0) {
                return Err(Error::new("ValueError", format!("Invalid events: {bits}")));
            }
            let fd = reg_fd(ctx, &handle, obj)?;
            let (fd, fileobj) = if name == "modify" {
                let (fd, key) = registered(ctx, &handle.borrow(), obj, fd)?;
                if key.interests == bits && ctx.equal(key.data, data) {
                    return Ok(key.key);
                }
                (fd, key.fileobj)
            } else {
                let fd = fd.ok_or_else(|| {
                    Error::new("OverflowError", "Python int too large to convert to C int")
                })?;
                if handle.borrow().keys.contains_key(&fd) {
                    return Err(Error::new(
                        "KeyError",
                        format!("{} (FD {fd}) is already registered", ctx.repr(obj)),
                    ));
                }
                handle
                    .borrow_mut()
                    .keys
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", "out of memory"))?;
                (fd, obj)
            };
            let module = ctx.module("selectors")?;
            let class = ctx.lookup_attr(module, "SelectorKey")?;
            let number = ctx.int(i64::from(fd));
            let key = ctx.call(class, vec![fileobj, number, events, data], Vec::new())?;
            let mut selector = handle.borrow_mut();
            selector
                .poll
                .set(fd, poll_mask(bits))
                .map_err(|_| Error::new("MemoryError", "out of memory"))?;
            selector.keys.insert(
                fd,
                Registration {
                    key,
                    fileobj,
                    interests: bits,
                    data,
                },
            );
            Ok(key)
        }
        "get_key" | "unregister" => {
            let [obj] = builtins::bind(name, args, kwargs, ["fileobj"])?;
            let obj = obj.ok_or_else(bad)?;
            if name == "get_key" && handle.borrow().closed {
                return Err(Error::new("RuntimeError", "Selector is closed"));
            }
            let fd = reg_fd(ctx, &handle, obj)?;
            let mut selector = handle.borrow_mut();
            let (fd, key) = registered(ctx, &selector, obj, fd)?;
            if name == "unregister" {
                selector.poll.remove(fd);
                selector.keys.remove(&fd);
            }
            Ok(key.key)
        }
        "select" => {
            let [seconds] = builtins::bind(name, args, kwargs, ["timeout"])?;
            handle.borrow_mut().select(ctx, seconds.unwrap_or(NONE))
        }
        "__enter__" | "close" => {
            builtins::bind(name, args, kwargs, [])?;
            if name == "__enter__" {
                return Ok(receiver);
            }
            handle.borrow_mut().close();
            Ok(NONE)
        }
        "__exit__" => {
            let args = builtins::bind(name, args, kwargs, ["exc_type", "exc", "tb"])?;
            if args.iter().any(Option::is_none) {
                return Err(bad());
            }
            handle.borrow_mut().close();
            Ok(NONE)
        }
        _ => Err(bad()),
    }
}
