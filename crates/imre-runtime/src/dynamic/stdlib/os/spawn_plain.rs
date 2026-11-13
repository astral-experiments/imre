//! Launch the exact builtin execv when neither argument conversion nor auditing can run Python.

use std::sync::atomic::Ordering;

use super::{Context, Id, NONE, Value, c_path, spawn_child};
use crate::dynamic::copy_ids;
use crate::objects::CStrings;
use crate::objects::exceptions::{Kind, Result};

/// The wrapper resolves _exit in its own globals only if the child's exec raises.
fn has_builtin_exit(ctx: &Context) -> bool {
    let scope = ctx.heap.current_scope();
    let Some(&builtin) = ctx.builtins.get("posix._exit") else {
        return false;
    };
    if scope == NONE {
        return false;
    }
    let globals = ctx.scope_data(ctx.root_scope(scope));
    let exit = match globals.exec_globals {
        Some(source) => ctx.dict_named_item(source.dict, "_exit"),
        None => ctx.namespace_binding(globals, "_exit"),
    };
    exit == Some(builtin)
}

/// Observe the parent thread count before its after-fork handlers can change it.
pub(super) fn multiple_threads(ctx: &Context) -> bool {
    !spawn_child::glibc_proves_single_threaded()
        && std::fs::read_dir("/proc/self/task")
            .map(|tasks| tasks.filter_map(std::result::Result::ok).nth(1).is_some())
            .unwrap_or_else(|_| ctx.interpreter.worker_count.load(Ordering::Relaxed) > 0)
}

/// A warning hook can run Python, so emit only after the parent handlers release their locks.
pub(super) fn warn_about_threads(ctx: &mut Context) {
    let message = format!(
        "This process (pid={}) is multi-threaded, use of fork() may lead to deadlocks in the child.",
        std::process::id()
    );
    let _ = ctx.emit_builtin_warning(Kind::DeprecationWarning, &message, 1);
}

/// Return a real child PID, or decline before forking if the child needs the interpreter.
pub(super) fn try_spawn(
    ctx: &mut Context,
    path: Id,
    argv: Id,
    exec: Id,
) -> Option<Result<libc::pid_t>> {
    let plain_path = |id| {
        matches!(
            ctx.heap.get(id),
            Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
        )
    };
    if !ctx.audit_hooks.is_empty()
        || ctx.builtins.get("posix.execv") != Some(&exec)
        || !has_builtin_exit(ctx)
        || !plain_path(path)
    {
        return None;
    }
    let (Value::List(items) | Value::Tuple(items)) = ctx.heap.get(argv) else {
        return None;
    };
    if !items.iter().copied().all(plain_path) {
        return None;
    }

    // _spawnvef catches execv's conversion errors in the child. An invalid image
    // still needs a waitable child, even when no call to execv is possible.
    let image = copy_ids(items).ok().and_then(|items| {
        let path = c_path(ctx, path, "embedded null byte").ok()?;
        let mut args = Vec::new();
        args.try_reserve_exact(items.len()).ok()?;
        for item in items {
            args.push(c_path(ctx, item, "embedded null byte").ok()?);
        }
        if args.first()?.as_bytes().is_empty() {
            return None;
        }
        Some((path, CStrings::new(args).ok()?))
    });
    let command = image.as_ref().map(|(path, args)| (path.as_c_str(), args));
    Some(CStrings::fork_execv(command))
}
