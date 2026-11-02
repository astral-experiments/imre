//! Private child continuation for spawn when exec may run Python callbacks.

use super::{Context, Error, Result};
use std::{io, mem::MaybeUninit, ptr};

fn unsupported() -> Error {
    Error::new(
        "NotImplementedError",
        "spawn with child-side Python requires a provably single-threaded glibc process",
    )
}

/// A positive glibc witness proves the process has never needed pthread locking
/// or has otherwise returned to a state glibc knows is single-threaded.
pub(super) fn glibc_proves_single_threaded() -> bool {
    // SAFETY: the glibc data symbol, when present, is a process-lifetime char.
    // Resolve dynamically to keep runtimes usable with pre-2.32 glibc.
    cfg!(target_env = "gnu")
        && unsafe {
            let marker = libc::dlsym(libc::RTLD_DEFAULT, c"__libc_single_threaded".as_ptr());
            !marker.is_null() && *marker.cast::<libc::c_char>() != 0
        }
}

/// Return zero in a child that may continue in the caller's boxed interpreter.
///
/// Imre starts native workers through pthreads. glibc documents a positive
/// __libc_single_threaded as proof that no other thread shares the process
/// image. A zero marker, including after a joined worker, supplies no proof.
/// Raw clone users bypassing the libc threading contract are unsupported.
pub(super) fn fork_for_child_callbacks(ctx: &mut Context) -> Result<libc::pid_t> {
    if ctx.threads.private_context
        || !crate::signals::is_main()
        || crate::objects::locks::native_thread_id() != u64::from(std::process::id())
    {
        return Err(unsupported());
    }
    let mut all = MaybeUninit::uninit();
    let mut previous = MaybeUninit::uninit();
    // SAFETY: sigfillset initializes all; both mask pointers are live. Blocking
    // signals spans the final witness check, libc fork, and child signal reset.
    let blocked = unsafe {
        libc::sigfillset(all.as_mut_ptr());
        libc::pthread_sigmask(libc::SIG_SETMASK, all.as_ptr(), previous.as_mut_ptr())
    };
    if blocked != 0 {
        return Err(super::super::os_error(io::Error::from_raw_os_error(
            blocked,
        )));
    }
    // SAFETY: the current thread is the process leader; the marker is read
    // according to glibc's <sys/single_threaded.h> contract. No guest callback
    // or worker launch occurs between a positive witness and the libc fork.
    let result = unsafe {
        if !glibc_proves_single_threaded() {
            Err(unsupported())
        } else {
            match libc::fork() {
                -1 => Err(super::super::os_error(io::Error::last_os_error())),
                0 => {
                    crate::signals::clear_pending_after_fork();
                    Ok(0)
                }
                pid => Ok(pid),
            }
        }
    };
    // SAFETY: the successful call above saved a valid mask for this same thread.
    // Restore before re-entering Python in either process, including on error.
    unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, previous.as_ptr(), ptr::null_mut()) };
    if matches!(result, Ok(0)) {
        crate::objects::reseed_after_fork();
        ctx.refresh_main_thread_after_fork();
    }
    result
}
