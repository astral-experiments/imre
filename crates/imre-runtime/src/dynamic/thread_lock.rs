//! Boxed access to the primitive shared by threading.Lock and _thread.LockType.
use super::builtins::{bind_method_arguments, boolean};
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::locks::{PrimitiveLock, timeout_float, timeout_int};
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

pub(super) fn acquire_args(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<(bool, Option<std::time::Duration>)> {
    let [blocking, timeout] =
        bind_method_arguments("acquire", args, kwargs, ["blocking", "timeout"], 2)?;
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

/// The native wait owns its receiver after Python argument conversion has finished.
pub(super) struct Acquire {
    lock: Arc<PrimitiveLock>,
    blocking: bool,
    timeout: Option<Duration>,
}

pub(super) enum NativeWait {
    Acquire(Option<Duration>),
    Join(Option<Deadline>),
}

impl NativeWait {
    /// # Safety
    /// The caller owns the interrupted context and frame. The native wait must release
    /// its native locks before returning and must hold no references to the Python heap.
    pub(super) unsafe fn run(
        self,
        _context: *mut Context,
        wait: impl Fn(Option<Duration>) -> bool,
    ) -> Result<bool> {
        #[cfg(unix)]
        if crate::signals::is_main() {
            let deadline = match self {
                Self::Acquire(timeout) => timeout.map(Deadline::after),
                Self::Join(deadline) => deadline,
            };
            let interval = Duration::from_millis(50);
            loop {
                let remaining = deadline.map(Deadline::remaining);
                let slice = remaining.map_or(interval, |left| left.min(interval));
                if wait(Some(slice)) {
                    return Ok(true);
                }
                // An expired join returns immediately. A lock wait interrupted during
                // a positive slice dispatches first, then probes the lock once more.
                if matches!(self, Self::Join(_))
                    && deadline.is_some_and(|end| end.remaining().is_zero())
                {
                    return Ok(false);
                }
                if !slice.is_zero()
                    && crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire)
                {
                    // SAFETY: the wait released its native locks before returning to this frame.
                    unsafe { Context::dispatch_pending_calls(_context)? };
                    continue;
                }
                if matches!(self, Self::Acquire(_)) && remaining == Some(slice) {
                    return Ok(false);
                }
            }
        }
        Ok(wait(match self {
            Self::Acquire(timeout) => timeout,
            Self::Join(deadline) => deadline.map(Deadline::remaining),
        }))
    }
}

impl Acquire {
    /// An already selected lock.acquire() with no Python arguments.
    pub(super) fn blocking(lock: &Arc<PrimitiveLock>) -> Self {
        Self {
            lock: Arc::clone(lock),
            blocking: true,
            timeout: None,
        }
    }

    pub(super) fn prepare(
        ctx: &mut Context,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Self> {
        let (id, lock, args) = receiver_args(ctx, name, args)?;
        if args.is_empty() && kwargs.is_empty() {
            return Ok(Self::blocking(lock));
        }
        let (blocking, timeout) = acquire_args(ctx, args, kwargs)?;
        // Conversion may replace this lock's native backing in a forked child.
        let Value::PrimitiveLock(lock) = ctx.heap.get(id) else {
            unreachable!("lock receiver checked above");
        };
        Ok(Self {
            lock: Arc::clone(lock),
            blocking,
            timeout,
        })
    }

    pub(super) fn run(self) -> Id {
        boolean(self.lock.acquire(self.blocking, self.timeout))
    }

    /// # Safety
    /// The interrupted caller and its Python frame belong to this thread throughout the wait.
    pub(super) unsafe fn run_registered(self, _context: *mut Context) -> Result<Id> {
        if !self.blocking {
            return Ok(self.run());
        }
        // SAFETY: the caller owns its frame and the lock releases its mutex after each wait.
        unsafe {
            NativeWait::Acquire(self.timeout).run(_context, |limit| self.lock.acquire(true, limit))
        }
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
