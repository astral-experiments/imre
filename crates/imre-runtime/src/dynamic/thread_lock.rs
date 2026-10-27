//! Boxed access to the primitive shared by threading.Lock and _thread.LockType.
use super::builtins::{bind_method_arguments, boolean};
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::locks::{timeout_float, timeout_int};

pub(super) const CLASS: &str = "_thread.lock";

pub(super) fn is_method(name: &str) -> bool {
    matches!(
        name,
        "acquire" | "release" | "locked" | "__enter__" | "__exit__"
    )
}

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new("TypeError", "lock() takes no keyword arguments"));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("lock expected 0 arguments, got {}", args.len()),
        ));
    }
    Ok(ctx.alloc(Value::PrimitiveLock(Default::default())))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&id, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method lock.{name}() needs an argument"),
        ));
    };
    let Value::PrimitiveLock(lock) = ctx.heap.get(id) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '_thread.lock' objects doesn't apply to a '{}' object",
                ctx.type_name(id)
            ),
        ));
    };
    let lock = lock.clone();
    if matches!(name, "acquire" | "__enter__") {
        let [blocking, timeout] =
            bind_method_arguments("acquire", args, kwargs, ["blocking", "timeout"], 2)?;
        let blocking = ctx.optional_truth(blocking, true)?;
        let timeout = match timeout {
            None => None,
            Some(value) => match *ctx.heap.get(value) {
                Value::Float(seconds) => timeout_float(blocking, seconds)?,
                _ => {
                    let seconds = ctx.index_value(value)?.ok_or_else(|| {
                        Error::new(
                            "TypeError",
                            format!(
                                "'{}' object cannot be interpreted as an integer or float",
                                ctx.type_name(value)
                            ),
                        )
                    })?;
                    timeout_int(blocking, &seconds)?
                }
            },
        };
        return Ok(boolean(lock.acquire(blocking, timeout)));
    }
    if !kwargs.is_empty() {
        let label = if name == "__exit__" {
            name.to_owned()
        } else {
            format!("lock.{name}")
        };
        return Err(Error::new(
            "TypeError",
            format!("{label}() takes no keyword arguments"),
        ));
    }
    if name != "__exit__" && !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("lock.{name}() takes no arguments ({} given)", args.len()),
        ));
    }
    match name {
        "locked" => Ok(boolean(lock.locked())),
        "release" | "__exit__" => {
            lock.release()?;
            Ok(NONE)
        }
        _ => unreachable!("registered lock method"),
    }
}
