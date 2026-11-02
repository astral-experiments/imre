//! Deliver externally requested faults to the native Python image while its launcher waits.
use std::io;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering::SeqCst};

const SIGNALS: [i32; 5] = [
    libc::SIGABRT,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGILL,
    libc::SIGSEGV,
];
static CHILD: AtomicI32 = AtomicI32::new(0);
static HANDLERS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn forward(sig: i32, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    // SAFETY: SA_SIGINFO supplies signal metadata. Only externally generated
    // signals belong to the child. A hardware fault in the launcher stays here.
    unsafe {
        #[cfg(target_os = "linux")]
        let (external, errno) = ((*info).si_code <= 0, libc::__errno_location());
        #[cfg(target_os = "macos")]
        let (external, errno) = ((*info).si_pid != 0, libc::__error());
        let saved = *errno;
        if external {
            HANDLERS.fetch_add(1, SeqCst);
            let pid = CHILD.load(SeqCst);
            if pid > 0 {
                libc::kill(pid, sig);
            }
            HANDLERS.fetch_sub(1, SeqCst);
        } else {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
        *errno = saved;
    }
}

pub(super) struct Relay(Vec<(i32, libc::sigaction)>);

impl Relay {
    pub(super) fn install(pid: u32) -> io::Result<Self> {
        let mut relay = Self(Vec::with_capacity(SIGNALS.len()));
        CHILD.store(pid as i32, SeqCst);
        // SAFETY: the handler is signal-safe and every replaced disposition is
        // restored by Drop, including when installation stops partway through.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = forward as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART | libc::SA_ONSTACK;
            libc::sigemptyset(&mut action.sa_mask);
            for sig in SIGNALS {
                let mut old = std::mem::zeroed();
                if libc::sigaction(sig, &action, &mut old) != 0 {
                    return Err(io::Error::last_os_error());
                }
                relay.0.push((sig, old));
            }
        }
        Ok(relay)
    }

    pub(super) fn wait_for_exit(&self, pid: u32) -> io::Result<()> {
        loop {
            // SAFETY: waitid initializes info. WNOWAIT leaves this specific child
            // waitable so its PID cannot be reused while signals are forwarded.
            let rc = unsafe {
                let mut info = std::mem::zeroed();
                libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT)
            };
            if rc == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        CHILD.store(0, SeqCst);
        for (sig, action) in &self.0 {
            // SAFETY: each action was initialized by a successful sigaction.
            unsafe { libc::sigaction(*sig, action, std::ptr::null_mut()) };
        }
        while HANDLERS.load(SeqCst) != 0 {
            std::thread::yield_now();
        }
    }
}

pub(crate) fn propagate(sig: i32) {
    if SIGNALS.contains(&sig) {
        // SAFETY: this launcher is about to exit. Preserve the native child's
        // signal wait status without overwriting its core file with a second dump.
        unsafe {
            let mut mask = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, sig);
            libc::setrlimit(
                libc::RLIMIT_CORE,
                &libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                },
            );
            libc::signal(sig, libc::SIG_DFL);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &mask, std::ptr::null_mut());
            libc::raise(sig);
        }
    }
}
