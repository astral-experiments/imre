//! Boxed selectors retain Python keys and wait through their respective native backends.
use super::super::{
    Context, builtins,
    value::{Error, Id, MarkQueue, NONE, Result, Value},
};
use crate::objects::readiness::{Poll, SelectFds};
use std::{cell::RefCell, rc::Rc, time::Duration};

const READ: i64 = 1;
const WRITE: i64 = 2;

#[derive(Clone, Copy)]
struct Registration {
    key: Id,
    fileobj: Id,
    interests: i64,
    data: Id,
}

pub(in crate::dynamic) struct Selector {
    poll: Option<Poll>,
    keys: Id,
    mapping: Id,
}

/// Install the selector constructors and the concrete key class in one module scope.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) -> Result<()> {
    let selector = ctx.builtin("selectors.PollSelector");
    let select = ctx.builtin("selectors.SelectSelector");
    ctx.bind(scope, "SelectSelector", select);
    ctx.bind(scope, "PollSelector", selector);
    ctx.bind(scope, "DefaultSelector", selector);
    let name = ctx.text("SelectorKey");
    let fields = ctx.text("fileobj fd events data");
    let module = ctx.text("selectors");
    let key =
        super::super::namedtuple::factory(ctx, &[name, fields], &[("module".into(), module)])?;
    ctx.bind(scope, "SelectorKey", key);
    let fields = ctx.scope("selectors", Some(scope));
    ctx.bind(fields, "__module__", module);
    for method in ["__init__", "__getitem__", "get", "__iter__", "__len__"] {
        let function = ctx.builtin(&format!("selectors._SelectorMapping.{method}"));
        ctx.bind(fields, method, function);
    }
    let base = super::super::mapping_abc::class(ctx, "Mapping")?;
    let class = ctx.allocate_class("_SelectorMapping".into(), fields, vec![base])?;
    ctx.bind(scope, "_SelectorMapping", class);
    Ok(())
}

/// Keep the full Python integer until the backend needs a native descriptor.
fn fd(ctx: &mut Context, obj: Id) -> Result<Id> {
    let number = if matches!(
        ctx.heap.get(ctx.scalar_value(obj)),
        Value::Int(_) | Value::Bool(_)
    ) {
        obj
    } else {
        match ctx
            .lookup_attr(obj, "fileno")
            .and_then(|f| ctx.call(f, Vec::new(), Vec::new()))
            .and_then(|v| builtins::call(ctx, "int", &[v], &[]))
        {
            Ok(number) => number,
            Err(e)
                if ["AttributeError", "TypeError", "ValueError"]
                    .iter()
                    .any(|kind| ctx.is_builtin_error(&e, kind)) =>
            {
                let name = ctx.checked_format(obj, true)?;
                return Err(Error::new(
                    "ValueError",
                    format!("Invalid file object: {name}"),
                ));
            }
            Err(e) => return Err(e),
        }
    };
    let zero = ctx.int(0);
    let negative = ctx.binary("<", number, zero)?;
    if ctx.truth(negative)? {
        let n = ctx.checked_format(number, false)?;
        return Err(Error::new(
            "ValueError",
            format!("Invalid file descriptor: {n}"),
        ));
    }
    Ok(number)
}

fn native_fd(ctx: &Context, fd: Id) -> Result<i32> {
    ctx.as_int(fd)?
        .to_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| Error::new("OverflowError", "Python int too large to convert to C int"))
}

