//! Fork callbacks are rooted in the interpreter heap that owns their identities.

use super::{Context, Error, Id, NONE, Result, unraisable};

const NAMES: [&str; 3] = ["before", "after_in_child", "after_in_parent"];

#[derive(Copy, Clone)]
pub(super) enum Phase {
    Before,
    Child,
    Parent,
}

/// Validate all callbacks, in signature order, before installing any of them.
pub(super) fn register(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            "register_at_fork() takes no positional arguments",
        ));
    }
    if kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "At least one argument is required.",
        ));
    }
    let values =
        super::builtins::bind_builtin_arguments("register_at_fork", args, kwargs, NAMES, 0)?;
    for (name, value) in NAMES.into_iter().zip(values) {
        if let Some(callback) = value
            && !ctx.callable(callback)
        {
            return Err(Error::new(
                "TypeError",
                format!("'{name}' must be callable, not {}", ctx.type_name(callback)),
            ));
        }
    }
    // Workers have fresh heaps. A worker-local Id cannot become a root in the parent.
    if ctx.threads.private_context {
        return Err(Error::new(
            "NotImplementedError",
            "register_at_fork cannot share callbacks from a private worker heap",
        ));
    }
    for (list, value) in ctx.fork_hooks.iter_mut().zip(values) {
        if value.is_some() {
            list.try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
        }
    }
    for (list, value) in ctx.fork_hooks.iter_mut().zip(values) {
        list.extend(value);
    }
    Ok(NONE)
}

fn run(ctx: &mut Context, phase: Phase) {
    // Registration only appends; capturing the length takes a snapshot without
    // retaining a borrow across Python calls. A later phase takes its own snapshot.
    let slot = phase as usize;
    let len = ctx.fork_hooks[slot].len();
    for n in 0..len {
        let i = if matches!(phase, Phase::Before) {
            len - 1 - n
        } else {
            n
        };
        let callback = ctx.fork_hooks[slot][i];
        let mark = ctx.heap.temporary_mark();
        let result = ctx.call(callback, Vec::new(), Vec::new());
        let release_result = ctx.heap.observes_released_roots()
            && matches!(result, Ok(value) if ctx.heap.could_own_finalizer(value, false));
        if let Err(error) = result {
            unraisable::report(
                ctx,
                error,
                false,
                Some(callback),
                "Exception ignored in atfork callback",
            );
        }
        ctx.heap.release_temporaries_since(mark);
        if release_result {
            ctx.collect_unreachable();
        }
    }
}

/// A native syscall error contains no Python heap references. Carry it across the
/// after-handlers before converting it into a Python exception at the call site.
pub(super) fn around(
    ctx: &mut Context,
    operation: impl FnOnce(&mut Context) -> crate::objects::exceptions::Result<libc::pid_t>,
) -> crate::objects::exceptions::Result<libc::pid_t> {
    run(ctx, Phase::Before);
    let result = operation(ctx);
    run(
        ctx,
        if matches!(result, Ok(0)) {
            Phase::Child
        } else {
            Phase::Parent
        },
    );
    result
}
