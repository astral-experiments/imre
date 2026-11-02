//! POSIX handlers record delivery; Python callbacks run at checkpoints on the main thread.
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread::ThreadId;

const LIMIT: usize = std::mem::size_of::<libc::sigset_t>() * 8 + 1;
static PENDING: [AtomicBool; LIMIT] = [const { AtomicBool::new(false) }; LIMIT];
#[unsafe(export_name = "imre_signal_tripped")]
pub(crate) static TRIPPED: AtomicBool = AtomicBool::new(false);
static MAIN: OnceLock<ThreadId> = OnceLock::new();
static WAKEUP: OnceLock<(UnixDatagram, UnixDatagram)> = OnceLock::new();
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn trip(number: libc::c_int) {
    if let Some(bit) = PENDING.get(number as usize) {
        bit.store(true, Ordering::Relaxed);
        if !TRIPPED.swap(true, Ordering::AcqRel) {
            let fd = WRITE_FD.load(Ordering::Acquire);
            if fd >= 0 {
                let saved = errno::errno();
                // SAFETY: the socket remains open and nonblocking. write is async-signal-safe.
                unsafe { libc::write(fd, b"\0".as_ptr().cast(), 1) };
                errno::set_errno(saved);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition { Default, Ignore, Python }

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

pub(crate) fn is_main() -> bool { MAIN.get() == Some(&std::thread::current().id()) }

pub(crate) fn nsig() -> i32 {
    #[cfg(target_os = "linux")]
    { libc::SIGRTMAX() + 1 }
    #[cfg(not(target_os = "linux"))]
    {
        static COUNT: OnceLock<i32> = OnceLock::new();
        *COUNT.get_or_init(|| (1..LIMIT as i32).rev()
            .find(|&number| disposition(number).is_ok()).unwrap_or(0) + 1)
    }
}

pub(crate) fn begin_dispatch() -> bool {
    TRIPPED.load(Ordering::Acquire) && is_main() && TRIPPED.swap(false, Ordering::AcqRel)
}

pub(crate) fn resume_dispatch() { TRIPPED.store(true, Ordering::Release); }
pub(crate) fn take(number: i32) -> bool { PENDING[number as usize].swap(false, Ordering::AcqRel) }
pub(crate) fn take_interrupt() -> bool {
    if !begin_dispatch() { return false; }
    let interrupted = take(libc::SIGINT);
    if PENDING.iter().any(|bit| bit.load(Ordering::Acquire)) { resume_dispatch(); }
    interrupted
}

/// Publish a permanent nonblocking wake socket before the first interruptible main-thread wait.
pub(crate) fn sleep_wakeup() -> io::Result<Option<RawFd>> {
    if !is_main() { return Ok(None); }
    if WAKEUP.get().is_none() {
        let (read, write) = UnixDatagram::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let _ = WAKEUP.set((read, write));
    }
    let (read, write) = WAKEUP.get().expect("initialized by the main thread");
    WRITE_FD.store(write.as_raw_fd(), Ordering::Release);
    Ok(Some(read.as_raw_fd()))
}

pub(crate) fn drain_wakeup() {
    if let Some((read, _)) = WAKEUP.get() {
        while read.recv(&mut [0]).is_ok() {}
    }
}

/// Report an unhandled KeyboardInterrupt to a POSIX parent after printing and flushing.
pub(crate) fn exit_interrupt() -> i32 {
    if install(libc::SIGINT, Disposition::Default).is_ok() {
        // SAFETY: raise targets this process with its restored default action.
        unsafe { libc::raise(libc::SIGINT) };
    }
    128 + libc::SIGINT
}