fn reg_fd(ctx: &mut Context, handle: &RefCell<Selector>, obj: Id) -> Result<Id> {
    match fd(ctx, obj) {
        Err(e) if ctx.is_builtin_error(&e, "ValueError") => {
            // Lookup by identity still permits unregistering a socket that was closed.
            for &(fd, entry) in ctx.dict_entries(handle.borrow().keys).unwrap() {
                if Registration::unpack(ctx, entry).fileobj == obj {
                    return Ok(fd);
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

impl Registration {
    // The backend mask can differ from the Python event object retained by SelectorKey.
    fn pack(self, ctx: &mut Context) -> Id {
        let interests = ctx.int(self.interests);
        ctx.alloc(Value::Tuple(vec![
            self.key,
            self.fileobj,
            interests,
            self.data,
        ]))
    }

    fn unpack(ctx: &Context, entry: Id) -> Self {
        let Value::Tuple(fields) = ctx.heap.get(entry) else {
            unreachable!()
        };
        let Value::Int(interests) = ctx.heap.get(fields[2]) else {
            unreachable!()
        };
        Self {
            key: fields[0],
            fileobj: fields[1],
            interests: interests.to_i64().unwrap(),
            data: fields[3],
        }
    }
}

fn registration(ctx: &mut Context, keys: Id, fd: Id) -> Result<Option<Registration>> {
    let (_, position) = ctx.find_dictionary_key(keys, fd, None)?;
    Ok(position.map(|i| Registration::unpack(ctx, ctx.dict_entries(keys).unwrap()[i].1)))
}

fn missing<T>(ctx: &mut Context, fileobj: Id) -> Result<T> {
    let name = ctx.checked_format(fileobj, true)?;
    Err(Error::new("KeyError", format!("{name} is not registered")))
}

impl Selector {
    pub(in crate::dynamic) fn trace(&self, roots: &mut MarkQueue<'_>) {
        roots.extend([self.keys, self.mapping]);
    }

    pub(in crate::dynamic) fn class_name(&self) -> &'static str {
        if self.poll.is_some() {
            "selectors.PollSelector"
        } else {
            "selectors.SelectSelector"
        }
    }
}

/// Resolve Python keys after the native wait releases the selector borrow; int subclasses may compare in Python.
fn select(ctx: &mut Context, handle: &RefCell<Selector>, timeout: Option<Duration>) -> Result<Id> {
    let keys = handle.borrow().keys;
    if handle.borrow().poll.is_none() {
        let mut sets = SelectFds::new();
        for (index, &(fd, entry)) in ctx.dict_entries(keys).unwrap().iter().enumerate() {
            let key = Registration::unpack(ctx, entry);
            let fd = native_fd(ctx, fd)?;
            if key.interests & READ != 0 {
                sets.insert(0, index, fd)?;
            }
            if key.interests & WRITE != 0 {
                sets.insert(1, index, fd)?;
            }
        }
        let mut results = Vec::new();
        if sets.wait(timeout)? {
            for i in 0..ctx.dict_entries(keys).unwrap().len() {
                let (fd, entry) = ctx.dict_entries(keys).unwrap()[i];
                let key = Registration::unpack(ctx, entry);
                let fd = native_fd(ctx, fd)?;
                let events =
                    (i64::from(sets.ready(0, fd)) * READ) | (i64::from(sets.ready(1, fd)) * WRITE);
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
    }
    let events = {
        let mut selector = handle.borrow_mut();
        let (count, ready) = selector.poll.as_mut().unwrap().ready(timeout)?;
        let mut events = Vec::new();
        events
            .try_reserve(count)
            .map_err(|_| Error::new("MemoryError", "out of memory"))?;
        events.extend(ready);
        events
    };
    let mut results = Vec::new();
    results
        .try_reserve(events.len())
        .map_err(|_| Error::new("MemoryError", "out of memory"))?;
    for (fd, flags) in events {
        let fd = ctx.int(i64::from(fd));
        let Some(key) = registration(ctx, keys, fd)? else {
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

fn close(ctx: &mut Context, handle: &RefCell<Selector>) -> Result<Id> {
    let keys = handle.borrow().keys;
    builtins::call(ctx, "dict.clear", &[keys], &[])?;
    handle.borrow_mut().mapping = NONE;
    Ok(NONE)
}

fn mapping_owner(ctx: &mut Context, mapping: Id) -> Result<Rc<RefCell<Selector>>> {
    let owner = ctx.lookup_attr(mapping, "_selector")?;
    if let Value::Selector(handle) = ctx.heap.get(owner) {
        Ok(handle.clone())
    } else {
        Err(Error::new("TypeError", "expected selector"))
    }
}

fn mapping(
    ctx: &mut Context,
    method: &str,
    receiver: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if method == "__init__" {
        let [Some(selector)] = builtins::bind(method, args, kwargs, ["selector"])? else {
            return Err(Error::new("TypeError", "missing selector"));
        };
        ctx.set_attr(receiver, "_selector", selector)?;
        return Ok(NONE);
    }
    let handle = mapping_owner(ctx, receiver)?;
    match method {
        "__len__" | "__iter__" => {
            builtins::bind(method, args, kwargs, [])?;
            let keys = handle.borrow().keys;
            if method == "__iter__" {
                ctx.iterator(keys)
            } else {
                Ok(ctx.int(ctx.dict_entries(keys).unwrap().len() as i64))
            }
        }
        "get" | "__getitem__" => {
            let (obj, default) = if method == "get" {
                let [obj, default] = builtins::bind(method, args, kwargs, ["fileobj", "default"])?;
                (obj, Some(default.unwrap_or(NONE)))
            } else {
                let [obj] = builtins::bind(method, args, kwargs, ["fileobj"])?;
                (obj, None)
            };
            let obj = obj.ok_or_else(|| Error::new("TypeError", "missing file object"))?;
            let fd = reg_fd(ctx, &handle, obj)?;
            // fileno() can replace the mapping's owner.
            let keys = mapping_owner(ctx, receiver)?.borrow().keys;
            registration(ctx, keys, fd)?
                .map(|entry| entry.key)
                .or(default)
                .map_or_else(|| missing(ctx, obj), Ok)
        }
        _ => unreachable!(),
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
        let handle = Rc::new(RefCell::new(Selector {
            poll: (name == "PollSelector").then(Poll::default),
            keys: ctx.dict(Vec::new()),
            mapping: NONE,
        }));
        let selector = ctx.alloc(Value::Selector(handle.clone()));
        ctx.heap.keep(selector);
        let module = ctx.module("selectors")?;
        let class = ctx.lookup_attr(module, "_SelectorMapping")?;
        let view = ctx.call(class, vec![selector], Vec::new())?;
        handle.borrow_mut().mapping = view;
        return Ok(selector);
    }
    let (&receiver, args) = args.split_first().ok_or_else(bad)?;
    if let Some(method) = name.strip_prefix("_SelectorMapping.") {
        return mapping(ctx, method, receiver, args, kwargs);
    }
    let Value::Selector(handle) = ctx.heap.get(receiver).clone() else {
        return Err(bad());
    };
    let keys = handle.borrow().keys;
    match name {
        "get_map" => {
            builtins::bind(name, args, kwargs, [])?;
            Ok(handle.borrow().mapping)
        }
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
                if registration(ctx, keys, fd)?.is_some() {
                    return Err(Error::new(
                        "KeyError",
                        format!(
                            "{} (FD {}) is already registered",
                            ctx.repr(obj),
                            ctx.repr(fd)
                        ),
                    ));
                }
                let module = ctx.module("selectors")?;
                (obj, ctx.lookup_attr(module, "SelectorKey")?, bits)
            } else {
                let key = registration(ctx, keys, fd)?.map_or_else(|| missing(ctx, obj), Ok)?;
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
                    poll.set(native_fd(ctx, fd)?, poll_mask(bits))
                        .map_err(|_| Error::new("MemoryError", "out of memory"))?;
                }
                let unequal = ctx.binary("!=", data, key.data)?;
                if !ctx.truth(unequal)? && !events_changed {
                    return Ok(key.key);
                }
                (key.fileobj, ctx.exact_type(key.key)?, bits)
            };
            let key = ctx.call(class, vec![fileobj, fd, events, data], Vec::new())?;
            let entry = Registration {
                key,
                fileobj,
                interests: bits,
                data,
            }
            .pack(ctx);
            if name == "register" && handle.borrow().poll.is_some() {
                let native = native_fd(ctx, fd)?;
                ctx.set_dictionary_key(keys, fd, entry, None, true)?;
                if handle
                    .borrow_mut()
                    .poll
                    .as_mut()
                    .unwrap()
                    .set(native, poll_mask(bits))
                    .is_err()
                {
                    ctx.delete_item(keys, fd)?;
                    return Err(Error::new("MemoryError", "out of memory"));
                }
            } else {
                ctx.set_dictionary_key(keys, fd, entry, None, true)?;
            }
            Ok(key)
        }
        "get_key" | "unregister" => {
            let [obj] = builtins::bind(name, args, kwargs, ["fileobj"])?;
            let obj = obj.ok_or_else(bad)?;
            if name == "get_key" {
                let view = handle.borrow().mapping;
                if view == NONE {
                    return Err(Error::new("RuntimeError", "Selector is closed"));
                }
                return match ctx.get_item(view, obj) {
                    Err(e) if ctx.is_builtin_error(&e, "KeyError") => missing(ctx, obj),
                    result => result,
                };
            }
            let fd = reg_fd(ctx, &handle, obj)?;
            let Some(key) = registration(ctx, keys, fd)? else {
                return missing(ctx, obj);
            };
            ctx.heap.keep(key.key);
            ctx.delete_item(keys, fd)?;
            if let Some(poll) = &mut handle.borrow_mut().poll {
                poll.remove(native_fd(ctx, fd)?);
            }
            Ok(key.key)
        }
        "select" => {
            let [seconds] = builtins::bind(name, args, kwargs, ["timeout"])?;
            let poll = handle.borrow().poll.is_some();
            let timeout = timeout(ctx, seconds.unwrap_or(NONE), poll)?;
            select(ctx, &handle, timeout)
        }
        "__enter__" | "close" => {
            builtins::bind(name, args, kwargs, [])?;
            if name == "__enter__" {
                return Ok(receiver);
            }
            close(ctx, &handle)
        }
        "__exit__" => {
            let args = builtins::bind(name, args, kwargs, ["exc_type", "exc", "tb"])?;
            if args.iter().any(Option::is_none) {
                return Err(bad());
            }
            close(ctx, &handle)
        }
        _ => Err(bad()),
    }
}
