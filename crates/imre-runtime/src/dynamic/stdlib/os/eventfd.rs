use super::{Context, Id, NONE, Result, Value, builtins, c_int, unsigned};

pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for name in ["eventfd", "eventfd_read", "eventfd_write"] {
        let func = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, func);
    }
    for (name, flag) in [
        ("EFD_CLOEXEC", libc::EFD_CLOEXEC),
        ("EFD_NONBLOCK", libc::EFD_NONBLOCK),
        ("EFD_SEMAPHORE", libc::EFD_SEMAPHORE),
    ] {
        let flag = ctx.int(i64::from(flag));
        ctx.bind(scope, name, flag);
    }
}

fn descriptor(ctx: &mut Context, id: Id) -> Result<i32> {
    super::stat::warn_bool_fd(ctx, id)?;
    super::super::select::fd(ctx, id)
}

fn checked(result: libc::c_int) -> Result<libc::c_int> {
    if result < 0 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(result)
}

/// Create or access a Linux event counter with the caller's descriptor flags.
pub(in crate::dynamic::stdlib) fn call(
    ctx: &mut Context,
    op: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match op {
        "eventfd" => {
            let [initval, flags] =
                builtins::bind_builtin_arguments(op, args, kwargs, ["initval", "flags"], 1)?;
            let initial = unsigned(ctx, initval.unwrap(), u64::from(u32::MAX), "int")? as u32;
            let flags = flags
                .map(|f| c_int(ctx, f))
                .transpose()?
                .unwrap_or(libc::EFD_CLOEXEC);
            // SAFETY: eventfd takes two value arguments; the caller owns a successful descriptor.
            let fd = checked(unsafe { libc::eventfd(initial, flags) })?;
            Ok(ctx.int(i64::from(fd)))
        }
        "eventfd_read" => {
            let [fd] = builtins::bind_builtin_arguments(op, args, kwargs, ["fd"], 1)?;
            let fd = descriptor(ctx, fd.unwrap())?;
            let mut value = 0;
            // SAFETY: value points to writable storage for one eventfd_t.
            checked(unsafe { libc::eventfd_read(fd, &mut value) })?;
            Ok(ctx.alloc(Value::Int(crate::Int::new(value.into()))))
        }
        "eventfd_write" => {
            let [fd, value] =
                builtins::bind_builtin_arguments(op, args, kwargs, ["fd", "value"], 2)?;
            let fd = descriptor(ctx, fd.unwrap())?;
            let value = unsigned(ctx, value.unwrap(), u64::MAX, "long long")?;
            // SAFETY: eventfd_write borrows the descriptor and takes the counter increment by value.
            checked(unsafe { libc::eventfd_write(fd, value) })?;
            Ok(NONE)
        }
        _ => unreachable!("registered eventfd operation"),
    }
}
