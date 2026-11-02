//! Launch the exact builtin execv when neither argument conversion nor auditing can run Python.

use std::sync::atomic::Ordering;

use super::{Context, Id, NONE, Result, Value, c_path, spawn_child};
use crate::dynamic::copy_ids;
use crate::objects::CStrings;
use crate::objects::exceptions::Kind;

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

/// CPython issues this advisory warning in the parent, even if the fork failed.
fn warn_about_threads(ctx: &mut Context) {
    if spawn_child::glibc_proves_single_threaded() {
        return;
    }
    // Linux knows about native workers outside this interpreter's thread registry.
    let multiple = std::fs::read_dir("/proc/self/task")
        .map(|tasks| tasks.filter_map(std::result::Result::ok).nth(1).is_some())
        .unwrap_or_else(|_| ctx.threads.count.load(Ordering::Relaxed) > 0);
    if multiple {
        let message = format!(
            "This process (pid={}) is multi-threaded, use of fork() may lead to deadlocks in the child.",
            std::process::id()
        );
        let _ = ctx.emit_builtin_warning(Kind::DeprecationWarning, &message, 1);
    }
}

/// Return a real child PID, or decline before forking if the child needs the interpreter.
pub(super) fn try_spawn(
    ctx: &mut Context,
    path: Id,
    argv: Id,
    exec: Id,
) -> Result<Option<libc::pid_t>> {
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
        return Ok(None);
    }
    let (Value::List(items) | Value::Tuple(items)) = ctx.heap.get(argv) else {
        return Ok(None);
    };
    if !items.iter().copied().all(plain_path) {
        return Ok(None);
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
    let pid = CStrings::fork_execv(command);
    warn_about_threads(ctx);
    Ok(Some(pid?))
}
