//! Boxed access to the primitive shared by threading.Lock and _thread.LockType.
use super::builtins::{bind_method_arguments, boolean};
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::locks::{PrimitiveLock, WaitPhase, timeout_float, timeout_int};
use crate::objects::logging::HandlerLock;
use crate::objects::time::Deadline;
use std::sync::Arc;
use std::time::Duration;

pub(super) const CLASS: &str = "_thread.lock";

pub(super) fn is_method(name: &str) -> bool {
    (cfg!(unix) && name == "_at_fork_reinit")
        || matches!(
            name,
            "acquire" | "release" | "locked" | "__enter__" | "__exit__"
        )
}

pub(super) fn construct(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        let label = if name == CLASS { "lock" } else { name };
        return Err(Error::new(
            "TypeError",
            format!("{label}() takes no keyword arguments"),
        ));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            if name == CLASS {
                format!("lock expected 0 arguments, got {}", args.len())
            } else {
                format!("{name}() takes no arguments ({} given)", args.len())
            },
        ));
    }
    Ok(ctx.alloc(Value::PrimitiveLock(Default::default())))
}

fn acquire_args(
    ctx: &mut Context,
    blocking: Option<Id>,
    timeout: Option<Id>,
) -> Result<(bool, Option<std::time::Duration>)> {
    let blocking = ctx.optional_truth(blocking, true)?;
    let timeout = match timeout {
        None => None,
        Some(value) => match *ctx.heap.get(ctx.scalar_value(value)) {
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
    Ok((blocking, timeout))
}

fn receiver_args<'a, 'b>(
    ctx: &'a Context,
    name: &str,
    args: &'b [Id],
) -> Result<(Id, &'a Arc<PrimitiveLock>, &'b [Id])> {
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
    Ok((id, lock, args))
}

/// The physical receiver selected by the registered call, together with its wait options.
pub(super) struct Acquire {
    receiver: Id,
    blocking: bool,
    timeout: Option<Duration>,
}

pub(super) enum Lock {
    Primitive(Arc<PrimitiveLock>),
    Reentrant(Arc<HandlerLock>),
}

pub(super) enum NativeWait<'a> {
    Acquire(Option<Duration>, Id, &'a mut Lock),
    Join(Option<Deadline>, &'a super::thread_worker::Handle),
}

impl NativeWait<'_> {
    fn wait(&self, limit: Option<Duration>, phase: WaitPhase) -> bool {
        match self {
            Self::Acquire(_, _, lock) => lock.acquire(true, limit, phase),
            Self::Join(_, handle) => handle.wait_for_exit(limit),
        }
    }

    /// Park the calling execution for a native wait and resume it before handling the result.
    ///
    /// # Safety
    /// The caller owns the context and the selected, rooted wait operands.
    unsafe fn wait_parked(
        &self,
        context: *mut Context,
        limit: Option<Duration>,
        phase: WaitPhase,
    ) -> Result<bool> {
        if limit == Some(Duration::ZERO) {
            return Ok(self.wait(limit, phase));
        }
        // SAFETY: the native wait uses its lock or join handle and never borrows the heap.
        unsafe { super::value::ParkedExecution::wait(context, || self.wait(limit, phase)) }
    }

    /// # Safety
    /// The caller must resume in its own context. An acquire receiver must be live at entry.
    pub(super) unsafe fn run(mut self, _context: *mut Context) -> Result<bool> {
        #[cfg(unix)]
        if crate::signals::is_main() {
            let (deadline, root) = match self {
                Self::Acquire(timeout, receiver, _) => {
                    // SAFETY: the caller owns the active frame at entry. Only the root's
                    // index and value cross the native wait.
                    let ctx = unsafe { &mut *_context };
                    ctx.heap.reserve_temporaries(1)?;
                    let mark = ctx.heap.temporary_mark();
                    ctx.heap.keep(receiver);
                    (timeout.map(Deadline::after), Some((mark, receiver)))
                }
                Self::Join(deadline, _) => (deadline, None),
            };
            let interval = Duration::from_millis(50);
            let mut phase = WaitPhase::Initial;
            let result = loop {
                let remaining = deadline.map(Deadline::remaining);
                let slice = remaining.map_or(interval, |left| left.min(interval));
                // SAFETY: every outcome restores the interrupted frame, including on timeout.
                match unsafe { self.wait_parked(_context, Some(slice), phase) } {
                    Ok(true) => break Ok(true),
                    Err(error) => break Err(error),
                    Ok(false) => {}
                }
                // An expired join returns immediately. A lock wait interrupted during
                // a positive slice dispatches first, then probes the lock once more.
                if matches!(self, Self::Join(..))
                    && deadline.is_some_and(|end| end.remaining().is_zero())
                {
                    break Ok(false);
                }
                if !slice.is_zero()
                    && crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire)
                {
                    // SAFETY: the native wait released its mutex and returned to the
                    // interrupted thread. Python reenters its active context here.
                    if let Err(error) = unsafe { Context::dispatch_pending_calls(_context) } {
                        break Err(error);
                    }
                    if let Self::Acquire(_, receiver, lock) = &mut self {
                        // A returning signal handler may have reset the lock in a forked child.
                        // SAFETY: dispatch returned to the rooted receiver's frame. Clone
                        // its current backing before the next native wait.
                        **lock = Lock::backing(unsafe { &*_context }, *receiver);
                    }
                    phase = WaitPhase::Interrupted;
                    continue;
                }
                if matches!(self, Self::Acquire(..)) && remaining == Some(slice) {
                    break Ok(false);
                }
                phase = WaitPhase::Parked;
            };
            if let Some((mark, receiver)) = root {
                // SAFETY: every exit resumes the same frame after any handler returned.
                unsafe { &mut *_context }
                    .heap
                    .release_weak_target(mark, receiver);
            }
            return result;
        }
        let limit = match &self {
            Self::Acquire(timeout, ..) => *timeout,
            Self::Join(deadline, _) => deadline.map(Deadline::remaining),
        };
        // SAFETY: the worker owns its execution until the native operation returns.
        unsafe { self.wait_parked(_context, limit, WaitPhase::Initial) }
    }
}

impl Lock {
    fn backing(ctx: &Context, receiver: Id) -> Self {
        match ctx.heap.get(receiver) {
            Value::PrimitiveLock(lock) => Self::Primitive(Arc::clone(lock)),
            Value::HandlerLock(lock) => Self::Reentrant(Arc::clone(lock)),
            _ => unreachable!("lock receiver checked before conversion"),
        }
    }

    fn acquire(&self, blocking: bool, timeout: Option<Duration>, phase: WaitPhase) -> bool {
        match self {
            Self::Primitive(lock) => lock.acquire(blocking, timeout),
            Self::Reentrant(lock) => lock.acquire_slice(blocking, timeout, phase),
        }
    }
}

impl Acquire {
    pub(super) fn prepare(
        ctx: &mut Context,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Self> {
        let (receiver, _, args) = receiver_args(ctx, name, args)?;
        Self::with_receiver(ctx, receiver, args, kwargs)
    }

    pub(super) fn with_receiver(
        ctx: &mut Context,
        receiver: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Self> {
        let (blocking, timeout) = if args.is_empty() && kwargs.is_empty() {
            (true, None)
        } else {
            let [blocking, timeout] =
                bind_method_arguments("acquire", args, kwargs, ["blocking", "timeout"], 2)?;
            // Both conversions can run Python and remove the binding we just selected.
            ctx.with_weak_target(receiver, |ctx| acquire_args(ctx, blocking, timeout))?
        };
        Ok(Self {
            receiver,
            blocking,
            timeout,
        })
    }

    /// # Safety
    /// Run in the preparing thread and context before any intervening Python call or
    /// collection, while the selected receiver is still live.
    pub(super) unsafe fn run_registered(self, context: *mut Context) -> Result<Id> {
        // SAFETY: conversion has finished. Only the native backing crosses a blocking wait.
        let mut lock = Lock::backing(unsafe { &*context }, self.receiver);
        if !self.blocking {
            return Ok(boolean(lock.acquire(
                self.blocking,
                self.timeout,
                WaitPhase::Initial,
            )));
        }
        // SAFETY: the receiver is live at entry. The interruptible wait keeps it rooted
        // through signal handlers that can replace its backing.
        unsafe { NativeWait::Acquire(self.timeout, self.receiver, &mut lock).run(context) }
            .map(boolean)
    }
}

fn check_method_args(name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<()> {
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
    Ok(())
}

/// Keep only the native waiter across notification. Test its state when releasing it.
pub(super) fn prepare_release(
    ctx: &Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Arc<PrimitiveLock>> {
    let (_, lock, args) = receiver_args(ctx, "release", args)?;
    check_method_args("release", args, kwargs)?;
    Ok(Arc::clone(lock))
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(name, "acquire" | "__enter__") {
        let wait = Acquire::prepare(ctx, name, args, kwargs)?;
        // SAFETY: argument conversion returned to this exclusively executing caller.
        return unsafe { wait.run_registered(ctx) };
    }
    let (id, lock, args) = receiver_args(ctx, name, args)?;
    check_method_args(name, args, kwargs)?;
    match name {
        #[cfg(unix)]
        "_at_fork_reinit" => {
            let Value::PrimitiveLock(lock) = ctx.heap.get_mut(id) else {
                unreachable!("lock receiver checked above");
            };
            crate::objects::locks::PrimitiveLock::at_fork_reinit(lock);
            Ok(NONE)
        }
        "locked" => Ok(boolean(lock.locked())),
        "release" | "__exit__" => {
            lock.release()?;
            Ok(NONE)
        }
        _ => unreachable!("registered lock method"),
    }
}
