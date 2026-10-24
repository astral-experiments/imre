//! Tell `relar run` when this image has loaded, so it can unlink its executable.

use std::ffi::CStr;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;

const PREFIX: &[u8] = b".relar-run-fd-";

pub(crate) fn wait_for_cleanup() {
    #[cfg(target_os = "linux")]
    // SAFETY: AT_EXECFN points to the kernel-provided executable pathname.
    let path = unsafe { libc::getauxval(libc::AT_EXECFN) } as *const libc::c_char;
    #[cfg(target_os = "macos")]
    // SAFETY: dyld image zero is the main executable and remains loaded.
    let path = unsafe { libc::_dyld_get_image_name(0) };
    if path.is_null() {
        return;
    }
    // SAFETY: the loader owns this NUL-terminated string throughout startup.
    let path = unsafe { CStr::from_ptr(path) }.to_bytes();
    let name = path.rsplit(|&byte| byte == b'/').next().unwrap_or_default();
    let Some(digits) = name.strip_prefix(PREFIX) else {
        return;
    };
    if digits.len() > 10 || !digits.iter().all(u8::is_ascii_digit) {
        return;
    }
    let Some(fd) = std::str::from_utf8(digits)
        .ok()
        .and_then(|digits| digits.parse::<RawFd>().ok())
    else {
        return;
    };
    if fd < 3 {
        return;
    }

    // A standalone program can have this basename and unrelated inherited
    // descriptors. Peek one byte beyond the longest valid challenge so a
    // longer message sharing its prefix cannot authenticate.
    let mut challenge = [0; PREFIX.len() + 10 + 1];
    // SAFETY: recv checks the descriptor and writes only into this buffer.
    let received = retry_interrupted(|| unsafe {
        libc::recv(
            fd,
            challenge.as_mut_ptr().cast(),
            challenge.len(),
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    });
    if received != Some(name.len()) || &challenge[..name.len()] != name || !is_unix_stream(fd) {
        return;
    }

    // SAFETY: the matching launcher challenge transfers this connected Unix
    // stream to us. Dropping it closes the inherited fd before user code runs.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    if stream.read_exact(&mut challenge[..name.len()]).is_err() {
        return;
    }
    // SAFETY: the ready byte is readable, the stream owns fd, and NOSIGNAL
    // lets an early launcher exit produce EPIPE without killing the program.
    if retry_interrupted(|| unsafe { libc::send(fd, [1u8].as_ptr().cast(), 1, libc::MSG_NOSIGNAL) })
        == Some(1)
    {
        // The launcher closes its endpoint after removing the temporary tree.
        let _ = io::copy(&mut stream, &mut io::sink());
    }
}

fn is_unix_stream(fd: RawFd) -> bool {
    let mut kind: libc::c_int = 0;
    let mut len = size_of_val(&kind) as libc::socklen_t;
    // SAFETY: both output pointers name writable storage of the given sizes.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut kind).cast(),
            &mut len,
        )
    } != 0
        || kind != libc::SOCK_STREAM
    {
        return false;
    }
    // SAFETY: zero is a valid initial representation for an address buffer.
    let mut peer: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = size_of_val(&peer) as libc::socklen_t;
    // SAFETY: getpeername writes at most len bytes into peer.
    unsafe {
        libc::getpeername(fd, (&raw mut peer).cast(), &mut len) == 0
            && libc::c_int::from(peer.ss_family) == libc::AF_UNIX
    }
}

fn retry_interrupted(mut call: impl FnMut() -> libc::ssize_t) -> Option<usize> {
    loop {
        let result = call();
        if result >= 0 {
            return Some(result as usize);
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return None;
        }
    }
}
