//! Public _thread.RLock backed by the reentrant lock shared with logging handlers.
use super::builtins::boolean;
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::logging::HandlerLock;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::sync::Arc;

pub(super) const CLASS: &str = "_thread.RLock";

pub(super) fn is_method(name: &str) -> bool {
    (cfg!(unix) && name == "_at_fork_reinit")
        || matches!(
            name,
            "acquire"
                | "release"
                | "locked"
                | "__enter__"
                | "__exit__"
                | "_recursion_count"
                | "_is_owned"
                | "_release_save"
                | "_acquire_restore"
        )
}

fn saved_state(ctx: &mut Context, arg: Id) -> Result<(usize, usize)> {
    let items = if let Some(items) = ctx.tuple_items(arg)? {
        items
    } else {
        let sequence = match ctx.heap.get(ctx.scalar_value(arg)) {
            Value::List(_) | Value::Range(_) | Value::Deque(_) | Value::Array(_) => true,
            Value::Instance { class, .. } => ctx.class_attr(*class, "__getitem__").is_some(),
            _ => false,
        };
        if !sequence {
            let kind = if arg == NONE {
                "None"
            } else {
                ctx.type_name(arg)
            };
            return Err(Error::new(
                "TypeError",
                format!("_acquire_restore() argument 1 must be 2-item tuple, not {kind}"),
            ));
        }
        let len = super::builtins::call(ctx, "len", &[arg], &[])?;
        let len = ctx.as_integer(len)?;
        if len != 2 {
            return Err(Error::new(
                "TypeError",
                format!("_acquire_restore() argument 1 must be sequence of length 2, not {len}"),
            ));
        }
        ctx.items(arg)?
    };
    let [count, owner] = items.as_slice() else {
        return Err(Error::new(
            "TypeError",
            format!(
                "_acquire_restore() argument 1 must be tuple of length 2, not {}",
                items.len()
            ),
        ));
    };
    let (count, owner) = (*count, *owner);
    let count = ctx.index_integer(count)?.big().to_isize().ok_or_else(|| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })?;
    let number = ctx.index_value(owner)?.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "_acquire_restore() argument 1, item 1 must be int, not {}",
                if owner == NONE {
                    "None"
                } else {
                    ctx.type_name(owner)
                }
            ),
        )
    })?;
    let owner = (number.big().as_ref() & BigInt::from(usize::MAX))
        .to_usize()
        .unwrap();
    Ok((count as usize, owner))
}

fn receiver_args<'a>(ctx: &Context, name: &str, args: &'a [Id]) -> Result<(Id, &'a [Id])> {
    let Some((&id, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method RLock.{name}() needs an argument"),
        ));
    };
    let receiver = ctx.scalar_value(id);
    if !matches!(ctx.heap.get(receiver), Value::HandlerLock(_)) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '_thread.RLock' objects doesn't apply to a '{}' object",
                ctx.type_name(id)
            ),
        ));
    }
    Ok((receiver, args))
}

// Conversion may run Python and reinitialize this receiver in a forked child.
fn backing(ctx: &Context, receiver: Id) -> Arc<HandlerLock> {
    match ctx.heap.get(receiver) {
        Value::HandlerLock(lock) => Arc::clone(lock),
        _ => unreachable!("RLock receiver checked above"),
    }
}

/// The native condition wait restores the saved owner and depth on the selected lock.
pub(super) struct Restore {
    lock: Arc<HandlerLock>,
    depth: usize,
    owner: usize,
}

impl Restore {
    pub(super) fn prepare(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Self> {
        let (receiver, args) = receiver_args(ctx, "_acquire_restore", args)?;
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "_acquire_restore() takes no keyword arguments",
            ));
        }
        let [arg] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_acquire_restore() takes exactly 1 argument ({} given)",
                    args.len()
                ),
            ));
        };
        let (depth, owner) = saved_state(ctx, *arg)?;
        Ok(Self {
            lock: backing(ctx, receiver),
            depth,
            owner,
        })
    }

    pub(super) fn run(self) -> Id {
        self.lock.acquire_restore(self.depth, self.owner);
        NONE
    }
}

pub(super) fn prepare_acquire(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<super::thread_lock::Acquire> {
    let (receiver, args) = receiver_args(ctx, name, args)?;
    super::thread_lock::Acquire::with_receiver(ctx, receiver, args, kwargs)
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "_acquire_restore" {
        return Ok(Restore::prepare(ctx, args, kwargs)?.run());
    }
    if matches!(name, "acquire" | "__enter__") {
        let wait = prepare_acquire(ctx, name, args, kwargs)?;
        // SAFETY: conversion returned to the owner of the interrupted frame.
        return unsafe { wait.run_registered(ctx) };
    }
    let (receiver, args) = receiver_args(ctx, name, args)?;
    if !kwargs.is_empty() {
        let label = if name == "__exit__" {
            name.to_owned()
        } else {
            format!("RLock.{name}")
        };
        return Err(Error::new(
            "TypeError",
            format!("{label}() takes no keyword arguments"),
        ));
    }
    if !args.is_empty() && name != "__exit__" {
        return Err(Error::new(
            "TypeError",
            format!("RLock.{name}() takes no arguments ({} given)", args.len()),
        ));
    }
    #[cfg(unix)]
    if name == "_at_fork_reinit" {
        let Value::HandlerLock(lock) = ctx.heap.get_mut(receiver) else {
            unreachable!("RLock receiver checked above")
        };
        crate::objects::logging::HandlerLock::at_fork_reinit(lock);
        return Ok(NONE);
    }
    let lock = backing(ctx, receiver);
    match name {
        "locked" => Ok(boolean(lock.state().is_some())),
        "_is_owned" => Ok(boolean(lock.owned())),
        "_recursion_count" => Ok(ctx.alloc(Value::Int(crate::Int::from_usize(lock.depth())))),
        "_release_save" => {
            let (depth, owner) = lock.release_save()?;
            let depth = ctx.int(depth as isize as i64);
            let owner = ctx.alloc(Value::Int(crate::Int::from_usize(owner)));
            Ok(ctx.alloc(Value::Tuple(vec![depth, owner])))
        }
        "release" | "__exit__" => {
            lock.release()?;
            Ok(NONE)
        }
        _ => unreachable!("registered RLock method"),
    }
}
