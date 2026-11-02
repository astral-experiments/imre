use super::{Context, Error, Id, Result, c_int};

/// Populate the Linux posix namespace so its dictionary exposes the host's policy constants.
pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for (name, policy) in [
        ("SCHED_OTHER", libc::SCHED_OTHER),
        ("SCHED_NORMAL", libc::SCHED_NORMAL),
        ("SCHED_FIFO", libc::SCHED_FIFO),
        ("SCHED_RR", libc::SCHED_RR),
        ("SCHED_BATCH", libc::SCHED_BATCH),
        ("SCHED_IDLE", libc::SCHED_IDLE),
        ("SCHED_DEADLINE", libc::SCHED_DEADLINE),
        ("SCHED_RESET_ON_FORK", libc::SCHED_RESET_ON_FORK),
    ] {
        let value = ctx.int(i64::from(policy));
        ctx.bind(scope, name, value);
    }
    let getter = ctx.builtin("posix.sched_getscheduler");
    ctx.bind(scope, "sched_getscheduler", getter);
}

/// Read the kernel policy for a checked pid_t, including the calling thread when pid is zero.
pub(in crate::dynamic::stdlib) fn get_policy(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "posix.sched_getscheduler() takes no keyword arguments",
        ));
    }
    let [pid] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "posix.sched_getscheduler() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let pid = c_int(ctx, *pid)?;
    // SAFETY: sched_getscheduler takes only a pid_t and validates the process in the kernel.
    let policy = unsafe { libc::sched_getscheduler(pid) };
    if policy < 0 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(ctx.int(i64::from(policy)))
}
