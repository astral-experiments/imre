//! Publish owned frame metadata before the platform's fatal signal handler needs it.
use std::ffi::c_int;
use std::io;

pub(crate) const MAX_FRAMES: usize = 100;

unsafe extern "C" {
    fn imre_fault_enable(fd: c_int, all_threads: c_int, c_stack: c_int) -> c_int;
    fn imre_fault_disable() -> c_int;
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
    generation() != 0
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
