//! Publish owned frame metadata before the platform's fatal signal handler needs it.
use std::ffi::c_int;
use std::io;

pub(crate) const MAX_FRAMES: usize = 100;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[repr(C)]
struct PreviousPublication {
    slot: *mut std::ffi::c_void,
    interpreter: *const std::sync::atomic::AtomicBool,
}

/// The existing interpreter shutdown token is shared with its native Python workers.
pub(crate) struct FramePublication<'a> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    previous: PreviousPublication,
    _interpreter: &'a std::sync::atomic::AtomicBool,
}

impl<'a> FramePublication<'a> {
    pub(crate) fn enter(interpreter: &'a std::sync::atomic::AtomicBool) -> Self {
        Self {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            // SAFETY: The token allocation lives until this thread leaves the publication.
            previous: unsafe { imre_fault_enter(interpreter) },
            _interpreter: interpreter,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for FramePublication<'_> {
    fn drop(&mut self) {
        // SAFETY: Runtime entry scopes unwind in order, on the entering pthread.
        unsafe { imre_fault_leave(&self.previous) }
    }
}

unsafe extern "C" {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn imre_fault_enter(interpreter: *const std::sync::atomic::AtomicBool) -> PreviousPublication;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn imre_fault_leave(previous: &PreviousPublication);
    fn imre_fault_enable(fd: c_int, all_threads: c_int, c_stack: c_int) -> c_int;
    fn imre_fault_disable() -> c_int;
    fn imre_fault_enabled() -> c_int;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn imre_fault_register(signal: c_int, fd: c_int, all_threads: c_int, chain: c_int) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn imre_fault_unregister(signal: c_int) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn imre_fault_unregister_all();
    fn imre_fault_generation() -> u32;
    fn imre_fault_frame(
        index: usize,
        file: *const u8,
        file_len: usize,
        name: *const u8,
        name_len: usize,
        line: u32,
    );
    fn imre_fault_line(index: usize, line: u32);
    fn imre_fault_truncate(depth: usize);
    fn imre_fault_dump(fd: c_int, all_threads: c_int);
}

pub(crate) fn enable(fd: i32, all_threads: bool, c_stack: bool) -> io::Result<()> {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return Err(io::ErrorKind::Unsupported.into());
    }
    // SAFETY: Only integer configuration is passed. The descriptor remains owned by the caller.
    if unsafe { imre_fault_enable(fd, all_threads.into(), c_stack.into()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn disable() -> bool {
    // SAFETY: The platform implementation serializes disposition changes.
    unsafe { imre_fault_disable() != 0 }
}

pub(crate) fn generation() -> u32 {
    // SAFETY: This reads a lock-free process-wide atomic.
    unsafe { imre_fault_generation() }
}

pub(crate) fn is_enabled() -> bool {
    // SAFETY: This reads the fatal-handler flag, independently of user registrations.
    unsafe { imre_fault_enabled() != 0 }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn register(signal: i32, fd: i32, all_threads: bool, chain: bool) -> io::Result<()> {
    // SAFETY: Only integer settings cross this boundary. The descriptor is borrowed.
    if unsafe { imre_fault_register(signal, fd, all_threads.into(), chain.into()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn unregister(signal: i32) -> io::Result<bool> {
    // SAFETY: The native registry restores the saved disposition under its configuration lock.
    match unsafe { imre_fault_unregister(signal) } {
        -1 => Err(io::Error::last_os_error()),
        changed => Ok(changed != 0),
    }
}

pub(crate) fn unregister_all() {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    // SAFETY: Called at interpreter teardown before its signal dispositions are finalized.
    unsafe {
        imre_fault_unregister_all();
    }
}

pub(crate) fn frame(index: usize, filename: &str, function: &str, line: u32) {
    // SAFETY: The native implementation copies both live slices during this call.
    unsafe {
        imre_fault_frame(
            index,
            filename.as_ptr(),
            filename.len(),
            function.as_ptr(),
            function.len(),
            line,
        )
    }
}

pub(crate) fn line(index: usize, line: u32) {
    // SAFETY: Index and line are values in the calling thread's frame publication.
    unsafe { imre_fault_line(index, line) }
}

pub(crate) fn truncate(depth: usize) {
    // SAFETY: Only the calling thread's publication is shortened.
    unsafe { imre_fault_truncate(depth) }
}

pub(crate) fn dump_traceback(fd: i32, all_threads: bool) {
    // SAFETY: The reporter writes to but does not own this descriptor.
    unsafe { imre_fault_dump(fd, all_threads.into()) }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
