//! Deliver errors raised by Python deallocators to the interpreter's current sys hook.
use super::stdlib::{audit, fixed_structseq, traceback};
use super::{Context, Error, Id, NONE, Result, Value};

/// Preserve the original exception and traceback before formatting or calling Python.
fn exception_fields(ctx: &mut Context, error: Error, supply_frame: bool) -> Result<[Id; 3]> {
    let value = ctx.raised_instance(error);
    ctx.heap.keep(value);
    let class = ctx.exact_type(value)?;
    let mut tb = ctx.exception_traceback(value)?;
    if tb == NONE
        && supply_frame
        && let Some((loc, frame)) = ctx.active_frame(0)?
    {
        tb = ctx.alloc(Value::Traceback {
            frame,
            next: NONE,
            lineno: i64::from(loc.line),
            span: loc.span,
        });
        ctx.set_traceback(value, tb)?;
    }
    // A deallocator's __repr__ may explicitly clear exc_value.__traceback__.
    ctx.heap.keep(tb);
    Ok([class, value, tb])
}

fn record(ctx: &mut Context, [class, value, tb]: [Id; 3], message: Id, object: Id) -> Id {
    fixed_structseq::record(
        ctx,
        fixed_structseq::UNRAISABLE_ARGS,
        vec![class, value, tb, message, object],
    )
}

/// A hook or an audit veto can run Python, including nested collections and finalizers.
pub(super) fn deallocator(ctx: &mut Context, error: Error, method: Id) {
    report(
        ctx,
        error,
        true,
        Some(method),
        "Exception ignored while calling deallocator",
    );
}

/// Preserve the exception before formatting the callable or invoking the user's hook.
pub(super) fn report(
    ctx: &mut Context,
    error: Error,
    supply_frame: bool,
    callable: Option<Id>,
    prefix: &str,
) {
    let Ok(fields) = exception_fields(ctx, error, supply_frame) else {
        return;
    };
    let message = match callable {
        Some(callable) => match ctx.checked_format(callable, true) {
            Ok(name) => ctx.text(format!("{prefix} {name}")),
            Err(_) => NONE,
        },
        None => ctx.text(prefix),
    };
    let args = record(ctx, fields, message, NONE);
    ctx.heap.keep(args);
    let Ok(hook) = ctx.sys_binding("unraisablehook") else {
        return;
    };
    let Some(hook) = hook else {
        let _ = display(ctx, args);
        return;
    };
    ctx.heap.keep(hook);
    let (error, message, object) = match audit::event(ctx, "sys.unraisablehook", &[hook, args]) {
        Err(error) => (error, "Exception ignored in audit hook", NONE),
        Ok(()) if hook == NONE => {
            let _ = display(ctx, args);
            return;
        }
        Ok(()) => match ctx.call(hook, vec![args], Vec::new()) {
            Ok(_) => return,
            Err(error) => (error, "Exception ignored in sys.unraisablehook", hook),
        },
    };
    if let Ok(fields) = exception_fields(ctx, error, false) {
        let message = ctx.text(message);
        let args = record(ctx, fields, message, object);
        ctx.heap.keep(args);
        let _ = display(ctx, args);
    }
}

pub(super) fn default_hook(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "sys.unraisablehook() takes no keyword arguments",
        ));
    }
    let [_, args] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "sys.unraisablehook() takes exactly one argument ({} given)",
                args.len().saturating_sub(1)
            ),
        ));
    };
    display(ctx, *args)?;
    Ok(NONE)
}

fn write(ctx: &mut Context, stream: Id, text: Id) -> Result<()> {
    let function = ctx.lookup_attr(stream, "write")?;
    ctx.call(function, vec![text], Vec::new())?;
    Ok(())
}

fn write_text(ctx: &mut Context, stream: Id, text: impl Into<String>) -> Result<()> {
    let text = ctx.text(text);
    write(ctx, stream, text)
}

fn display(ctx: &mut Context, args: Id) -> Result<()> {
    let Value::NamedTuple {
        class: Some(fixed_structseq::UNRAISABLE_ARGS),
        values,
        ..
    } = ctx.heap.get(args)
    else {
        return Err(Error::new(
            "TypeError",
            "sys.unraisablehook argument type must be UnraisableHookArgs",
        ));
    };
    let &[class, value, tb, message, object] = values.as_slice() else {
        unreachable!()
    };
    let Some(file) = ctx.sys_binding("stderr")?.filter(|&id| id != NONE) else {
        return Ok(());
    };
    ctx.heap.keep(file);
    let convert = ctx.builtin("str");
    if message != NONE {
        let prefix = ctx.call(convert, vec![message], Vec::new())?;
        write(ctx, file, prefix)?;
    } else if object != NONE {
        write_text(ctx, file, "Exception ignored in")?;
    }
    if object != NONE {
        write_text(ctx, file, ": ")?;
        let repr = ctx.builtin("repr");
        let object = ctx
            .call(repr, vec![object], Vec::new())
            .unwrap_or_else(|_| ctx.text("<object repr() failed>"));
        write(ctx, file, object)?;
        write_text(ctx, file, "\n")?;
    } else if message != NONE {
        write_text(ctx, file, ":\n")?;
    }
    if tb != NONE {
        // PyTraceBack_Print retains the innermost frames, with a default limit of 1000.
        let limit = ctx
            .sys_binding("tracebacklimit")?
            .and_then(|id| ctx.as_int(id).ok())
            .map_or(1000, |n| {
                n.to_i64().unwrap_or_else(|| {
                    if n.big().sign() == num_bigint::Sign::Minus {
                        0
                    } else {
                        i64::MAX
                    }
                })
            });
        let mut lines = Vec::new();
        if limit > 0 {
            let _ = traceback::format_frames(ctx, tb, Some(-limit), &mut lines);
        }
        for line in lines {
            if write_text(ctx, file, line).is_err() {
                break;
            }
        }
    }
    let module = ctx
        .lookup_attr(class, "__module__")
        .and_then(|s| ctx.str_value(s).map(str::to_owned))
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let name = ctx
        .lookup_attr(class, "__qualname__")
        .and_then(|s| ctx.str_value(s).map(str::to_owned))
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let name = if matches!(module.as_str(), "builtins" | "__main__") {
        name
    } else {
        format!("{module}.{name}")
    };
    write_text(ctx, file, name)?;
    if value != NONE {
        write_text(ctx, file, ": ")?;
        let text = ctx
            .call(convert, vec![value], Vec::new())
            .unwrap_or_else(|_| ctx.text("<exception str() failed>"));
        write(ctx, file, text)?;
    }
    write_text(ctx, file, "\n")?;
    let flush = ctx.lookup_attr(file, "flush")?;
    ctx.call(flush, Vec::new(), Vec::new())?;
    Ok(())
}
