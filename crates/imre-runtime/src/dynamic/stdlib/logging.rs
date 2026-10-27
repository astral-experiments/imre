//! Adapt boxed Python values to the process-local Logger registry and handler locks.
use std::rc::Rc;
use std::sync::Arc;

use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::builtins::boolean;
use crate::objects::logging::{self, HandlerLock, Logger};

pub(super) fn is_function(name: &str) -> bool {
    matches!(
        name,
        "get"
            | "new"
            | "name"
            | "level"
            | "effective"
            | "parent"
            | "set_level"
            | "propagate"
            | "set_propagate"
            | "disabled"
            | "set_disabled"
            | "enabled"
            | "check_level"
            | "level_name"
            | "lock"
            | "acquire"
            | "release"
    )
}

fn handle(ctx: &Context, id: Id) -> Result<Rc<Logger>> {
    match ctx.heap.get(id) {
        Value::LoggerHandle(logger) => Ok(Rc::clone(logger)),
        _ => Err(Error::new("TypeError", "expected a native logger handle")),
    }
}

fn wrapper(ctx: &mut Context, logger: Rc<Logger>) -> Result<Id> {
    let key = Rc::as_ptr(&logger);
    if let Some(&id) = ctx.logger_wrappers.get(&key) {
        return Ok(id);
    }
    let module = ctx.module("logging")?;
    let class = ctx.lookup_attr(module, "Logger")?;
    let native = ctx.alloc(Value::LoggerHandle(logger));
    let id = ctx.call(class, vec![native], Vec::new())?;
    ctx.logger_wrappers.insert(key, id);
    Ok(id)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "native logging operations take positional arguments",
        ));
    }
    match (name, args) {
        ("get", &[name]) => {
            let name = if name == NONE {
                ""
            } else {
                ctx.str_value(name)?
            };
            wrapper(ctx, Logger::registered(name))
        }
        // A registered Logger uses the same Python initializer; retain its existing
        // registry handle instead of constructing an independent logger.
        ("new", &[id, _]) if matches!(ctx.heap.get(id), Value::LoggerHandle(_)) => Ok(id),
        ("new", &[name, level]) => {
            let logger = Logger::standalone(ctx.str_value(name)?, ctx.as_int(level)?);
            Ok(ctx.alloc(Value::LoggerHandle(logger)))
        }
        ("parent", &[id]) => handle(ctx, id)?
            .parent()
            .map_or(Ok(NONE), |p| wrapper(ctx, p)),
        ("name", &[id]) => Ok(ctx.text(handle(ctx, id)?.name())),
        ("level", &[id]) => Ok(ctx.alloc(Value::Int(handle(ctx, id)?.level()))),
        ("effective", &[id]) => Ok(ctx.alloc(Value::Int(handle(ctx, id)?.effective_level()))),
        ("set_level", &[id, level]) => {
            handle(ctx, id)?.set_level(&ctx.as_int(level)?);
            Ok(NONE)
        }
        ("propagate", &[id]) => Ok(boolean(handle(ctx, id)?.propagate())),
        ("disabled", &[id]) => Ok(boolean(handle(ctx, id)?.disabled())),
        ("set_propagate", &[id, value]) => {
            handle(ctx, id)?.set_propagate(ctx.truth(value)?);
            Ok(NONE)
        }
        ("set_disabled", &[id, value]) => {
            handle(ctx, id)?.set_disabled(ctx.truth(value)?);
            Ok(NONE)
        }
        ("enabled", &[id, level]) => {
            let logger = handle(ctx, id)?;
            Ok(boolean(
                !logger.disabled() && logger.enabled_at(&ctx.as_int(level)?)?,
            ))
        }
        ("check_level", &[level]) => {
            let value = match ctx.heap.get(level) {
                Value::Str(name) => logging::named_level(&crate::Str::from(name.as_str()))?,
                Value::Int(_) | Value::Bool(_) => return Ok(level),
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "Level not an integer or a valid string: {}",
                            ctx.repr(level)
                        ),
                    ));
                }
            };
            Ok(ctx.alloc(Value::Int(value)))
        }
        ("level_name", &[level]) => Ok(ctx.text(logging::level_name(&ctx.as_int(level)?).as_str())),
        ("lock", &[]) => Ok(ctx.alloc(Value::HandlerLock(Arc::new(HandlerLock::default())))),
        ("acquire" | "release", &[lock]) => {
            let Value::HandlerLock(lock) = ctx.heap.get(lock) else {
                return Err(Error::new("TypeError", "expected a logging lock"));
            };
            if name == "acquire" {
                Ok(boolean(lock.acquire(true, None)?))
            } else {
                lock.release()?;
                Ok(NONE)
            }
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}
