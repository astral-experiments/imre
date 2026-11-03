//! Fork and report setup errors through a close-on-exec pipe. Only a guarded
//! interpreter fork may run a guest callback in the child.
use super::*;
use std::os::fd::IntoRawFd;

pub(super) enum PreExecError {
    Fork(Error),
    Io(io::Error),
}

impl From<io::Error> for PreExecError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Start the child and return its PID and error reader before waiting for exec.
pub(super) fn spawn(
    mut launch: Launch,
    cwd: Option<&CStr>,
    streams: u32,
    fork_launch: popen::ForkLaunch<'_>,
    child_pipes: Option<popen::ChildPipes>,
    attributes: &ChildAttributes,
) -> std::result::Result<libc::pid_t, PreExecError> {
    let popen::ForkLaunch {
        fork_exec: ForkExec {
            mut callback,
            reader,
        },
        child_fds: stdio,
        parent_fds,
    } = fork_launch;
    let has_callback = callback.is_some();
    if has_callback && !cfg!(all(target_os = "linux", target_env = "gnu")) {
        return Err(PreExecError::Fork(Error::new(
            Kind::NotImplementedError,
            "subprocess preexec_fn requires a provably single-threaded glibc process",
        )));
    }
    // Do this before opening the error pipe. It must not revive an invalid fd.
    for &fd in stdio.iter().chain(&parent_fds).chain(&attributes.keep) {
        if fd != -1 {
            // SAFETY: F_GETFD validates the number without consuming or changing it.
            if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
    }
    let (read, write) = io::pipe()?;
    let write: OwnedFd = write.into();
    let write = if write.as_raw_fd() < 3 {
        // SAFETY: the live writer is duplicated into a descriptor unaffected by stdio setup.
        let fd = unsafe { libc::fcntl(write.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: fcntl returned a fresh descriptor. Release the original low fd now.
        let high = unsafe { OwnedFd::from_raw_fd(fd) };
        drop(write);
        high
    } else {
        write
    };
    let error_fd = write.as_raw_fd();
    let mut keep = Vec::new();
    if has_callback {
        keep.try_reserve_exact(attributes.keep.len() + 1)
            .map_err(|_| PreExecError::Fork(Error::allocation()))?;
        keep.extend_from_slice(&attributes.keep);
        keep.insert(keep.partition_point(|fd| *fd < error_fd), error_fd);
    }

    let environment = (!has_callback).then(|| ENVIRONMENT.lock().unwrap());
    let pid = if let Some(callback) = &mut callback {
        callback.fork().map_err(PreExecError::Fork)?
    } else {
        // SAFETY: the environment lock excludes concurrent changes at fork. The
        // child only executes native setup and never drops the inherited guard.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(io::Error::last_os_error().into());
        }
        pid
    };
    if pid == 0 {
        let read_fd = read.into_raw_fd();
        let error_fd = write.into_raw_fd();
        // The child must never unwind back into copied parent frames and drop
        // descriptor owners after the callback may have reused their numbers.
        let run = || -> io::Result<()> {
            if has_callback {
                // SAFETY: the callback must see pass_fds as inheritable on entry.
                unsafe { pass_fds::inherit(&attributes.keep)? };
            }
            for fd in parent_fds.into_iter().chain([read_fd]) {
                if fd >= 0 {
                    // SAFETY: these are the unused parent ends in the copied process.
                    if unsafe { libc::close(fd) } < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
            }
            redirect_stdio(stdio)?;
            if let Some(cwd) = cwd {
                // SAFETY: cwd is a validated, live C string. Only the child changes directory.
                if unsafe { libc::chdir(cwd.as_ptr()) } < 0 {
                    let errno = io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO);
                    return Err(launch_error::child_errno(
                        launch_error::Site::Directory,
                        errno,
                    ));
                }
            }
            // SAFETY: only the forked child changes its process state.
            unsafe { Launch::configure_child(attributes, streams & (1 << 20) == 0, None)? };
            let Some(callback) = callback else {
                // SAFETY: stdio, cwd and attributes are configured. Only native
                // descriptor operations and exec remain on this path.
                return unsafe { launch.exec_child(streams & (1 << 17) == 0, &attributes.keep) };
            };
            if !callback.invoke() {
                return Ok(());
            }
            if streams & (1 << 17) == 0 {
                close_fds(&keep)?;
            }
            // SAFETY: the callback has returned and all child settings are complete.
            Err(unsafe { launch.exec_image() })
        };
        let result = if has_callback {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        } else {
            Ok(run())
        };
        // A zero record means the callback raised. OS errors use the same signed
        // errno encoding for chdir and exec as the regular Command path.
        let code = match result {
            Ok(Ok(())) => 0,
            Ok(Err(e)) => e.raw_os_error().unwrap_or(libc::EIO),
            Err(panic) => {
                std::mem::forget(panic);
                libc::EIO
            }
        };
        let bytes = code.to_ne_bytes();
        loop {
            // SAFETY: the fixed-size write fits in PIPE_BUF and the slice is live.
            if unsafe { libc::write(error_fd, bytes.as_ptr().cast(), bytes.len()) } >= 0
                || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
            {
                break;
            }
        }
        // SAFETY: never run parent-side destructors or flush copied Python streams.
        unsafe { libc::_exit(255) };
    }
    drop(environment);
    drop(write);
    // A callback can wait for EOF from a redirected pipe before it returns.
    // Release our copies of all child endpoints before waiting for exec.
    drop(child_pipes);
    *reader = Some(read);
    Ok(pid)
}

/// Install all streams from the original fd table, including swaps of 0, 1 and 2.
fn redirect_stdio(mut stdio: [RawFd; 3]) -> io::Result<()> {
    for (target, source) in stdio.iter_mut().enumerate() {
        if *source >= 0 && *source < target as RawFd {
            // SAFETY: preserve an earlier standard fd that dup2 might overwrite.
            *source = unsafe { libc::fcntl(*source, libc::F_DUPFD_CLOEXEC, 3) };
            if *source < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    for (target, source) in stdio.into_iter().enumerate() {
        let target = target as RawFd;
        if source == target {
            // SAFETY: a dup2 no-op still needs to clear CLOEXEC.
            unsafe { pass_fds::inherit(&[target])? };
        } else if source != -1 {
            // SAFETY: dup2 targets a standard descriptor in the child.
            if unsafe { libc::dup2(source, target) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// Close the live descriptor table, including fds the callback just opened.
/// Keep the (still CLOEXEC) error writer and preserve the callback's final flags on pass_fds.
#[cfg(target_os = "linux")]
fn close_fds(keep: &[RawFd]) -> io::Result<()> {
    let mut first = 3;
    for last in keep
        .iter()
        .copied()
        .filter(|fd| *fd >= 3)
        .map(|fd| fd as u32)
        .chain([u32::MAX])
    {
        if first < last {
            // SAFETY: these inclusive ranges lie between retained fds in the child.
            let rc = unsafe { libc::syscall(libc::SYS_close_range, first, last - 1, 0) };
            if rc < 0 {
                // A kernel or sandbox may reject close_range while still
                // allowing ordinary closes. Enumerate the child's live fds.
                return close_from_proc(keep);
            }
        }
        first = last.saturating_add(1);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn close_from_proc(keep: &[RawFd]) -> io::Result<()> {
    // SAFETY: the directory stream is used only by this child and closed below.
    let dir = unsafe { libc::opendir(c"/proc/self/fd".as_ptr()) };
    if dir.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: dir is a live directory stream.
    let dir_fd = unsafe { libc::dirfd(dir) };
    loop {
        // SAFETY: clear this thread's errno to distinguish end of stream from an error.
        let entry = unsafe {
            *libc::__errno_location() = 0;
            libc::readdir(dir)
        };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: the directory stream and its descriptor are still ours.
            unsafe { libc::closedir(dir) };
            return if error.raw_os_error() == Some(0) {
                Ok(())
            } else {
                Err(error)
            };
        }
        // SAFETY: readdir supplies a NUL-terminated name until the next operation.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if let Some(fd) = name.to_str().ok().and_then(|s| s.parse::<RawFd>().ok())
            && fd >= 3
            && fd != dir_fd
            && keep.binary_search(&fd).is_err()
        {
            // SAFETY: this is the child and fd is not retained. Do not retry close.
            unsafe { libc::close(fd) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn close_fds(_: &[RawFd]) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOSYS))
}
