//! POSIX handlers record delivery; Python callbacks run at checkpoints on the main thread.
use std::io;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

const LIMIT: usize = std::mem::size_of::<libc::sigset_t>() * 8 + 1;
static PENDING: [AtomicBool; LIMIT] = [const { AtomicBool::new(false) }; LIMIT];
#[unsafe(export_name = "imre_signal_tripped")]
pub(crate) static TRIPPED: AtomicBool = AtomicBool::new(false);
static MAIN: OnceLock<ThreadId> = OnceLock::new();

extern "C" fn trip(number: libc::c_int) {
    if let Some(bit) = PENDING.get(number as usize) {
        bit.store(true, Ordering::Relaxed);
        TRIPPED.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    Default,
    Ignore,
    Python,
}

impl Disposition {
    fn decode(action: libc::sigaction) -> Option<Self> {
        match action.sa_sigaction {
            libc::SIG_DFL => Some(Self::Default),
            libc::SIG_IGN => Some(Self::Ignore),
            ptr if ptr == trip as *const () as usize => Some(Self::Python),
            _ => None,
        }
    }
}

pub(crate) fn disposition(number: i32) -> io::Result<Option<Disposition>> {
    let mut previous = std::mem::MaybeUninit::uninit();
    // SAFETY: a null action queries the disposition into previous without changing it.
    if unsafe { libc::sigaction(number, std::ptr::null(), previous.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sigaction initialized the previous disposition on success.
    Ok(Disposition::decode(unsafe { previous.assume_init() }))
}

pub(crate) fn install(number: i32, handler: Disposition) -> io::Result<Option<Disposition>> {
    // SAFETY: initialize the mask and handler before passing the action to POSIX.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // Let C embedders supply an alternate signal stack, as CPython does.
    action.sa_flags = libc::SA_ONSTACK;
    action.sa_sigaction = match handler {
        Disposition::Default => libc::SIG_DFL,
        Disposition::Ignore => libc::SIG_IGN,
        Disposition::Python => trip as *const () as usize,
    };
    let mut previous = std::mem::MaybeUninit::uninit();
    // SAFETY: both structures live through sigaction. SA_RESTART is clear for Python handlers.
    if unsafe { libc::sigaction(number, &action, previous.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sigaction initialized the previous disposition on success.
    Ok(Disposition::decode(unsafe { previous.assume_init() }))
}

pub(crate) fn initialize_interrupt() {
    MAIN.get_or_init(|| {
        if matches!(disposition(libc::SIGINT), Ok(Some(Disposition::Default))) {
            let _ = install(libc::SIGINT, Disposition::Python);
        }
        std::thread::current().id()
    });
}

pub(crate) fn is_main() -> bool {
    MAIN.get() == Some(&std::thread::current().id())
}

pub(crate) fn nsig() -> i32 {
    #[cfg(target_os = "linux")]
    {
        libc::SIGRTMAX() + 1
    }
    #[cfg(not(target_os = "linux"))]
    {
        static COUNT: OnceLock<i32> = OnceLock::new();
        *COUNT.get_or_init(|| {
            (1..LIMIT as i32)
                .rev()
                .find(|&number| disposition(number).is_ok())
                .unwrap_or(0)
                + 1
        })
    }
}

pub(crate) fn begin_dispatch() -> bool {
    TRIPPED.load(Ordering::Acquire) && is_main() && TRIPPED.swap(false, Ordering::AcqRel)
}

pub(crate) fn resume_dispatch() {
    TRIPPED.store(true, Ordering::Release);
}
pub(crate) fn take(number: i32) -> bool {
    PENDING[number as usize].swap(false, Ordering::AcqRel)
}
pub(crate) fn take_interrupt() -> bool {
    if !begin_dispatch() {
        return false;
    }
    let interrupted = take(libc::SIGINT);
    if PENDING.iter().any(|bit| bit.load(Ordering::Acquire)) {
        resume_dispatch();
    }
    interrupted
}

/// Report an unhandled KeyboardInterrupt to a POSIX parent after printing and flushing.
pub(crate) fn exit_interrupt() -> i32 {
    if install(libc::SIGINT, Disposition::Default).is_ok() {
        // SAFETY: raise targets this process with its restored default action.
        unsafe { libc::raise(libc::SIGINT) };
    }
    128 + libc::SIGINT
}
