//! Linux process-state observations from waitid(2).

use super::{Context, Error, Id, NONE, Result, c_int, native};
use crate::dynamic::stdlib::fixed_structseq;

pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for (name, value) in [
        ("P_ALL", libc::P_ALL as i32),
        ("P_PID", libc::P_PID as i32),
        ("P_PGID", libc::P_PGID as i32),
        ("P_PIDFD", libc::P_PIDFD as i32),
        ("WEXITED", libc::WEXITED),
        ("WSTOPPED", libc::WSTOPPED),
        ("WCONTINUED", libc::WCONTINUED),
        ("WNOWAIT", libc::WNOWAIT),
        ("WUNTRACED", libc::WUNTRACED),
        ("CLD_EXITED", libc::CLD_EXITED),
        ("CLD_KILLED", libc::CLD_KILLED),
        ("CLD_DUMPED", libc::CLD_DUMPED),
        ("CLD_TRAPPED", libc::CLD_TRAPPED),
        ("CLD_STOPPED", libc::CLD_STOPPED),
        ("CLD_CONTINUED", libc::CLD_CONTINUED),
    ] {
        let value = ctx.int(i64::from(value));
        ctx.bind(scope, name, value);
    }
}

pub(super) fn call(ctx: &mut Context, args: &[Id]) -> Result<Id> {
    let [idtype, id, options] = args else {
        return Err(Error::new(
            "TypeError",
            format!("waitid expected 3 arguments, got {}", args.len()),
        ));
    };
    let idtype = c_int(ctx, *idtype)? as libc::idtype_t;
    let id = c_int(ctx, *id)? as libc::id_t;
    let options = c_int(ctx, *options)?;
    let info = loop {
        match native::waitid(idtype, id, options) {
            Ok(info) => break info,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                ctx.dispatch_signals()?
            }
            Err(error) => return Err(super::super::os_error(error)),
        }
    };
    // SAFETY: waitid produced child siginfo, or left si_pid zero for WNOHANG.
    let pid = unsafe { info.si_pid() };
    if pid == 0 {
        return Ok(NONE);
    }
    // SAFETY: a nonzero child PID selects the SIGCHLD fields in the siginfo union.
    let (uid, status) = unsafe { (info.si_uid(), info.si_status()) };
    let values = [
        i64::from(pid),
        i64::from(uid),
        i64::from(info.si_signo),
        i64::from(status),
        i64::from(info.si_code),
    ];
    let values = values.into_iter().map(|value| ctx.int(value)).collect();
    Ok(fixed_structseq::record(
        ctx,
        fixed_structseq::WAITID_RESULT,
        values,
    ))
}
