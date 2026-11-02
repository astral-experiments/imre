//! Deliver errors raised by Python deallocators to the interpreter's current sys hook.
use super::stdlib::{audit, fixed_structseq, traceback};
use super::{Context, Error, Id, NONE, Result, Value};

fn sys_binding(ctx: &mut Context, name: &str) -> Result<Option<Id>> {
    let sys = match ctx.threads.system_module {
        Some(sys) => sys,
        None => ctx.module("sys")?,
    };
    let Value::Module { scope, .. } = *ctx.heap.get(sys) else {
        unreachable!()
    };
    Ok(ctx.scope_name(scope, name))
}

fn record(ctx: &mut Context, value: Id, message: Id, object: Id) -> Result<Id> {
    let class = ctx.exact_type(value)?;
    let tb = ctx.exception_traceback(value)?;
    Ok(fixed_structseq::record(
        ctx,
        fixed_structseq::UNRAISABLE_ARGS,
        vec![class, value, tb, message, object],
    ))
}

/// A hook or an audit veto can run Python, including nested collections and finalizers.
pub(super) fn deallocator(ctx: &mut Context, error: Error, method: Id) {
    let value = ctx.raised_instance(error);
    ctx.heap.keep(value);
    let message = match ctx.checked_format(method, true) {
        Ok(method) => ctx.text(format!(
            "Exception ignored while calling deallocator {method}"
        )),
        Err(_) => NONE,
    };
    let Ok(args) = record(ctx, value, message, NONE) else {
        return;
    };
    ctx.heap.keep(args);
    let Ok(hook) = sys_binding(ctx, "unraisablehook") else {
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
    let value = ctx.raised_instance(error);
    ctx.heap.keep(value);
    let message = ctx.text(message);
    if let Ok(args) = record(ctx, value, message, object) {
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
    let Some(file) = sys_binding(ctx, "stderr")?.filter(|&id| id != NONE) else {
        return Ok(());
    };
    ctx.heap.keep(file);
    let mut lines = Vec::new();
    if object != NONE {
        let prefix = if message == NONE {
            "Exception ignored in".to_owned()
        } else {
            ctx.checked_format(message, false)?
        };
        let object = ctx
            .checked_format(object, true)
            .unwrap_or_else(|_| "<object repr() failed>".to_owned());
        lines.push(format!("{prefix}: {object}\n"));
    } else if message != NONE {
        lines.push(format!("{}:\n", ctx.checked_format(message, false)?));
    }
    let _ = traceback::format_frames(ctx, tb, None, &mut lines);
    let module = ctx
        .lookup_attr(class, "__module__")
        .and_then(|s| ctx.str_value(s).map(str::to_owned))
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let name = ctx
        .lookup_attr(class, "__qualname__")
        .and_then(|s| ctx.str_value(s).map(str::to_owned))
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let mut tail = if matches!(module.as_str(), "builtins" | "__main__") {
        name
    } else {
        format!("{module}.{name}")
    };
    if value != NONE {
        tail.push_str(": ");
        tail.push_str(
            &ctx.checked_format(value, false)
                .unwrap_or_else(|_| "<exception str() failed>".to_owned()),
        );
    }
    tail.push('\n');
    lines.push(tail);
    for line in lines {
        let write = ctx.lookup_attr(file, "write")?;
        let line = ctx.text(line);
        ctx.call(write, vec![line], Vec::new())?;
    }
    let flush = ctx.lookup_attr(file, "flush")?;
    ctx.call(flush, Vec::new(), Vec::new())?;
    Ok(())
}
