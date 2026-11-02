use super::{Context, Error, Id, NONE, Result, builtins, c_int};

pub(in crate::dynamic) const PRIORITY_AND_YIELD: [&str; 3] = [
    "sched_get_priority_min",
    "sched_get_priority_max",
    "sched_yield",
];

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
    let param = ctx.builtin(super::super::fixed_structseq::SCHED_PARAM);
    ctx.bind(scope, "sched_param", param);
    for name in PRIORITY_AND_YIELD {
        let function = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, function);
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

/// Get the policy's priority bound, distinguishing a valid -1 result from an errno.
pub(in crate::dynamic::stdlib) fn priority_bound(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [Some(policy)] = builtins::bind_builtin_arguments(name, args, kwargs, ["policy"], 1)?
    else {
        unreachable!("policy is required")
    };
    let policy = c_int(ctx, policy)?;
    // SAFETY: both queries take a policy by value. Only the calling thread's errno is cleared.
    let bound = unsafe {
        *libc::__errno_location() = 0;
        if name == "sched_get_priority_min" {
            libc::sched_get_priority_min(policy)
        } else {
            libc::sched_get_priority_max(policy)
        }
    };
    if bound == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(0) {
            return Err(crate::objects::errno_error(error).into());
        }
    }
    Ok(ctx.int(i64::from(bound)))
}

/// Relinquish the calling thread's processor to the kernel scheduler.
pub(in crate::dynamic::stdlib) fn yield_cpu(args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "posix.sched_yield() takes no keyword arguments",
        ));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "posix.sched_yield() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    // SAFETY: sched_yield takes no arguments and retains no pointers.
    if unsafe { libc::sched_yield() } < 0 {
        return Err(crate::objects::errno_error(std::io::Error::last_os_error()).into());
    }
    Ok(NONE)
}
