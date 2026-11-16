//! Explicit Python audit events and interpreter-local hook registration.

use super::{Context, Error, Id, NONE, Result, Value};
use crate::dynamic::{copy_ids, copy_label};

/// # Safety
/// The originating frame owns the context and its event operands through all hook calls.
unsafe fn dispatch(context: *mut Context, event: Id, args: &[Id]) -> Result<Id> {
    // SAFETY: create the argument tuple and reserve depth in the active execution.
    let ctx = unsafe { &mut *context };
    let args = ctx.heap.packed_tuple(copy_ids(args)?, false);
    ctx.check_call_depth(ctx.heap.execution.slot_depth)?;
    ctx.enter_slot_depth("maximum recursion depth exceeded")?;
    let temporaries = ctx.heap.temporary_mark();
    let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
        // A hook added by a running hook also receives this event.
        for index in 0.. {
            // SAFETY: the preceding invocation returned to this execution.
            let ctx = unsafe { &mut *context };
            let Some(&hook) = ctx.audit_hooks.get(index) else {
                break;
            };
            match ctx.lookup_attr(hook, "__cantrace__") {
                Ok(flag) => {
                    ctx.truth(flag)?;
                }
                Err(error) if ctx.is_attribute_error(&error) => {}
                Err(error) => return Err(error),
            }
            ctx.discard_truth(temporaries);
            let call = ctx.prepare_call(hook, vec![event, args].into(), Vec::new())?;
            // SAFETY: the active frame roots the event, and preparation has returned its owned call.
            unsafe { call.run(context) }?;
            // SAFETY: the returning hook has released its frame.
            unsafe { &mut *context }.discard_truth(temporaries);
        }
        Ok(NONE)
    });
    // SAFETY: all nested hook dispatches and their waits have returned.
    unsafe { &mut *context }.heap.execution.slot_depth -= 1;
    result
}

/// Emit an event from the runtime, using this interpreter's live audit hooks.
pub(in crate::dynamic) fn event(ctx: &mut Context, name: &str, args: &[Id]) -> Result<()> {
    // SAFETY: this synchronous caller owns the context through event delivery.
    unsafe { event_at(ctx, name, args) }
}

/// # Safety
/// The calling execution owns the context and roots the arguments until this event returns.
pub(in crate::dynamic) unsafe fn event_at(
    context: *mut Context,
    name: &str,
    args: &[Id],
) -> Result<()> {
    // SAFETY: only the event name lookup needs the context before dispatch.
    let ctx = unsafe { &mut *context };
    if !ctx.audit_hooks.is_empty() {
        let name = ctx.text(name);
        // SAFETY: the event name and its arguments are rooted in the caller.
        unsafe { dispatch(context, name, args) }?;
    }
    Ok(())
}

pub(super) fn emit(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "sys.audit() takes no keyword arguments",
        ));
    }
    let Some((&event, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "audit expected at least 1 argument, got 0",
        ));
    };
    let Some(storage) = ctx.string_storage(event) else {
        let class = if event == NONE {
            "None"
        } else {
            ctx.type_name(event)
        };
        return Err(Error::new(
            "TypeError",
            format!("audit() argument 1 must be str, not {class}"),
        ));
    };
    if matches!(ctx.heap.get(storage), Value::SurrogateStr(_)) {
        ctx.encode_utf8_codepoints(event, false, "strict")?;
    }
    let name = ctx.str_value(storage)?;
    if name.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }
    if ctx.audit_hooks.is_empty() {
        return Ok(NONE);
    }
    let name = copy_label(name)?;
    let name = ctx.text(name);
    // SAFETY: the emitter keeps all arguments live through synchronous dispatch.
    unsafe { dispatch(ctx, name, args) }
}

pub(super) fn register(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let hook = match (args, kwargs) {
        ([hook], []) => *hook,
        ([], [(key, hook)]) if key == "hook" => *hook,
        _ => {
            let count = args.len() + kwargs.len();
            return Err(Error::new(
                "TypeError",
                if count > 1 {
                    let kind = if args.is_empty() {
                        "keyword argument"
                    } else {
                        "argument"
                    };
                    format!("addaudithook() takes at most 1 {kind} ({count} given)")
                } else {
                    "addaudithook() missing required argument 'hook' (pos 1)".to_owned()
                },
            ));
        }
    };
    if !ctx.audit_hooks.is_empty() {
        let name = ctx.text("sys.addaudithook");
        // SAFETY: registration owns the context until the existing hooks return.
        if let Err(error) = unsafe { dispatch(ctx, name, &[]) } {
            let exception = ctx.builtin("Exception");
            if ctx.raised_error_matches(&error, exception) {
                return Ok(NONE);
            }
            return Err(error);
        }
    }
    ctx.audit_hooks
        .try_reserve(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    ctx.audit_hooks.push(hook);
    Ok(NONE)
}
