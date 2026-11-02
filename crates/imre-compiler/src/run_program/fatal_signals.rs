//! Exec the Python image in the caller's process. Its short-lived helper owns
//! only the temporary image and the startup handshake.
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};

use tempfile::TempDir;

pub(super) fn exec(
    mut program: Command,
    directory: TempDir,
    name: String,
    mut control: UnixStream,
    peer: UnixStream,
) -> io::Result<ExitStatus> {
    let image = CString::new(program.get_program().as_bytes())?;
    let dir = CString::new(directory.path().as_os_str().as_bytes())?;
    let mut argv = Vec::new();
    for arg in std::env::args_os() {
        argv.extend_from_slice(arg.as_bytes());
        argv.push(0);
    }
    let fd = control.as_raw_fd();
    let peer_fd = peer.as_raw_fd();
    // A fatal signal to the process group immediately after fork must not
    // kill the cleanup child. Only the execing thread restores its old mask.
    // SAFETY: all three signal sets are initialized before they are read.
    let mut previous = unsafe { std::mem::zeroed() };
    let blocked = unsafe {
        let mut mask = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        for sig in SIGNALS {
            libc::sigaddset(&mut mask, sig);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut previous)
    };
    if blocked != 0 {
        return Err(io::Error::from_raw_os_error(blocked));
    }
    // SAFETY: the child uses only the inherited buffers and async-signal-safe
    // syscalls before _exit. No allocation, locks or destructors after fork.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: the child owns its descriptor table and never returns.
        unsafe {
            libc::close(peer_fd);
            // Group signals directed at the Python process must not prevent cleanup.
            libc::setpgid(0, 0);
        }
        cleanup(fd, &image, &dir, &argv);
    }

    let fork_error = (pid < 0).then(io::Error::last_os_error);
    // SAFETY: previous is the mask saved by the successful pthread_sigmask.
    let restored =
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };
    if let Some(error) = fork_error {
        return Err(error);
    }

    // The helper unlinks exactly our image and removes the directory if empty,
    // even when exec fails or the parent disappears before acknowledgment.
    let _ = directory.keep();
    let mut challenge = name.into_bytes();
    challenge.extend_from_slice(&pid.to_ne_bytes());
    let sent = if restored == 0 {
        control.write_all(&challenge)
    } else {
        Err(io::Error::from_raw_os_error(restored))
    };
    drop(control);
    let error = match sent {
        Ok(()) => program.exec(),
        Err(e) => e,
    };
    drop(peer);
    // Failed exec. Closing the socket above lets the helper finish before we return.
    while unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) } < 0
        && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
    {}
    Err(error)
}

fn cleanup(fd: RawFd, image: &CStr, dir: &CStr, argv: &[u8]) -> ! {
    let mut ready = 0u8;
    // SAFETY: the pointers and slices were prepared before fork. The image is
    // no longer needed once its runtime acknowledges loading or closes its fd.
    unsafe {
        while libc::read(fd, (&raw mut ready).cast(), 1) < 0
            && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {}
        let mut errno: libc::c_int = 0;
        if libc::unlink(image.as_ptr()) != 0 {
            errno = io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
        }
        if libc::rmdir(dir.as_ptr()) != 0 && errno == 0 {
            errno = io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
        }
        if matches!(ready, 1 | 2) {
            send_all(fd, &errno.to_ne_bytes());
            if errno == 0 && ready == 2 {
                send_all(fd, &(argv.len() as u64).to_ne_bytes());
                send_all(fd, argv);
            }
        }
        libc::_exit(i32::from(errno != 0));
    }
}

// Called only in the helper, where _exit closes the socket on error.
fn send_all(fd: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: the slice stays live throughout send, including retries.
        let n = unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL) };
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 {
            unsafe { libc::_exit(1) };
        }
        bytes = &bytes[n as usize..];
    }
}

const SIGNALS: [i32; 5] = [
    libc::SIGABRT,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGILL,
    libc::SIGSEGV,
];

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
