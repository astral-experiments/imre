//! Tell `imre run` when this image has loaded, so it can unlink its executable.

use std::ffi::CStr;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;

const RUN_PREFIX: &[u8] = b".imre-run-fd-";
const EXEC_PREFIX: &[u8] = b".imre-exec-fd-";

pub(crate) fn wait_for_cleanup(want_original: bool) -> io::Result<Option<Vec<u8>>> {
    #[cfg(target_os = "linux")]
    // SAFETY: AT_EXECFN points to the kernel-provided executable pathname.
    let path = unsafe { libc::getauxval(libc::AT_EXECFN) } as *const libc::c_char;
    #[cfg(target_os = "macos")]
    // SAFETY: dyld image zero is the main executable and remains loaded.
    let path = unsafe { libc::_dyld_get_image_name(0) };
    if path.is_null() {
        return Ok(None);
    }
    // SAFETY: the loader owns this NUL-terminated string throughout startup.
    let path = unsafe { CStr::from_ptr(path) }.to_bytes();
    let name = path.rsplit(|&byte| byte == b'/').next().unwrap_or_default();
    let (digits, exec) = if let Some(digits) = name.strip_prefix(RUN_PREFIX) {
        (digits, false)
    } else if let Some(digits) = name.strip_prefix(EXEC_PREFIX) {
        (digits, true)
    } else {
        return Ok(None);
    };
    if digits.len() > 10 || !digits.iter().all(u8::is_ascii_digit) {
        return Ok(None);
    }
    let Some(fd) = std::str::from_utf8(digits)
        .ok()
        .and_then(|digits| digits.parse::<RawFd>().ok())
    else {
        return Ok(None);
    };
    if fd < 3 {
        return Ok(None);
    }

    // A standalone program can have this basename and unrelated inherited
    // descriptors. Peek one byte beyond the longest valid challenge so a
    // longer message sharing its prefix cannot authenticate.
    let mut pid_bytes = [0; size_of::<libc::pid_t>()];
    let expected = name.len() + if exec { pid_bytes.len() } else { 0 };
    let mut challenge = [0; EXEC_PREFIX.len() + 10 + size_of::<libc::pid_t>() + 1];
    // SAFETY: recv checks the descriptor and writes only into this buffer.
    let received = retry_interrupted(|| unsafe {
        libc::recv(
            fd,
            challenge.as_mut_ptr().cast(),
            challenge.len(),
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    });
    if received != Some(expected) || &challenge[..name.len()] != name || !is_unix_stream(fd) {
        return Ok(None);
    }

    let helper = if exec {
        pid_bytes.copy_from_slice(&challenge[name.len()..expected]);
        let pid = libc::pid_t::from_ne_bytes(pid_bytes);
        if pid <= 0 {
            return Ok(None);
        }
        Some(pid)
    } else {
        None
    };
    // SAFETY: the matching launcher challenge transfers this connected Unix
    // stream to us. Dropping it closes the inherited fd before user code runs.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let result = acknowledge(stream, &mut challenge[..expected], want_original);
    if let Some(pid) = helper {
        // The socket has been closed even on error, so the helper can always
        // exit. Reap this exact child before user code observes its children.
        loop {
            let mut status = 0;
            // SAFETY: pid is positive and status is writable.
            if unsafe { libc::waitpid(pid, &mut status, 0) } >= 0 {
                if status != 0 {
                    return Err(io::Error::other("could not clean up the temporary image"));
                }
                break;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                break; // Automatically reaped with inherited SIGCHLD=SIG_IGN.
            }
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    result
}

fn acknowledge(
    mut stream: UnixStream,
    challenge: &mut [u8],
    want_original: bool,
) -> io::Result<Option<Vec<u8>>> {
    let fd = stream.as_raw_fd();
    if stream.read_exact(challenge).is_err() {
        return Ok(None);
    }
    // SAFETY: the ready byte is readable, the stream owns fd, and NOSIGNAL
    // lets an early launcher exit produce EPIPE without killing the program.
    if retry_interrupted(|| unsafe {
        libc::send(
            fd,
            [if want_original { 2u8 } else { 1 }].as_ptr().cast(),
            1,
            libc::MSG_NOSIGNAL,
        )
    }) == Some(1)
    {
        // The launcher closes its endpoint after removing the temporary tree.
        if want_original {
            // A real exec cannot contain more argument bytes than ARG_MAX.
            // Bound allocation even if an unrelated process mimics the handshake.
            // SAFETY: sysconf reads this process's exec limit without pointers.
            let max = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
            let limit = u64::try_from(max).unwrap_or(64 * 1024 * 1024);
            let mut bytes = Vec::new();
            stream.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "launcher arguments exceed ARG_MAX",
                ));
            }
            // A launcher predating the argument protocol sends no payload.
            return Ok((!bytes.is_empty()).then_some(bytes));
        }
        io::copy(&mut stream, &mut io::sink())?;
    }
    Ok(None)
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
