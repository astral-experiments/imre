//! POSIX handlers record delivery; Python callbacks run at checkpoints on the main thread.
use std::io;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU8, AtomicUsize, Ordering};
use std::thread::ThreadId;

const LIMIT: usize = std::mem::size_of::<libc::sigset_t>() * 8 + 1;
// A wakeup-fd report can reenter Python while an outer scan still owns later signals.
static SIGNALS_PENDING: AtomicBool = AtomicBool::new(false);
static PENDING: [AtomicBool; LIMIT] = [const { AtomicBool::new(false) }; LIMIT];
// A worker can request delivery without reading the main interpreter's Python heap.
const FOREIGN: u8 = 0;
const SUPPRESSED: u8 = 1;
const PYTHON: u8 = 2;
static DISPOSITIONS: [AtomicU8; LIMIT] = [const { AtomicU8::new(FOREIGN) }; LIMIT];
#[unsafe(export_name = "imre_signal_tripped")]
pub(crate) static TRIPPED: AtomicBool = AtomicBool::new(false);
static MAIN: OnceLock<ThreadId> = OnceLock::new();

// The kernel can call trip on any thread. Publish the borrowed fd and its
// warning policy together, without allocating or taking a lock in the handler.
const WARN_ON_FULL: i64 = 1 << 32;
static WAKEUP: AtomicI64 = AtomicI64::new(-1);
static WRITERS: AtomicUsize = AtomicUsize::new(0);
// Reserve FIFO positions without waiting for another signal handler to publish.
static ERROR_HEAD: AtomicUsize = AtomicUsize::new(0);
static ERROR_TAIL: AtomicUsize = AtomicUsize::new(0);
// Match the bounded pending-call capacity on CPython's main thread.
static WAKEUP_ERRORS: [AtomicI32; 32] = [const { AtomicI32::new(0) }; 32];

pub(crate) fn replace_wakeup_fd(fd: i32, warn_on_full: bool) -> i32 {
    let mut target = i64::from(fd);
    if warn_on_full {
        target |= WARN_ON_FULL;
    }
    WAKEUP.swap(target, Ordering::AcqRel) as i32
}

fn queue_wakeup_error(error: i32) {
    let ticket = ERROR_TAIL.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |tail| {
        // Another producer and the reader can both pass this tail snapshot.
        // In that case the CAS must retry, rather than dropping an error.
        let used = tail
            .wrapping_sub(ERROR_HEAD.load(Ordering::Acquire))
            .cast_signed();
        (used < WAKEUP_ERRORS.len() as isize).then(|| tail.wrapping_add(1))
    });
    if let Ok(ticket) = ticket {
        WAKEUP_ERRORS[ticket % WAKEUP_ERRORS.len()].store(error, Ordering::Release);
        resume_dispatch();
    }
}

pub(crate) fn has_wakeup_errors() -> bool {
    let head = ERROR_HEAD.load(Ordering::Relaxed);
    WAKEUP_ERRORS[head % WAKEUP_ERRORS.len()].load(Ordering::Acquire) != 0
}

pub(crate) fn wakeup_errors() -> impl Iterator<Item = io::Error> {
    (0..WAKEUP_ERRORS.len()).map_while(|_| {
        let head = ERROR_HEAD.load(Ordering::Relaxed);
        let code = WAKEUP_ERRORS[head % WAKEUP_ERRORS.len()].swap(0, Ordering::Acquire);
        // An interrupted producer will request dispatch when its entry is ready.
        if code == 0 {
            return None;
        }
        ERROR_HEAD.store(head.wrapping_add(1), Ordering::Release);
        Some(io::Error::from_raw_os_error(code))
    })
}

