//! Report uncaught exceptions through the interpreter's current sys.excepthook.
use std::io::Write as _;

use super::stdlib::{audit, traceback};
use super::{Context, Error, Id, NONE, Result, Value, ffi, unraisable};

pub(super) fn default_hook(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "sys.excepthook() takes no keyword arguments",
        ));
    }
    let &[_, mut value, tb] = args else {
        return Err(Error::new(
            "TypeError",
            format!("excepthook expected 3 arguments, got {}", args.len()),
        ));
    };
    match ctx.sys_binding("stderr")? {
        Some(NONE) => return Ok(NONE),
        None => {
            traceback::display(ctx, value);
            return Ok(NONE);
        }
        Some(_) => {}
    }
    if matches!(ctx.heap.get(value), Value::Raised(_)) {
        if matches!(ctx.heap.get(tb), Value::Traceback { .. })
            && ctx.exception_traceback(value)? == NONE
        {
            ctx.set_traceback(value, tb)?;
        }
    } else if value != NONE {
        let message = format!(
            "print_exception(): Exception expected for value, {} found",
            ctx.type_name(value)
        );
        value = ctx.raised_instance(Error::new("TypeError", message));
    }
    traceback::display(ctx, value);
    Ok(NONE)
}

pub(super) fn write_stderr(ctx: &mut Context, message: &str) {
    let mark = ctx.heap.temporary_mark();
    let file = ctx.sys_binding("stderr").ok().flatten().unwrap_or(NONE);
    ctx.heap.keep(file);
    let before_writer = ctx.heap.temporary_mark();
    let written = ctx.lookup_attr(file, "write").and_then(|writer| {
        ctx.heap.keep(writer);
        let text = ctx.text(message);
        ctx.call(writer, vec![text], Vec::new())
    });
    let failed = ctx.release_with_result(before_writer, written).is_err();
    ctx.discard_temporaries(before_writer, None);
    if failed {
        let _ = std::io::stderr().write_all(message.as_bytes());
    }
    ctx.discard_temporaries(mark, None);
}

/// Publish the original exception before the audit event or hook can run Python.
pub(super) fn report(ctx: &mut Context, sys: Id, error: Error, interrupted: &mut bool) -> i32 {
    let value = ctx.raised_instance(error);
    ctx.heap.keep(value);
    let kind = ctx.exact_type(value).expect("raised exception has a class");
    let tb = ctx
        .exception_traceback(value)
        .expect("raised exception has a traceback field");
    ctx.heap.keep(tb);
    let Value::Module { scope, .. } = *ctx.heap.get(sys) else {
        unreachable!("interpreter sys module")
    };
    for (name, id) in [
        ("last_exc", value),
        ("last_type", kind),
        ("last_value", value),
        ("last_traceback", tb),
    ] {
        if let Some(dict) = ctx.scope_data(scope).namespace {
            // A colliding Python key can raise. CPython still publishes the other fields.
            let key = ctx.heap.code_name(name);
            let _ = ctx.put_item(dict, key, id);
        } else {
            ctx.bind(scope, name, id);
        }
    }
    let hook = if let Some(dict) = ctx.scope_data(scope).namespace {
        let key = ctx.heap.code_name("excepthook");
        ctx.get_item(dict, key).ok()
    } else {
        ctx.scope_name(scope, "excepthook")
    };
    if let Some(hook) = hook {
        ctx.heap.keep(hook);
    }
    if let Err(error) = audit::event(
        ctx,
        "sys.excepthook",
        &[hook.unwrap_or(NONE), kind, value, tb],
    ) {
        if ctx.is_builtin_error(&error, "RuntimeError") {
            return 1;
        }
        unraisable::report(ctx, error, false, None, "Exception ignored in audit hook");
    }
    let Some(hook) = hook else {
        write_stderr(ctx, "sys.excepthook is missing\n");
        traceback::display(ctx, value);
        return 1;
    };
    if let Err(error) = ctx.call(hook, vec![kind, value, tb], Vec::new()) {
        if ctx.is_builtin_error(&error, "SystemExit") {
            *interrupted = false;
            return ffi::report_system_exit(ctx, error);
        }
        *interrupted |= ctx.is_builtin_error(&error, "KeyboardInterrupt");
        let secondary = ctx.raised_instance(error);
        ctx.heap.keep(secondary);
        let _ = crate::flush_stdout();
        write_stderr(ctx, "Error in sys.excepthook:\n");
        traceback::display(ctx, secondary);
        write_stderr(ctx, "\nOriginal exception was:\n");
        traceback::display(ctx, value);
    }
    1
}
