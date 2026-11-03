//! Linux scheduling parameters for a process and for POSIX spawn attributes.
use crate::dynamic::stdlib::{fixed_structseq, os};
use crate::dynamic::{Context, Error, Id, NONE, Result, Value, builtins};

/// Convert the exact sched_param struct sequence, including the scalar's index protocol.
pub(in crate::dynamic::stdlib) fn convert(
    ctx: &mut Context,
    param: Id,
) -> Result<libc::sched_param> {
    let Value::NamedTuple {
        class: Some(fixed_structseq::SCHED_PARAM),
        values,
        ..
    } = ctx.heap.get(param)
    else {
        return Err(Error::new("TypeError", "must have a sched_param object"));
    };
    let priority = values[0];
    let priority = ctx.index_c_long(priority)?;
    // SAFETY: Linux sched_param contains integer fields; leave unused fields zero.
    let mut native = unsafe { std::mem::zeroed::<libc::sched_param>() };
    native.sched_priority = i32::try_from(priority)
        .map_err(|_| Error::new("OverflowError", "sched_priority out of range"))?;
    Ok(native)
}

/// Read or set the Linux scheduling parameters for a single PID.
pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "sched_getparam" {
        let pid = builtins::one_positional_arg("posix.sched_getparam", args, kwargs)?;
        let pid = os::c_int(ctx, pid)?;
        // SAFETY: zero is valid for the integer fields of Linux sched_param.
        let mut param = unsafe { std::mem::zeroed::<libc::sched_param>() };
        // SAFETY: param is an aligned, writable sched_param borrowed exclusively for this call.
        if unsafe { libc::sched_getparam(pid, &mut param) } < 0 {
            return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
        }
        let priority = ctx.int(i64::from(param.sched_priority));
        return Ok(fixed_structseq::record(
            ctx,
            fixed_structseq::SCHED_PARAM,
            vec![priority],
        ));
    }

    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("posix.{name}() takes no keyword arguments"),
        ));
    }
    let required = if name == "sched_setparam" { 2 } else { 3 };
    if args.len() != required {
        return Err(Error::new(
            "TypeError",
            format!("{name} expected {required} arguments, got {}", args.len()),
        ));
    }
    let pid = os::c_int(ctx, args[0])?;
    let rc = if name == "sched_setparam" {
        let param = convert(ctx, args[1])?;
        // SAFETY: param is fully initialized and libc reads it during the call.
        unsafe { libc::sched_setparam(pid, &param) }
    } else {
        let policy = os::c_int(ctx, args[1])?;
        let param = convert(ctx, args[2])?;
        // SAFETY: PID and policy are passed by value and the pointer is valid for this call.
        unsafe { libc::sched_setscheduler(pid, policy, &param) }
    };
    if rc < 0 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(NONE)
}