fn wake_selector(number: i32) {
    if WAKEUP.load(Ordering::Relaxed) != -1 {
        // Pin before reading the fd. Shutdown either sees this writer or the
        // writer sees the disabled fd, even if it was preempted at entry.
        WRITERS.fetch_add(1, Ordering::SeqCst);
        let target = WAKEUP.load(Ordering::SeqCst);
        if target != -1 {
            write_wakeup_byte(target, number);
        }
        WRITERS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn write_wakeup_byte(target: i64, number: i32) {
    let byte = number as u8;
    let saved = errno::errno();
    // SAFETY: write is async-signal-safe and reads only this byte. The fd is
    // borrowed from Python and may have been closed since registration.
    while unsafe { libc::write(target as i32, (&raw const byte).cast(), 1) } < 0 {
        let error = errno::errno().0;
        if error == libc::EINTR {
            continue;
        }
        if target & WARN_ON_FULL != 0 || (error != libc::EAGAIN && error != libc::EWOULDBLOCK) {
            queue_wakeup_error(error);
        }
        break;
    }
    errno::set_errno(saved);
}

extern "C" fn trip(number: libc::c_int) {
    if let Some(bit) = PENDING.get(number as usize) {
        bit.store(true, Ordering::Relaxed);
        resume_signals();
        // A selector draining this byte must already be able to see the pending handler.
        wake_selector(number);
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

fn current_action(number: i32) -> io::Result<libc::sigaction> {
    let mut previous = std::mem::MaybeUninit::uninit();
    // SAFETY: a null action queries the disposition into previous without changing it.
    if unsafe { libc::sigaction(number, std::ptr::null(), previous.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sigaction initialized the previous disposition on success.
    Ok(unsafe { previous.assume_init() })
}

/// Suspend this thread until the kernel delivers a caught signal.
#[unsafe(no_mangle)]
pub extern "C" fn imre_signal_pause() {
    // SAFETY: pause has no arguments; interruption is its normal completion.
    unsafe { libc::pause() };
}

pub(crate) fn disposition(number: i32) -> io::Result<Option<Disposition>> {
    current_action(number).map(Disposition::decode)
}

pub(crate) fn set_interrupt(number: i32, interrupt: bool) -> io::Result<()> {
    let mut action = current_action(number)?;
    if interrupt {
        action.sa_flags &= !libc::SA_RESTART;
    } else {
        action.sa_flags |= libc::SA_RESTART;
    }
    // SAFETY: preserve the installed handler, mask and other flags from sigaction.
    if unsafe { libc::sigaction(number, &action, std::ptr::null_mut()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn install(number: i32, handler: Disposition) -> io::Result<()> {
    // SAFETY: initialize the mask and handler before passing the action to POSIX.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: sigemptyset initializes the writable mask field of this local action.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // Let C embedders supply an alternate signal stack, as CPython does.
    action.sa_flags = libc::SA_ONSTACK;
    action.sa_sigaction = match handler {
        Disposition::Default => libc::SIG_DFL,
        Disposition::Ignore => libc::SIG_IGN,
        Disposition::Python => trip as *const () as usize,
    };
    // SAFETY: action lives through sigaction. SA_RESTART is clear for Python handlers.
    if unsafe { libc::sigaction(number, &action, std::ptr::null_mut()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    record_handler(number as usize, Some(handler));
    Ok(())
}

/// Share only the disposition bits needed by workers, never a Python handler reference.
pub(crate) fn record_handler(number: usize, handler: Option<Disposition>) {
    let state = match handler {
        Some(Disposition::Default | Disposition::Ignore) => SUPPRESSED,
        Some(Disposition::Python) => PYTHON,
        None => FOREIGN,
    };
    DISPOSITIONS[number].store(state, Ordering::Release);
}

/// Honor the interpreter disposition even if a native library has changed sigaction.
pub(crate) fn interrupt_main(number: i32) {
    if DISPOSITIONS[number as usize].load(Ordering::Acquire) != SUPPRESSED {
        trip(number);
    }
}

pub(crate) fn initialize_interrupt() {
    MAIN.get_or_init(|| {
        if matches!(disposition(libc::SIGINT), Ok(Some(Disposition::Default))) {
            let _ = install(libc::SIGINT, Disposition::Python);
        }
        std::thread::current().id()
    });
}

/// Stop using the borrowed fd before shutdown releases Python-owned resources.
pub(crate) fn disable_wakeup() {
    WAKEUP.store(-1, Ordering::SeqCst);
    while WRITERS.load(Ordering::SeqCst) != 0 {
        std::thread::yield_now();
    }
}

/// Release only the signal dispositions the main interpreter took ownership of.
pub(crate) fn finalize() {
    if is_main() {
        disable_wakeup();
        for number in 1..nsig() {
            if DISPOSITIONS[number as usize].load(Ordering::Acquire) == PYTHON {
                let _ = install(number, Disposition::Default);
            }
        }
    }
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

/// Kernel-pending signals are not inherited. Clear their Python counterparts
/// while delivery is blocked, before any new signal can be recorded in the child.
#[cfg(target_os = "linux")]
pub(crate) fn clear_pending_after_fork() {
    ERROR_HEAD.store(0, Ordering::Relaxed);
    ERROR_TAIL.store(0, Ordering::Relaxed);
    WRITERS.store(0, Ordering::Relaxed);
    for slot in &WAKEUP_ERRORS {
        slot.store(0, Ordering::Relaxed);
    }
    for bit in &PENDING {
        bit.store(false, Ordering::Relaxed);
    }
    SIGNALS_PENDING.store(false, Ordering::Release);
    TRIPPED.store(false, Ordering::Release);
}

pub(crate) fn begin_dispatch() -> bool {
    TRIPPED.load(Ordering::Acquire) && is_main() && TRIPPED.swap(false, Ordering::AcqRel)
}

pub(crate) fn begin_signal_dispatch() -> bool {
    SIGNALS_PENDING.load(Ordering::Acquire)
        && is_main()
        && SIGNALS_PENDING.swap(false, Ordering::AcqRel)
}

pub(crate) fn resume_signals() {
    // Multiple threads can publish different signal numbers before the main thread scans them.
    SIGNALS_PENDING.fetch_or(true, Ordering::Release);
    resume_dispatch();
}

pub(crate) fn resume_dispatch() {
    // Carry each publisher's signal or wakeup error into the same acquiring checkpoint.
    TRIPPED.fetch_or(true, Ordering::Release);
}
pub(crate) fn take(number: i32) -> bool {
    PENDING[number as usize].swap(false, Ordering::AcqRel)
}
pub(crate) fn take_interrupt() -> bool {
    if !begin_dispatch() {
        return false;
    }
    let interrupted = begin_signal_dispatch() && {
        let interrupt = take(libc::SIGINT);
        if PENDING.iter().any(|bit| bit.load(Ordering::Acquire)) {
            resume_signals();
        }
        interrupt
    };
    if has_wakeup_errors() {
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
