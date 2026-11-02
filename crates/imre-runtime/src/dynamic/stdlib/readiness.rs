//! Boxed selectors retain Python keys and wait through their respective native backends.
use super::super::{
    Context, builtins,
    value::{Error, Id, MarkQueue, NONE, Result, Value},
};
use crate::objects::{
    IntKey,
    readiness::{Poll, SelectFds},
};
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
    poll: Option<Poll>,
    keys: HashMap<IntKey, Registration>,
    closed: bool,
}

/// Install the selector constructors and the concrete key class in one module scope.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) -> Result<()> {
    let selector = ctx.builtin("selectors.PollSelector");
    let select = ctx.builtin("selectors.SelectSelector");
    ctx.bind(scope, "SelectSelector", select);
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
    ctx.bind(class_scope, "__annotations__", annotations);
    let base = ctx.builtin("typing.NamedTuple");
    let key = ctx.allocate_class("SelectorKey".into(), class_scope, vec![base])?;
    ctx.bind(scope, "SelectorKey", key);
    Ok(())
}

/// Keep the full Python integer until the backend needs a native descriptor.
fn fd(ctx: &mut Context, obj: Id) -> Result<IntKey> {
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
                e.kind.as_ref(),
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
    Ok(IntKey(n))
}

fn native_fd(fd: &IntKey) -> Result<i32> {
    fd.0.to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn reg_fd(ctx: &mut Context, handle: &RefCell<Selector>, obj: Id) -> Result<IntKey> {
    match fd(ctx, obj) {
        Err(e) if e.kind == "ValueError" => {
            // Lookup by identity still permits unregistering a socket that was closed.
            for (fd, key) in &handle.borrow().keys {
                if key.fileobj == obj {
                    return Ok(fd.clone());
                }
            }
            Err(e)
        }
        result => result,
    }
}

fn register_events(ctx: &mut Context, events: Id) -> Result<i64> {
    let forbidden = ctx.int(!(READ | WRITE));
    if !ctx.truth(events)? || {
        let other = ctx.binary("&", events, forbidden)?;
        ctx.truth(other)?
    } {
        return Err(Error::new(
            "ValueError",
            format!("Invalid events: {}", ctx.repr(events)),
        ));
    }
    let read = ctx.int(READ);
    let read = ctx.binary("&", events, read)?;
    let write = ctx.int(WRITE);
    let write = ctx.binary("&", events, write)?;
    Ok((i64::from(ctx.truth(read)?) * READ) | (i64::from(ctx.truth(write)?) * WRITE))
}

fn poll_mask(n: i64) -> i16 {
    (if n & READ != 0 { libc::POLLIN } else { 0 })
        | (if n & WRITE != 0 { libc::POLLOUT } else { 0 })
}

/// Select waits in seconds; poll rounds positive seconds up to milliseconds.
fn timeout(ctx: &mut Context, value: Id, poll: bool) -> Result<Option<Duration>> {
    if value == NONE {
        return Ok(None);
    }
    if !poll {
        let zero = ctx.int(0);
        let seconds = builtins::call(ctx, "max", &[value, zero], &[])?;
        return super::select::timeout(ctx, Some(&seconds), false);
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

fn registered(ctx: &Context, selector: &Selector, obj: Id, fd: &IntKey) -> Result<Registration> {
    selector
        .keys
        .get(fd)
        .copied()
        .ok_or_else(|| Error::new("KeyError", format!("{} is not registered", ctx.repr(obj))))
}

impl Selector {
    pub(in crate::dynamic) fn trace(&self, roots: &mut MarkQueue<'_>) {
        for entry in self.keys.values() {
            roots.extend([entry.key, entry.fileobj, entry.data]);
        }
    }

    /// Each selected pair reuses the key returned by register or modify.
    fn select(&mut self, ctx: &mut Context, timeout: Option<Duration>) -> Result<Id> {
        let Some(poll) = &mut self.poll else {
            let mut sets = SelectFds::new();
            for (index, (fd, key)) in self.keys.iter().enumerate() {
                let fd = native_fd(fd)?;
                if key.interests & READ != 0 {
                    sets.insert(0, index, fd)?;
                }
                if key.interests & WRITE != 0 {
                    sets.insert(1, index, fd)?;
                }
            }
            let mut results = Vec::new();
            if sets.wait(timeout)? {
                for (fd, key) in &self.keys {
                    let fd = native_fd(fd)?;
                    let events = (i64::from(sets.ready(0, fd)) * READ)
                        | (i64::from(sets.ready(1, fd)) * WRITE);
                    if events != 0 {
                        results
                            .try_reserve(1)
                            .map_err(|_| Error::new("MemoryError", "out of memory"))?;
                        let mask = ctx.int(events);
                        results.push(ctx.alloc(Value::Tuple(vec![key.key, mask])));
                    }
                }
            }
            return Ok(ctx.alloc(Value::List(results)));
        };
        let (count, events) = poll.ready(timeout)?;
        let mut results = Vec::new();
        results
            .try_reserve(count)
            .map_err(|_| Error::new("MemoryError", "out of memory"))?;
        for (fd, flags) in events {
            let Some(&key) = self.keys.get(&IntKey(crate::Int::from(i64::from(fd)))) else {
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

    pub(in crate::dynamic) fn class_name(&self) -> &'static str {
        if self.poll.is_some() {
            "selectors.PollSelector"
        } else {
            "selectors.SelectSelector"
        }
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
    if matches!(name, "PollSelector" | "SelectSelector") {
        builtins::bind(name, args, kwargs, [])?;
        let selector = Selector {
            poll: (name == "PollSelector").then(Poll::default),
            ..Selector::default()
        };
        return Ok(ctx.alloc(Value::Selector(Rc::new(RefCell::new(selector)))));
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
            // Registration validates events first; modification looks up the key first.
            let bits = if name == "register" {
                Some(register_events(ctx, events)?)
            } else {
                None
            };
            let fd = reg_fd(ctx, &handle, obj)?;
            let (fileobj, class, bits) = if let Some(bits) = bits {
                if handle.borrow().keys.contains_key(&fd) {
                    return Err(Error::new(
                        "KeyError",
                        format!("{} (FD {}) is already registered", ctx.repr(obj), fd.0),
                    ));
                }
                handle
                    .borrow_mut()
                    .keys
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", "out of memory"))?;
                let module = ctx.module("selectors")?;
                (obj, ctx.lookup_attr(module, "SelectorKey")?, bits)
            } else {
                let key = registered(ctx, &handle.borrow(), obj, &fd)?;
                // A data comparison can unregister or replace the key and collect its old data.
                ctx.heap.keep(key.key);
                let bits = if handle.borrow().poll.is_none() {
                    let old = ctx.lookup_attr(key.key, "events")?;
                    let changed = ctx.binary("!=", events, old)?;
                    if ctx.truth(changed)? {
                        call(ctx, "unregister", &[receiver, obj], &[])?;
                        return call(ctx, "register", &[receiver, obj, events, data], &[]);
                    }
                    key.interests
                } else {
                    ctx.as_integer(events)?
                };
                let events_changed = key.interests != bits;
                if events_changed && let Some(poll) = &mut handle.borrow_mut().poll {
                    poll.set(native_fd(&fd)?, poll_mask(bits))
                        .map_err(|_| Error::new("MemoryError", "out of memory"))?;
                }
                let unequal = ctx.binary("!=", data, key.data)?;
                if !ctx.truth(unequal)? && !events_changed {
                    return Ok(key.key);
                }
                (key.fileobj, ctx.exact_type(key.key)?, bits)
            };
            let number = ctx.alloc(Value::Int(fd.0.clone()));
            let key = ctx.call(class, vec![fileobj, number, events, data], Vec::new())?;
            let mut selector = handle.borrow_mut();
            if name == "register" {
                if let Some(poll) = &mut selector.poll {
                    poll.set(native_fd(&fd)?, poll_mask(bits))
                        .map_err(|_| Error::new("MemoryError", "out of memory"))?;
                }
            } else if !selector.keys.contains_key(&fd) {
                selector
                    .keys
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", "out of memory"))?;
            }
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
            let key = registered(ctx, &selector, obj, &fd)?;
            if name == "unregister" {
                if let Some(poll) = &mut selector.poll {
                    poll.remove(native_fd(&fd)?);
                }
                selector.keys.remove(&fd);
            }
            Ok(key.key)
        }
        "select" => {
            let [seconds] = builtins::bind(name, args, kwargs, ["timeout"])?;
            let poll = handle.borrow().poll.is_some();
            let timeout = timeout(ctx, seconds.unwrap_or(NONE), poll)?;
            handle.borrow_mut().select(ctx, timeout)
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
