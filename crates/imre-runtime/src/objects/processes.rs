//! Sealed synchronous Unix processes with inherited, redirected, or captured streams.
//!
//! Native launch data owns its bytes before fork. The child performs only
//! async-signal-safe descriptor, signal, and exec operations; language owners
//! remain in the parent. All requested pipes are drained before decoding or check.

use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::exceptions::{Error, Kind, Result, finish, raise_process, raise_timeout};
use super::files::{decode_utf8_text, os_error};
use super::mappings::{imre_dict_contains, imre_dict_new, imre_dict_set};
use super::records::{RecordDescriptor, imre_record_new};
use super::words::{OwnedWord, WordKind};
use super::{Bytes, Int, Object, Storage, Str, acquire_rc, fail, object};

mod asynchronous;
pub(super) use asynchronous::AsyncProcess;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    static environ: *const *const libc::c_char;
}

/// Rich failures keep the original argv list, so later aliases affect str(error).
/// Its sealed string and path elements cannot point back to the exception.
#[derive(Clone)]
pub(super) struct ProcessError {
    cmd: Rc<Object>,
    returncode: i32,
    stdout: OwnedWord,
    stderr: OwnedWord,
}

impl ProcessError {
    pub(super) fn field(&self, field: u32, tagged: bool) -> usize {
        match field {
            0 => Rc::into_raw(self.cmd.clone()) as usize,
            1 => exit_word(self.returncode),
            2..=4 => {
                let output = if field == 3 {
                    &self.stderr
                } else {
                    &self.stdout
                };
                // SAFETY: the exception owns the immutable word with this exact kind.
                let word = unsafe { output.kind.acquire(output.word) }.into_word();
                if tagged && output.kind == WordKind::new(8) {
                    word | 1
                } else {
                    word
                }
            }
            _ => fail("SystemError", "invalid process exception field"),
        }
    }

    pub(super) fn message(&self) -> Str {
        let mut cmd = String::new();
        // SAFETY: this owner roots the original acyclic argv list during repr.
        unsafe {
            super::values::write_repr(&mut cmd, Rc::as_ptr(&self.cmd) as usize, WordKind::new(5));
        }
        if self.returncode < 0 {
            let signal = -self.returncode;
            if let Some(name) = signal_name(signal) {
                Str::from(format!(
                    "Command '{cmd}' died with <Signals.{name}: {signal}>."
                ))
            } else {
                Str::from(format!(
                    "Command '{cmd}' died with unknown signal {signal}."
                ))
            }
        } else {
            Str::from(format!(
                "Command '{cmd}' returned non-zero exit status {}.",
                self.returncode
            ))
        }
    }
}

#[derive(Clone)]
enum Deadline {
    Integer(Int),
    Float(f64),
}

/// A deadline failure owns the original argv and undecoded pipe snapshots.
/// Empty streams distinguish an observed EOF from no data observed before expiry.
#[derive(Clone)]
pub(super) struct TimeoutError {
    cmd: Rc<Object>,
    timeout: Deadline,
    stdout: Option<Rc<Bytes>>,
    stderr: Option<Rc<Bytes>>,
}

impl TimeoutError {
    /// The compiler proves the numeric field's exact kind at every read.
    pub(super) fn field(&self, field: u32) -> usize {
        match field {
            0 => Rc::into_raw(self.cmd.clone()) as usize,
            1 => match &self.timeout {
                Deadline::Integer(seconds) => seconds.clone().into_raw(),
                Deadline::Float(seconds) => seconds.to_bits() as usize,
            },
            2 | 4 => self
                .stdout
                .as_ref()
                .map_or(0, |value| Rc::into_raw(value.clone()) as usize),
            3 => self
                .stderr
                .as_ref()
                .map_or(0, |value| Rc::into_raw(value.clone()) as usize),
            _ => fail("SystemError", "invalid timeout exception field"),
        }
    }

    /// Formatting observes later mutations of the same rooted argv list.
    pub(super) fn message(&self) -> Str {
        let mut cmd = String::new();
        // SAFETY: this owner roots an exact acyclic list[str] during repr.
        unsafe {
            super::values::write_repr(&mut cmd, Rc::as_ptr(&self.cmd) as usize, WordKind::new(5));
        }
        let seconds = match &self.timeout {
            Deadline::Integer(seconds) => Str::from(seconds.big().to_string()),
            Deadline::Float(seconds) => crate::floats::render(*seconds),
        };
        Str::from(format!("Command '{cmd}' timed out after {seconds} seconds"))
    }
}

/// Python formats a child's terminating signal with the host's signal number.
fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGUSR2 => "SIGUSR2",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        libc::SIGCHLD => "SIGCHLD",
        libc::SIGCONT => "SIGCONT",
        libc::SIGSTOP => "SIGSTOP",
        libc::SIGTSTP => "SIGTSTP",
        libc::SIGTTIN => "SIGTTIN",
        libc::SIGTTOU => "SIGTTOU",
        libc::SIGURG => "SIGURG",
        libc::SIGXCPU => "SIGXCPU",
        libc::SIGXFSZ => "SIGXFSZ",
        libc::SIGVTALRM => "SIGVTALRM",
        libc::SIGPROF => "SIGPROF",
        libc::SIGWINCH => "SIGWINCH",
        libc::SIGIO => "SIGIO",
        libc::SIGSYS => "SIGSYS",
        #[cfg(target_os = "macos")]
        libc::SIGEMT => "SIGEMT",
        #[cfg(target_os = "macos")]
        libc::SIGINFO => "SIGINFO",
        #[cfg(all(
            target_os = "linux",
            not(any(
                target_arch = "mips64",
                target_arch = "mips64r6",
                target_arch = "sparc64"
            ))
        ))]
        libc::SIGSTKFLT => "SIGSTKFLT",
        #[cfg(target_os = "linux")]
        libc::SIGPWR => "SIGPWR",
        #[cfg(target_os = "linux")]
        value if value == libc::SIGRTMIN() => "SIGRTMIN",
        #[cfg(target_os = "linux")]
        value if value == libc::SIGRTMAX() => "SIGRTMAX",
        _ => return None,
    })
}

fn exit_word(returncode: i32) -> usize {
    // Every signed 32-bit exit code fits the signed 63-bit small representation.
    ((returncode as usize) << 1) | 1
}

fn cstring(bytes: &[u8]) -> Result<CString> {
    CString::new(bytes).map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))
}

/// Self-contained immutable C arrays: moving the owner never moves CString data.
struct CStrings {
    strings: Vec<CString>,
    pointers: Vec<*const libc::c_char>,
}

// SAFETY: pointers refer only to immutable allocations owned by `strings`; the
// array is terminated, never mutated, and cannot outlive those allocations.
unsafe impl Send for CStrings {}
// SAFETY: shared access only reads the same immutable owned strings and pointers.
unsafe impl Sync for CStrings {}

impl CStrings {
    fn new(strings: Vec<CString>) -> Self {
        let mut pointers = strings
            .iter()
            .map(|value| value.as_ptr())
            .collect::<Vec<_>>();
        pointers.push(std::ptr::null());
        Self { strings, pointers }
    }
}

enum Descriptors {
    #[cfg(target_os = "linux")]
    Range,
    Snapshot(Vec<RawFd>),
}

impl Descriptors {
    /// Snapshot open descriptors before fork when close_range is unavailable.
    /// The snapshot includes open fds above a lowered RLIMIT_NOFILE.
    fn prepare() -> io::Result<Self> {
        // SAFETY: UINT_MAX exceeds every valid signed RawFd, so the probe changes
        // no descriptor. Only support for the CLOEXEC flag is being tested.
        #[cfg(target_os = "linux")]
        if unsafe {
            libc::syscall(
                libc::SYS_close_range,
                u32::MAX,
                u32::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        } == 0
        {
            return Ok(Self::Range);
        }
        let mut descriptors = Vec::new();
        let fd_dir = if cfg!(target_os = "macos") {
            "/dev/fd"
        } else {
            "/proc/self/fd"
        };
        for entry in fs::read_dir(fd_dir)? {
            let name = entry?.file_name();
            if let Some(fd) = name.to_str().and_then(|name| name.parse::<RawFd>().ok())
                && fd >= 3
            {
                descriptors.push(fd);
            }
        }
        Ok(Self::Snapshot(descriptors))
    }

    /// Marks inherited descriptors for exec without closing Command's error pipe.
    ///
    /// # Safety
    /// Call only in the forked child after stdio duplication. No callback or
    /// allocator operation may run before exec; the snapshot remains immutable.
    unsafe fn apply(&self) -> io::Result<()> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Range => {
                // SAFETY: child descriptors >=3 must close on successful exec;
                // retaining them until then preserves spawn's error reporting.
                if unsafe {
                    libc::syscall(
                        libc::SYS_close_range,
                        3_u32,
                        u32::MAX,
                        libc::CLOSE_RANGE_CLOEXEC,
                    )
                } != 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            Self::Snapshot(descriptors) => {
                for &fd in descriptors {
                    // SAFETY: fcntl does not allocate; a closed snapshot entry is
                    // harmless. Any newly created Command pipe already has CLOEXEC.
                    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
                    if flags < 0 {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::EBADF) {
                            continue;
                        }
                        return Err(error);
                    }
                    // SAFETY: the live child fd is marked without closing it.
                    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
            }
        }
        Ok(())
    }
}

struct Launch {
    argv: CStrings,
    env: Option<CStrings>,
    candidates: Vec<CString>,
    descriptors: Descriptors,
}

impl Launch {
    /// Prepares native launch data after validating nonempty argv and env names.
    fn new(argv: Vec<CString>, env: Option<Vec<(Str, Str)>>) -> Result<Self> {
        let program = &argv[0];
        let path = if let Some(env) = &env {
            env.iter()
                .find(|(key, _)| key == "PATH")
                .map(|(_, value)| value.as_bytes().to_vec())
        } else {
            std::env::var_os("PATH").map(|path| path.as_bytes().to_vec())
        }
        .unwrap_or_else(|| b"/bin:/usr/bin".to_vec());
        let env = env
            .map(|env| {
                env.into_iter()
                    .map(|(key, value)| {
                        let mut entry = Vec::with_capacity(key.len() + value.len() + 1);
                        entry.extend_from_slice(key.as_bytes());
                        entry.push(b'=');
                        entry.extend_from_slice(value.as_bytes());
                        cstring(&entry)
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(CStrings::new)
            })
            .transpose()?;
        let candidates = if program.as_bytes().contains(&b'/') {
            vec![program.clone()]
        } else {
            path.split(|&byte| byte == b':')
                .map(|directory| {
                    let mut candidate =
                        Vec::with_capacity(directory.len() + program.as_bytes().len() + 1);
                    candidate.extend_from_slice(directory);
                    if !directory.is_empty() {
                        candidate.push(b'/');
                    }
                    candidate.extend_from_slice(program.as_bytes());
                    cstring(&candidate)
                })
                .collect::<Result<Vec<_>>>()?
        };
        Ok(Self {
            argv: CStrings::new(argv),
            env,
            candidates,
            descriptors: Descriptors::prepare().map_err(os_error)?,
        })
    }

    /// Performs CPython's direct exec search, never libc execvp's shell fallback.
    /// Returning an error lets Command report errno through its private pipe.
    ///
    /// # Safety
    /// Call only from Command::pre_exec after stdio and cwd setup. Every referenced
    /// buffer was allocated in the parent; only async-signal-safe libc operations
    /// and allocation-free OS-error construction are allowed until exec or _exit.
    unsafe fn exec_child(&self) -> io::Result<()> {
        for signal in [libc::SIGPIPE, libc::SIGXFSZ] {
            // SAFETY: restore_signals resets dispositions Python ignores; signal
            // is async-signal-safe and both arguments are valid signal constants.
            if unsafe { libc::signal(signal, libc::SIG_DFL) } == libc::SIG_ERR {
                return Err(io::Error::last_os_error());
            }
        }
        // SAFETY: this is the forked child after Command's stdio setup.
        unsafe { self.descriptors.apply()? };
        let mut saved_error = None;
        let mut last_error = libc::ENOENT;
        for executable in &self.candidates {
            // SAFETY: the immutable C arrays are NUL-terminated and live through
            // exec. None preserves the inherited environment exactly as bytes.
            unsafe {
                if let Some(env) = &self.env {
                    libc::execve(
                        executable.as_ptr(),
                        self.argv.pointers.as_ptr(),
                        env.pointers.as_ptr(),
                    );
                } else {
                    libc::execv(executable.as_ptr(), self.argv.pointers.as_ptr());
                }
            }
            last_error = io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
            if saved_error.is_none() && !matches!(last_error, libc::ENOENT | libc::ENOTDIR) {
                saved_error = Some(last_error);
            }
        }
        Err(io::Error::from_raw_os_error(
            saved_error.unwrap_or(last_error),
        ))
    }

    fn spawn(self, cwd: Option<&CString>, streams: u32) -> io::Result<Child> {
        // Duplicate the parent's explicit descriptors before setting up either
        // child stream. stderr=STDOUT instead follows the child's chosen stdout.
        fn output(mode: u32) -> io::Result<Stdio> {
            Ok(match mode {
                0 | 5 => Stdio::inherit(),
                1 => Stdio::piped(),
                2 => Stdio::null(),
                3 => Stdio::from(io::stdout().as_fd().try_clone_to_owned()?),
                4 => Stdio::from(io::stderr().as_fd().try_clone_to_owned()?),
                _ => fail("SystemError", "invalid subprocess stream mode"),
            })
        }
        let mut command = Command::new(OsStr::from_bytes(self.argv.strings[0].as_bytes()));
        let stderr = (streams >> 4) & 15;
        command
            .stdin(Stdio::inherit())
            .stdout(output(streams & 15)?)
            .stderr(output(stderr)?);
        if let Some(cwd) = cwd {
            command.current_dir(OsStr::from_bytes(cwd.as_bytes()));
        }
        // SAFETY: this private closure contains only owned immutable native data;
        // exec_child performs no allocation, locking, callback, or language drop.
        // It never returns Ok, so Command cannot fall through to libc execvp.
        unsafe {
            command.pre_exec(move || {
                if stderr == 5 && libc::dup2(libc::STDOUT_FILENO, libc::STDERR_FILENO) < 0 {
                    return Err(io::Error::last_os_error());
                }
                self.exec_child()
            })
        };
        command.spawn()
    }
}

/// A failed pipe setup/read cannot abandon a running child or a zombie.
struct ChildGuard {
    child: Child,
    reaped: bool,
}

impl ChildGuard {
    /// Reaps without blocking, including an inherited auto-reaping SIGCHLD policy.
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.reaped = true;
                Ok(Some(status))
            }
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                self.reaped = true;
                Ok(Some(ExitStatus::from_raw(0)))
            }
            result => result,
        }
    }

    /// Kills only a still-running direct child and always collects its status.
    fn terminate(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_none() {
            match self.child.kill() {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => return Err(error),
            }
            self.wait()?;
        }
        Ok(())
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        match self.child.wait() {
            Ok(status) => {
                self.reaped = true;
                Ok(status)
            }
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                // Like CPython, an inherited SIGCHLD=SIG_IGN can auto-reap the
                // child; there is then no available status and no pid to kill.
                self.reaped = true;
                Ok(ExitStatus::from_raw(0))
            }
            Err(error) => Err(error),
        }
    }
}

/// Timeout capture stops at the deadline rather than waiting for descendant pipes.
enum Captured {
    Completed(ExitStatus, Vec<u8>, Vec<u8>),
    TimedOut(Option<Vec<u8>>, Option<Vec<u8>>),
}

/// Matches POSIX communicate: poll fairly, snapshot before post-deadline reads,
/// then kill/reap the direct child without an unbounded post-timeout pipe drain.
fn capture_timeout(child: Child, deadline: &Deadline) -> Result<Captured> {
    let started = Instant::now();
    let mut child = ChildGuard {
        child,
        reaped: false,
    };
    let timeout = match deadline {
        Deadline::Integer(seconds) => crate::floats::int_to_float(seconds)?,
        Deadline::Float(seconds) => *seconds,
    };
    let mut stdout = child.child.stdout.take();
    let mut stderr = child.child.stderr.take();
    let fds = [
        stdout.as_ref().map_or(-1, AsRawFd::as_raw_fd),
        stderr.as_ref().map_or(-1, AsRawFd::as_raw_fd),
    ];
    if fds == [-1, -1] {
        // With no selector CPython simply waits, even for NaN, +inf or large
        // finite deadlines that poll() would have rejected converting to ms.
        return wait_timeout(&mut child, started, timeout, [Vec::new(), Vec::new()]);
    }
    for fd in fds.into_iter().filter(|&fd| fd >= 0) {
        nonblocking(fd).map_err(os_error)?;
    }
    if timeout.is_nan() {
        return Err(Error::new(
            Kind::ValueError,
            "cannot convert float NaN to integer",
        ));
    }
    if timeout == f64::INFINITY {
        return Err(Error::new(
            Kind::OverflowError,
            "cannot convert float infinity to integer",
        ));
    }
    // Python's poll first converts milliseconds to its signed nanosecond clock,
    // then bounds the millisecond argument to the platform's signed C int.
    let milliseconds = ((timeout - started.elapsed().as_secs_f64()) * 1000.0).ceil();
    if milliseconds >= 9_223_372_036_855.0 {
        return Err(Error::new(
            Kind::OverflowError,
            "timestamp too large to convert to C PyTime_t",
        ));
    }
    if milliseconds > f64::from(i32::MAX) {
        return Err(Error::new(Kind::OverflowError, "timeout is too large"));
    }
    let duration = if timeout > 0.0 {
        Duration::from_secs_f64(timeout)
    } else {
        Duration::ZERO
    };
    let deadline = started + duration;
    let mut polls = fds.map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    let mut outputs = [Vec::new(), Vec::new()];
    let mut observed = [false; 2];
    let mut remaining = fds.iter().filter(|&&fd| fd >= 0).count();
    let mut buffer = [0_u8; 32 * 1024];
    while remaining != 0 {
        let now = Instant::now();
        if now >= deadline {
            child.terminate().map_err(os_error)?;
            let [stdout, stderr] = outputs;
            return Ok(Captured::TimedOut(
                observed[0].then_some(stdout),
                observed[1].then_some(stderr),
            ));
        }
        let milliseconds = ((deadline - now).as_secs_f64() * 1000.0).ceil() as i32;
        // SAFETY: poll receives two writable descriptors owned by this frame.
        if unsafe {
            libc::poll(
                polls.as_mut_ptr(),
                polls.len() as libc::nfds_t,
                milliseconds,
            )
        } < 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(os_error(error));
        }
        // CPython checks expiry before consuming events returned by poll.
        if Instant::now() >= deadline {
            continue;
        }
        for index in 0..2 {
            if polls[index].revents & libc::POLLNVAL != 0 {
                return Err(os_error(io::Error::from_raw_os_error(libc::EBADF)));
            }
            if polls[index].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
                continue;
            }
            let result = if index == 0 {
                stdout.as_mut().expect("polled stdout").read(&mut buffer)
            } else {
                stderr.as_mut().expect("polled stderr").read(&mut buffer)
            };
            match result {
                Ok(count) => {
                    observed[index] = true;
                    if count == 0 {
                        polls[index].fd = -1;
                        remaining -= 1;
                    } else {
                        outputs[index].extend_from_slice(&buffer[..count]);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(os_error(error)),
            }
        }
    }
    wait_timeout(&mut child, started, timeout, outputs)
}

/// After all configured pipes close, a timeout no longer carries partial output.
fn wait_timeout(
    child: &mut ChildGuard,
    started: Instant,
    timeout: f64,
    outputs: [Vec<u8>; 2],
) -> Result<Captured> {
    let mut delay = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait().map_err(os_error)? {
            let [stdout, stderr] = outputs;
            return Ok(Captured::Completed(status, stdout, stderr));
        }
        let remaining = timeout - started.elapsed().as_secs_f64();
        if remaining <= 0.0 {
            child.terminate().map_err(os_error)?;
            return Ok(Captured::TimedOut(None, None));
        }
        let sleep = Duration::try_from_secs_f64(remaining).unwrap_or(delay);
        std::thread::sleep(delay.min(sleep));
        delay = (delay * 2).min(Duration::from_millis(50));
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.reaped {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Err(error) if error.raw_os_error() == Some(libc::ECHILD) => return,
                _ => {}
            }
            let _ = self.child.kill();
            let _ = self.wait();
        }
    }
}

fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: the caller owns this open pipe descriptor throughout both calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: changing only O_NONBLOCK preserves all other open-file flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Polls both streams fairly, including HUP buffers, before collecting status.
/// Decoding happens afterward so every failure still closes pipes and reaps.
fn capture(child: Child) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    capture_guard(ChildGuard {
        child,
        reaped: false,
    })
}

fn capture_guard(mut child: ChildGuard) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    let mut stdout = child.child.stdout.take();
    let mut stderr = child.child.stderr.take();
    let fds = [
        stdout.as_ref().map(AsRawFd::as_raw_fd),
        stderr.as_ref().map(AsRawFd::as_raw_fd),
    ];
    for fd in fds.into_iter().flatten() {
        nonblocking(fd)?;
    }
    let mut polls = fds.map(|fd| libc::pollfd {
        fd: fd.unwrap_or(-1),
        events: libc::POLLIN,
        revents: 0,
    });
    let mut outputs = [Vec::new(), Vec::new()];
    let mut remaining = fds.iter().flatten().count();
    let mut buffer = [0_u8; 16 * 1024];
    while remaining != 0 {
        // SAFETY: poll has exactly two writable descriptors and waits without
        // borrowing any language storage. EINTR restarts the wait below.
        if unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, -1) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        for index in 0..2 {
            if polls[index].revents & libc::POLLNVAL != 0 {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            if polls[index].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
                continue;
            }
            let result = if index == 0 {
                stdout
                    .as_mut()
                    .expect("poll only signals captured stdout")
                    .read(&mut buffer)
            } else {
                stderr
                    .as_mut()
                    .expect("poll only signals captured stderr")
                    .read(&mut buffer)
            };
            match result {
                Ok(0) => {
                    polls[index].fd = -1;
                    remaining -= 1;
                }
                Ok(count) => outputs[index].extend_from_slice(&buffer[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
    let status = child.wait()?;
    let [stdout, stderr] = outputs;
    Ok((status, stdout, stderr))
}

/// Borrows the POSIX environment, which the language runtime never mutates.
fn native_environ() -> *const *const libc::c_char {
    // SAFETY: the OS stores a live null-terminated environment for this process.
    unsafe {
        #[cfg(target_os = "macos")]
        {
            (*libc::_NSGetEnviron()).cast()
        }
        #[cfg(not(target_os = "macos"))]
        {
            environ
        }
    }
}

/// # Safety
/// The array (possibly null) and its C strings must be live and immutable for `'a`.
unsafe fn environment_entries<'a>(
    mut entries: *const *const libc::c_char,
) -> impl Iterator<Item = (&'a [u8], &'a [u8])> {
    std::iter::from_fn(move || {
        loop {
            // SAFETY: each live entry has a successor; the terminating pointer is null.
            let entry = unsafe { entries.as_ref().copied()? };
            if entry.is_null() {
                return None;
            }
            // SAFETY: the non-null C string and the next pointer belong to the array.
            let bytes = unsafe {
                entries = entries.add(1);
                CStr::from_ptr(entry).to_bytes()
            };
            if let Some(split) = bytes.iter().position(|&byte| byte == b'=') {
                return Some((&bytes[..split], &bytes[split + 1..]));
            }
        }
    })
}

/// Returns the first matching value, or the supplied nullable default.
///
/// # Safety
/// The key and any non-null default are rooted strings. A non-null result transfers one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_get(key: *const Str, default: *const Str) -> *const Str {
    // SAFETY: environment entries live until the call returns and the caller owns the key.
    let found = unsafe {
        environment_entries(native_environ()).find(|(name, _)| *name == (*key).as_bytes())
    };
    if let Some((_, bytes)) = found {
        let result = std::str::from_utf8(bytes)
            .map(|text| Rc::into_raw(Rc::new(Str::from(text))))
            .map_err(|_| {
                Error::new(
                    Kind::UnicodeDecodeError,
                    "environment contains non-UTF-8 text",
                )
            });
        finish(result, std::ptr::null())
    } else {
        if !default.is_null() {
            // SAFETY: return an independent owner of the rooted default.
            unsafe { Rc::increment_strong_count(default) };
        }
        default
    }
}

/// Copies native entries in order, preserving empty names and the first duplicate.
/// Entries without '=' are ignored, as in CPython's POSIX environment snapshot.
///
/// # Safety
/// `entries` is null or a live null-terminated array of C-string pointers. The array
/// and strings remain immutable until this function returns.
unsafe fn environment_snapshot(entries: *const *const libc::c_char) -> Result<Rc<Object>> {
    // SAFETY: new transfers one fresh exact dict[str, str] owner.
    let result = unsafe { Rc::from_raw(imre_dict_new(3, 3)) };
    let decode = |bytes: &[u8]| {
        std::str::from_utf8(bytes).map(Str::from).map_err(|_| {
            Error::new(
                Kind::UnicodeDecodeError,
                "environment contains non-UTF-8 text",
            )
        })
    };
    // SAFETY: the caller roots the terminated environment for this iteration.
    for (name, value) in unsafe { environment_entries(entries) } {
        let key = Rc::new(decode(name)?);
        // SAFETY: both the new dictionary and exact string key remain rooted.
        if unsafe { imre_dict_contains(Rc::as_ptr(&result), Rc::as_ptr(&key) as usize) } == 0 {
            let value = Rc::new(decode(value)?);
            // SAFETY: insertion acquires both rooted strings; no language alias
            // can observe this new acyclic dictionary before it is complete.
            unsafe {
                imre_dict_set(
                    Rc::as_ptr(&result),
                    Rc::as_ptr(&key) as usize,
                    Rc::as_ptr(&value) as usize,
                );
            }
        }
    }
    Ok(result)
}

/// Returns an independent UTF-8 dictionary without exposing undecodable host bytes.
/// The sealed runtime never changes environ; diagnostics omit inherited values.
#[unsafe(no_mangle)]
pub extern "C" fn imre_environ_copy() -> *const Object {
    // SAFETY: the OS supplies a terminated environment, and this standalone runtime
    // has no environment mutation or language threads. Rust vars_os omits empty
    // names, so the raw POSIX entries are required for Python-compatible copies.
    let result = unsafe { environment_snapshot(native_environ()) };
    finish(result.map(Rc::into_raw), std::ptr::null())
}

/// Runs one argv command with inherited stdin and two captured UTF-8 text streams.
/// `env=None` inherits; an explicit dictionary replaces the environment. No shell
/// interpretation occurs, including executable files without a valid format.
///
/// # Safety
/// Root an exact list[str], nullable UTF-8 cwd, and nullable dict[str, str]. `check`
/// is 0 or 1. `descriptor` is immutable static metadata with a string-list field, an integer, and two None/str/bytes stream fields.
/// Enter with pending clear. Success transfers one fully initialized record owner;
/// null means pending must be checked before using the result. Inputs are borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_run(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    check: u32,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: this public entrypoint forwards its exact rooted input contract.
    unsafe { run_process(args, cwd, env, check, 0x11, None, descriptor) }
}

/// Runs the same sealed text process with an optional exact float deadline.
/// Timeout errors retain undecoded bytes; `None` uses the original capture path.
///
/// # Safety
/// The ordinary run preconditions apply. `timeout` is null or points to a rooted
/// `f64`; this call borrows it and checks pending before returning a null record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_run_timeout(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    check: u32,
    timeout: *const f64,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the nullable timeout and all process inputs remain rooted.
    unsafe {
        run_process(
            args,
            cwd,
            env,
            check,
            0x11,
            timeout.as_ref().copied().map(Deadline::Float),
            descriptor,
        )
    }
}

/// # Safety
/// Process inputs follow the ordinary run contract. `timeout` is a rooted
/// integer word. The deadline retains its exact original value if it expires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_run_timeout_int(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    check: u32,
    timeout: usize,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: the caller roots all arguments and the exact integer across this call.
    unsafe {
        run_process(
            args,
            cwd,
            env,
            check,
            0x11,
            Some(Deadline::Integer(Int::clone(&Int::borrow(timeout)))),
            descriptor,
        )
    }
}

/// # Safety
/// The caller roots an exact argv list whose elements are all strings.
unsafe fn native_argv(args: *const Object) -> Result<Vec<CString>> {
    // SAFETY: only independent native strings escape the immutable list borrow.
    unsafe {
        object(args).0.with(|storage| {
            let values = storage.sequence();
            values.check_kind(3);
            values
                .values
                .iter()
                .map(|&word| cstring((&*(word as *const Str)).as_bytes()))
                .collect()
        })
    }
}

/// # Safety
/// The caller roots an exact list of strings, Paths, or their closed union.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_check_output(args: *const Object) -> *const Str {
    // SAFETY: inspect only the exact runtime representations admitted by this call.
    let argv = unsafe {
        object(args).0.with(|storage| -> Result<Vec<CString>> {
            let list = storage.sequence();
            list.values
                .iter()
                .map(|&word| {
                    let (word, ty) = if list.kind.raw() == 9 {
                        super::unions::member(word)
                    } else {
                        (word, list.kind)
                    };
                    match ty.raw() {
                        3 => cstring((&*(word as *const Str)).as_bytes()),
                        5 => object(word as *const Object)
                            .0
                            .with(|storage| match storage {
                                Storage::Path { .. } => cstring(storage.path().as_bytes()),
                                _ => fail("SystemError", "expected a subprocess Path argument"),
                            }),
                        _ => fail("SystemError", "invalid subprocess argument kind"),
                    }
                })
                .collect()
        })
    };
    let run = argv.and_then(|argv| {
        if argv.is_empty() {
            return Err(Error::new(Kind::IndexError, "list index out of range"));
        }
        let child = Launch::new(argv, None)?.spawn(None, 1).map_err(os_error)?;
        let (status, stdout, _) = capture(child).map_err(os_error)?;
        Ok((status, Rc::new(decode_utf8_text(stdout)?)))
    });
    match run {
        Ok((status, stdout)) if status.success() => Rc::into_raw(stdout),
        Ok((status, stdout)) => {
            raise_process(ProcessError {
                // SAFETY: the exception acquires the original rooted argv.
                cmd: unsafe { acquire_rc(args as usize) },
                returncode: status
                    .code()
                    .unwrap_or_else(|| -status.signal().unwrap_or(0)),
                // SAFETY: each field transfers one decoded string or no owner.
                stdout: unsafe {
                    OwnedWord::from_owned(Rc::into_raw(stdout) as usize, WordKind::new(3))
                },
                stderr: OwnedWord {
                    word: 0,
                    kind: WordKind::new(0),
                },
            });
            std::ptr::null()
        }
        Err(error) => error.returning(std::ptr::null()),
    }
}

/// Runs a child with its output forwarded directly to the parent's stderr.
///
/// # Safety
/// The caller roots an exact list[str] until launch and checks pending on return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_to_stderr(args: *const Object, check: u32) {
    let run = || -> Result<ExitStatus> {
        // SAFETY: all argv owners are rooted by the caller.
        let argv = unsafe { native_argv(args)? };
        if argv.is_empty() {
            return Err(Error::new(Kind::IndexError, "list index out of range"));
        }
        let launch = Launch::new(argv, None)?;
        let child = launch.spawn(None, 4).map_err(os_error)?;
        ChildGuard {
            child,
            reaped: false,
        }
        .wait()
        .map_err(os_error)
    };
    let absent = || OwnedWord {
        word: 0,
        kind: WordKind::new(0),
    };
    match run() {
        Ok(status) if check != 0 && !status.success() => raise_process(ProcessError {
            // SAFETY: the original list remains rooted while acquiring our owner.
            cmd: unsafe { acquire_rc(args as usize) },
            returncode: status
                .code()
                .unwrap_or_else(|| -status.signal().unwrap_or(0)),
            stdout: absent(),
            stderr: absent(),
        }),
        Ok(_) => {}
        Err(error) => error.raise(),
    }
}

/// Validate rooted language arguments and materialize an independent native launch.
unsafe fn prepare_process(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
) -> Result<(Launch, Option<CString>)> {
    // SAFETY: the caller roots the exact argv, optional directory and environment.
    unsafe {
        // Python selects args[0], validates every environment name, then
        // converts native strings. Preserve that order for mixed failures.
        if object(args)
            .0
            .with(|storage| storage.sequence().values.is_empty())
        {
            return Err(Error::new(Kind::IndexError, "list index out of range"));
        }
        let env = if env.is_null() {
            None
        } else {
            Some(object(env).0.with(|storage| {
                let Storage::Dictionary(mapping) = storage else {
                    fail("SystemError", "expected an environment dictionary");
                };
                mapping.string_pairs()
            }))
        };
        if env
            .as_ref()
            .is_some_and(|pairs| pairs.iter().any(|(key, _)| key.contains('=')))
        {
            return Err(Error::new(
                Kind::ValueError,
                "illegal environment variable name",
            ));
        }
        let argv = native_argv(args)?;
        let cwd = (!cwd.is_null())
            .then(|| cstring((&*cwd).as_bytes()))
            .transpose()?;
        Ok((Launch::new(argv, env)?, cwd))
    }
}

/// Launch with the requested POSIX stream descriptors and a nullable deadline.
///
/// # Safety
/// Inputs are rooted through the call. The descriptor contains a string list,
/// an integer and two stream fields of kind None, str or bytes. A nonzero
/// deadline is either a rooted integer word (2) or a nullable float pointer (7).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_run_modes(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    check: u32,
    streams: u32,
    deadline_kind: u32,
    deadline: usize,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    if streams & 256 != 0 {
        return Error::new(
            Kind::ValueError,
            "stdout and stderr arguments may not be used with capture_output.",
        )
        .returning(std::ptr::null_mut());
    }
    if streams & 15 == 5 {
        return Error::new(Kind::ValueError, "STDOUT can only be used for stderr")
            .returning(std::ptr::null_mut());
    }
    // SAFETY: the caller supplies either None, a rooted integer, or a nullable float.
    unsafe {
        let timeout = match deadline_kind {
            0 => None,
            2 => Some(Deadline::Integer(Int::clone(&Int::borrow(deadline)))),
            7 => (deadline as *const f64)
                .as_ref()
                .copied()
                .map(Deadline::Float),
            _ => fail("SystemError", "invalid process deadline kind"),
        };
        run_process(args, cwd, env, check, streams, timeout, descriptor)
    }
}

/// Convert the drained bytes into the exact field type declared by the call site.
fn completed_stream(data: Vec<u8>, kind: WordKind) -> Result<OwnedWord> {
    let word = match kind.raw() {
        0 => 0,
        3 => Rc::into_raw(Rc::new(decode_utf8_text(data)?)) as usize,
        8 => Rc::into_raw(Rc::new(data)) as usize,
        _ => fail("SystemError", "invalid process output field kind"),
    };
    // SAFETY: each arm transfers its correctly typed owned allocation, or None.
    Ok(unsafe { OwnedWord::from_owned(word, kind) })
}

/// Shares launch validation and successful records without changing no-timeout IO.
unsafe fn run_process(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    check: u32,
    streams: u32,
    timeout: Option<Deadline>,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: only independent native strings escape the exact input storage.
    let prepared = unsafe { prepare_process(args, cwd, env) };
    let (launch, cwd) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return error.returning(std::ptr::null_mut()),
    };
    let captured = launch
        .spawn(cwd.as_ref(), streams)
        .map_err(os_error)
        .and_then(|child| {
            if let Some(timeout) = &timeout {
                capture_timeout(child, timeout)
            } else {
                capture(child)
                    .map(|(status, stdout, stderr)| Captured::Completed(status, stdout, stderr))
                    .map_err(os_error)
            }
        });
    let (status, stdout, stderr) = match captured {
        Ok(Captured::Completed(status, stdout, stderr)) => (status, stdout, stderr),
        Ok(Captured::TimedOut(stdout, stderr)) => {
            raise_timeout(TimeoutError {
                // SAFETY: the original argv stays rooted across launch and capture.
                cmd: unsafe { acquire_rc(args as usize) },
                timeout: timeout.expect("only a deadline produces a timeout"),
                stdout: stdout.map(Rc::new),
                stderr: stderr.map(Rc::new),
            });
            return std::ptr::null_mut();
        }
        Err(error) => return error.returning(std::ptr::null_mut()),
    };
    let result: Result<_> = (|| {
        let returncode = status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or(0));
        // SAFETY: the compiler’s four-field descriptor outlives this run.
        let kinds = unsafe { [*(*descriptor).kinds.add(2), *(*descriptor).kinds.add(3)] };
        // Python decodes stdout before stderr, even when check=True would fail.
        Ok((
            returncode,
            completed_stream(stdout, WordKind::new(kinds[0]))?,
            completed_stream(stderr, WordKind::new(kinds[1]))?,
        ))
    })();
    let (returncode, stdout, stderr) = match result {
        Ok(result) => result,
        Err(error) => return error.returning(std::ptr::null_mut()),
    };
    // SAFETY: the caller retains the original list through this operation. Keeping
    // that exact owner preserves CompletedProcess.args and CalledProcessError.cmd.
    let args = unsafe { acquire_rc(args as usize) };
    if check != 0 && returncode != 0 {
        raise_process(ProcessError {
            cmd: args,
            returncode,
            stdout,
            stderr,
        });
        return std::ptr::null_mut();
    }
    // SAFETY: static metadata describes these four exact fields. No fallible work
    // remains; typed stores preserve reference provenance and transfer each owner.
    unsafe {
        let result = imre_record_new(descriptor);
        result.cast::<*const Object>().write(Rc::into_raw(args));
        result.add(1).write(exit_word(returncode));
        for (index, stream) in [stdout, stderr].into_iter().enumerate() {
            let field = result.add(index + 2);
            if stream.kind == WordKind::new(0) {
                field.write(0);
            } else {
                field
                    .cast::<*const ()>()
                    .write(stream.into_word() as *const ());
            }
        }
        result
    }
}

/// Raises the same rich failure as run(check=True), retaining the result fields.
///
/// # Safety
/// `record` is a rooted, fully initialized sealed CompletedProcess record with
/// a string-list field, an integer, and two None/str/bytes stream fields. Enter with pending clear and check it immediately afterward.
/// Neither the record nor any field owner is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_check(record: *const usize) {
    // SAFETY: the sealed readonly returncode was produced by exit_word from i32.
    let returncode = unsafe { (record.add(1).read() as isize >> 1) as i32 };
    if returncode == 0 {
        return;
    }
    // SAFETY: each exact pointer field remains rooted while acquiring its owner;
    // no field reference or record borrow escapes into pending error replacement.
    let error = unsafe {
        let acquire_stream = |index| {
            let kind = WordKind::new(*super::records::descriptor(record).kinds.add(index));
            kind.acquire(kind.read_record_field(record.add(index)))
        };
        ProcessError {
            cmd: acquire_rc(record.cast::<*const Object>().read() as usize),
            returncode,
            stdout: acquire_stream(2),
            stderr: acquire_stream(3),
        }
    };
    raise_process(error);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::exceptions::{
        imre_error_pending, imre_error_take, imre_exception_matches, imre_exception_message,
        imre_exception_new, imre_subprocess_error_field, imre_subprocess_timeout_field,
    };
    use super::super::records::imre_record_release;
    use super::super::{imre_seq_append, imre_seq_new};
    use crate::Int;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "imre-runtime-processes-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    struct Completed {
        payload: *mut usize,
        _descriptor: Box<RecordDescriptor>,
    }

    impl Completed {
        fn code(&self) -> i64 {
            // SAFETY: this owned record roots its canonical integer field.
            unsafe { Int::borrow(self.payload.add(1).read()).as_small().unwrap() }
        }

        fn text(&self, index: usize) -> &str {
            assert!(matches!(index, 2 | 3));
            // SAFETY: the two exact string fields remain rooted by this record.
            unsafe { &*self.payload.add(index).cast::<*const Str>().read() }
        }
    }

    impl Drop for Completed {
        fn drop(&mut self) {
            // SAFETY: consume the one payload owner before dropping its descriptor.
            unsafe { imre_record_release(self.payload) };
        }
    }

    fn args(values: &[&str]) -> Rc<Object> {
        // SAFETY: new transfers one fresh exact list[str] owner.
        let result = unsafe { Rc::from_raw(imre_seq_new(3)) };
        for value in values {
            let value = Rc::new(Str::new(value));
            // SAFETY: insertion acquires the rooted exact string into this list.
            unsafe { imre_seq_append(Rc::as_ptr(&result), Rc::as_ptr(&value) as usize, 3) };
        }
        result
    }

    fn environment(values: &[(&str, &str)]) -> Rc<Object> {
        // SAFETY: new transfers one fresh exact dict[str, str] owner.
        let result = unsafe { Rc::from_raw(imre_dict_new(3, 3)) };
        for (key, value) in values {
            let key = Rc::new(Str::new(key));
            let value = Rc::new(Str::new(value));
            // SAFETY: both strings remain rooted while insertion acquires them.
            unsafe {
                imre_dict_set(
                    Rc::as_ptr(&result),
                    Rc::as_ptr(&key) as usize,
                    Rc::as_ptr(&value) as usize,
                );
            }
        }
        result
    }

    #[test]
    fn async_children_rendezvous_and_drain_both_pipes() {
        use super::asynchronous::{
            imre_async_process_communicate, imre_async_process_spawn, imre_async_process_status,
        };

        let dir = Directory::new();
        let cwd = Str::new(dir.0.to_str().unwrap());
        let script = r#"
            touch "$1"
            retries=0
            while ! test -f "$2"; do
                retries=$((retries + 1))
                test "$retries" -lt 500 || exit 91
                sleep .01
            done
            dd if=/dev/zero bs=65536 count=4 2>/dev/null
            dd if=/dev/zero bs=65536 count=5 1>&2 2>/dev/null
            exit 7
        "#;
        let spawn = |name, partner| {
            let argv = args(&["/bin/sh", "-c", script, "barrier", name, partner]);
            // SAFETY: the exact list and cwd remain rooted until spawn returns.
            unsafe {
                let child =
                    imre_async_process_spawn(Rc::as_ptr(&argv), &cwd, std::ptr::null(), 1, 1);
                assert!(!child.is_null());
                Rc::from_raw(child)
            }
        };
        let first = spawn("first", "second");
        assert_eq!(
            // SAFETY: the first process is rooted and waits for a missing file.
            unsafe { imre_async_process_status(Rc::as_ptr(&first), false) },
            0
        );
        let second = spawn("second", "first");
        const KINDS: [u32; 2] = [8, 8];
        let descriptor = RecordDescriptor {
            length: 2,
            kinds: KINDS.as_ptr(),
            semantics: std::ptr::null(),
        };
        for owner in [first, second] {
            let process = Rc::as_ptr(&owner);
            // SAFETY: each newly owned pair roots two immutable byte buffers
            // until its release. Its descriptor outlives both communications.
            unsafe {
                assert_eq!(imre_async_process_status(process, true), exit_word(7));
                for lengths in [[4 * 65536, 5 * 65536], [0, 0]] {
                    let pair = imre_async_process_communicate(process, &descriptor);
                    assert!(!pair.is_null());
                    for (slot, length) in lengths.into_iter().enumerate() {
                        let bytes = &*pair.add(slot).cast::<*const Bytes>().read();
                        assert_eq!(bytes.len(), length);
                    }
                    imre_record_release(pair);
                }
                assert_eq!(imre_async_process_status(process, false), exit_word(7));
            }
        }
    }

    fn run(
        args: &Rc<Object>,
        cwd: Option<&str>,
        env: Option<&Rc<Object>>,
        check: bool,
    ) -> std::result::Result<Completed, Rc<Object>> {
        const KINDS: [u32; 4] = [5, 2, 3, 3];
        let descriptor = Box::new(RecordDescriptor {
            length: KINDS.len(),
            kinds: KINDS.as_ptr(),
            semantics: std::ptr::null(),
        });
        let cwd = cwd.map(Str::new);
        // SAFETY: the inputs remain rooted and the descriptor is retained with
        // every returned record; pending errors transfer into an independent Rc.
        unsafe {
            let payload = imre_subprocess_run(
                Rc::as_ptr(args),
                cwd.as_ref().map_or(std::ptr::null(), |cwd| cwd),
                env.map_or(std::ptr::null(), Rc::as_ptr),
                u32::from(check),
                &*descriptor,
            );
            if payload.is_null() {
                assert_eq!(imre_error_pending(), 1);
                Err(Rc::from_raw(imre_error_take()))
            } else {
                assert_eq!(imre_error_pending(), 0);
                Ok(Completed {
                    payload,
                    _descriptor: descriptor,
                })
            }
        }
    }

    fn success(args: &Rc<Object>, cwd: Option<&str>, env: Option<&Rc<Object>>) -> Completed {
        match run(args, cwd, env, false) {
            Ok(result) => result,
            Err(_) => panic!("controlled child unexpectedly failed to run"),
        }
    }

    fn failure(
        args: &Rc<Object>,
        cwd: Option<&str>,
        env: Option<&Rc<Object>>,
        check: bool,
    ) -> Rc<Object> {
        match run(args, cwd, env, check) {
            Ok(_) => panic!("controlled child unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    fn assert_kind(error: &Rc<Object>, kind: Kind) {
        // SAFETY: the error remains rooted and the expected kind is a valid ABI id.
        let matches = unsafe { imre_exception_matches(Rc::as_ptr(error), kind as u32) };
        assert_eq!(matches, 1);
    }

    fn message(error: &Rc<Object>) -> Rc<Str> {
        // SAFETY: formatting returns one owned string from the rooted exception.
        unsafe { Rc::from_raw(imre_exception_message(Rc::as_ptr(error))) }
    }

    /// Exercises the public deadline ABI and transfers the resulting pending owner.
    fn timeout_failure(args: &Rc<Object>, timeout: f64) -> Rc<Object> {
        const KINDS: [u32; 4] = [5, 2, 3, 3];
        let descriptor = RecordDescriptor {
            length: KINDS.len(),
            kinds: KINDS.as_ptr(),
            semantics: std::ptr::null(),
        };
        // SAFETY: exact rooted inputs and static field kinds survive the call.
        unsafe {
            let result = imre_subprocess_run_timeout(
                Rc::as_ptr(args),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &timeout,
                &descriptor,
            );
            if !result.is_null() {
                super::super::records::imre_record_release(result);
                panic!("controlled timeout unexpectedly succeeded");
            }
            assert_eq!(imre_error_pending(), 1);
            Rc::from_raw(imre_error_take())
        }
    }

    /// Reads one nullable immutable bytes owner from an exact timeout view.
    fn timeout_bytes(error: &Rc<Object>, field: u32) -> Option<Rc<Bytes>> {
        // SAFETY: the caller proves TimeoutExpired and selects a nullable byte field.
        unsafe {
            let word = imre_subprocess_timeout_field(Rc::as_ptr(error), field);
            (word != 0).then(|| Rc::from_raw(word as *const Bytes))
        }
    }

    #[test]
    fn timeout_preserves_raw_bytes_and_reaps_the_direct_child() {
        let args = args(&[
            "/bin/sh",
            "-c",
            r#"printf 'raw\r\n\377\342\202'; printf %s "$$" >&2; exec sleep 10"#,
        ]);
        let weak = Rc::downgrade(&args);
        let error = timeout_failure(&args, 0.5);
        assert_kind(&error, Kind::TimeoutExpired);
        assert_kind(&error, Kind::SubprocessError);
        assert_kind(&error, Kind::Exception);
        let stdout = timeout_bytes(&error, 2).unwrap();
        let output = timeout_bytes(&error, 4).unwrap();
        assert!(Rc::ptr_eq(&stdout, &output));
        assert_eq!(stdout.as_slice(), b"raw\r\n\xff\xe2\x82");
        let stderr = timeout_bytes(&error, 3).unwrap();
        let pid: i32 = std::str::from_utf8(&stderr).unwrap().parse().unwrap();
        // SAFETY: kill with signal zero only probes this reaped child PID;
        // waitpid's null status is valid with WNOHANG and cannot consume other children.
        unsafe {
            assert_eq!(libc::kill(pid, 0), -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
            assert_eq!(libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG), -1);
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            assert_eq!(
                imre_exception_matches(Rc::as_ptr(&error), Kind::CalledProcessError as u32),
                0
            );
            assert_eq!(
                f64::from_bits(imre_subprocess_timeout_field(Rc::as_ptr(&error), 1) as u64),
                0.5
            );
            let cmd = Rc::<Object>::from_raw(
                imre_subprocess_timeout_field(Rc::as_ptr(&error), 0) as *const Object
            );
            assert!(Rc::ptr_eq(&cmd, &args));
            let addition = Rc::new(Str::new("mutated"));
            imre_seq_append(Rc::as_ptr(&args), Rc::as_ptr(&addition) as usize, 3);
        }
        assert!(message(&error).ends_with("'mutated']' timed out after 0.5 seconds"));
        drop(args);
        assert!(weak.upgrade().is_some());
        drop(error);
        assert!(weak.upgrade().is_none());
        assert_eq!(stdout.as_slice(), b"raw\r\n\xff\xe2\x82");
    }

    #[test]
    fn timeout_does_not_wait_for_descendant_pipe_owners() {
        let argv = args(&["/bin/sh", "-c", "sleep 2 & printf 'parent finished'"]);
        let started = Instant::now();
        let error = timeout_failure(&argv, 0.4);
        assert_kind(&error, Kind::TimeoutExpired);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            timeout_bytes(&error, 2).unwrap().as_slice(),
            b"parent finished"
        );
    }

    #[test]
    fn deadline_capture_drains_both_full_pipes_before_expiry() {
        let mut child = Command::new("python3")
            .args([
                "-c",
                "import os,time\nos.write(1,b'!')\nfor _ in range(4):\n os.write(1,b'O'*65536)\n os.write(2,b'E'*65536)\ntime.sleep(10)",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Start the capture deadline after interpreter startup, before draining either pipe.
        let mut ready = [0];
        let stdout = child.stdout.as_mut().unwrap();
        stdout.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [b'!']);
        let Ok(Captured::TimedOut(Some(stdout), Some(stderr))) =
            capture_timeout(child, &Deadline::Float(1.0))
        else {
            panic!("expected a timeout with both output streams");
        };
        assert_eq!(stdout.len(), 262_144);
        assert_eq!(stderr.len(), 262_144);
        assert!(stdout.iter().all(|byte| *byte == b'O'));
        assert!(stderr.iter().all(|byte| *byte == b'E'));
    }

    #[test]
    fn timeout_boundaries_and_failed_validation_leave_no_direct_children() {
        let argv = args(&["sleep", "10"]);
        for timeout in [0.0, -0.0, -0.5, f64::NEG_INFINITY] {
            let error = timeout_failure(&argv, timeout);
            assert_kind(&error, Kind::TimeoutExpired);
            assert!(timeout_bytes(&error, 2).is_none());
            assert!(timeout_bytes(&error, 3).is_none());
        }
        for (timeout, kind, expected) in [
            (
                f64::NAN,
                Kind::ValueError,
                "cannot convert float NaN to integer",
            ),
            (
                f64::INFINITY,
                Kind::OverflowError,
                "cannot convert float infinity to integer",
            ),
            (2147484.0, Kind::OverflowError, "timeout is too large"),
            (
                1e20,
                Kind::OverflowError,
                "timestamp too large to convert to C PyTime_t",
            ),
        ] {
            let error = timeout_failure(&argv, timeout);
            assert_kind(&error, kind);
            assert_eq!(message(&error).as_str(), expected);
        }
        for _ in 0..32 {
            drop(timeout_failure(&argv, 0.0));
        }
        assert_eq!(success(&args(&["/usr/bin/true"]), None, None).code(), 0);
    }

    #[test]
    fn records_and_rich_failures_keep_original_args_and_owned_streams() {
        let args = args(&[
            "/usr/bin/python3",
            "-c",
            "import os; os.write(1, 'é\\r\\n🦀\\r'.encode()); os.write(2, b'err\\r'); raise SystemExit(7)",
        ]);
        let weak = Rc::downgrade(&args);
        let result = success(&args, None, Some(&environment(&[])));
        assert_eq!(result.code(), 7);
        assert_eq!(result.text(2), "é\n🦀\n");
        assert_eq!(result.text(3), "err\n");
        let addition = Rc::new(Str::new("later"));
        // SAFETY: the original list and added string remain rooted; the result
        // owns the same list, and checking acquires all rich exception fields.
        let error = unsafe {
            assert_eq!(
                result.payload.cast::<*const Object>().read(),
                Rc::as_ptr(&args)
            );
            imre_seq_append(Rc::as_ptr(&args), Rc::as_ptr(&addition) as usize, 3);
            imre_subprocess_check(result.payload);
            assert_eq!(imre_error_pending(), 1);
            Rc::from_raw(imre_error_take())
        };
        for kind in [
            Kind::CalledProcessError,
            Kind::SubprocessError,
            Kind::Exception,
        ] {
            assert_kind(&error, kind);
        }
        assert!(message(&error).contains("'later']' returned non-zero exit status 7."));
        // SAFETY: every field accessor transfers one owner of its documented
        // representation; the alias views are acquired and dropped independently.
        unsafe {
            let cmd = Rc::<Object>::from_raw(
                imre_subprocess_error_field(Rc::as_ptr(&error), 0) as *const Object
            );
            assert!(Rc::ptr_eq(&cmd, &args));
            assert_eq!(
                Int(imre_subprocess_error_field(Rc::as_ptr(&error), 1)).as_small(),
                Some(7)
            );
            let stdout = Rc::<Str>::from_raw(
                imre_subprocess_error_field(Rc::as_ptr(&error), 2) as *const Str
            );
            let output = Rc::<Str>::from_raw(
                imre_subprocess_error_field(Rc::as_ptr(&error), 4) as *const Str
            );
            assert!(Rc::ptr_eq(&stdout, &output));
            assert_eq!(stdout.as_str(), "é\n🦀\n");
            let stderr = Rc::<Str>::from_raw(
                imre_subprocess_error_field(Rc::as_ptr(&error), 3) as *const Str
            );
            assert_eq!(stderr.as_str(), "err\n");
            let addition = Rc::new(Str::new("newest"));
            imre_seq_append(Rc::as_ptr(&args), Rc::as_ptr(&addition) as usize, 3);
        }
        assert!(message(&error).contains("'newest']' returned non-zero exit status 7."));
        drop(result);
        drop(args);
        assert!(weak.upgrade().is_some());
        drop(error);
        assert!(weak.upgrade().is_none());
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn both_full_pipes_drain_before_strict_decoding_and_check() {
        let result = success(
            &args(&[
                "/usr/bin/python3",
                "-c",
                "import os\nfor _ in range(16):\n os.write(1, b'x'*65536)\n os.write(2, b'y'*65536)",
            ]),
            None,
            Some(&environment(&[])),
        );
        assert_eq!(result.code(), 0);
        assert_eq!(result.text(2).len(), 1_048_576);
        assert_eq!(result.text(3).len(), 1_048_576);
        assert!(result.text(2).bytes().all(|byte| byte == b'x'));
        assert!(result.text(3).bytes().all(|byte| byte == b'y'));
        // SAFETY: this successful rooted result has no pending error to replace.
        unsafe { imre_subprocess_check(result.payload) };
        assert_eq!(imre_error_pending(), 0);

        let error = failure(
            &args(&[
                "/usr/bin/python3",
                "-c",
                "import os; os.write(1, b'abc\\xff'); os.write(2, b'\\xfe'); raise SystemExit(9)",
            ]),
            None,
            Some(&environment(&[])),
            true,
        );
        assert_kind(&error, Kind::UnicodeDecodeError);
        assert_eq!(message(&error).as_str(), "invalid UTF-8 at byte 3");
        let split = success(
            &args(&[
                "/usr/bin/python3",
                "-c",
                "import os; os.write(1, b'\\xf0\\x9f'); os.write(2, b'x\\r'); os.write(1, b'\\xa6\\x80\\r\\n')",
            ]),
            None,
            Some(&environment(&[])),
        );
        assert_eq!(split.text(2), "🦀\n");
        assert_eq!(split.text(3), "x\n");
    }

    #[test]
    fn replacement_environment_path_and_cwd_do_not_use_a_shell() {
        let directory = Directory::new();
        std::os::unix::fs::symlink("/usr/bin/env", directory.0.join("controlled-child")).unwrap();
        let controlled = environment(&[
            ("PATH", ""),
            ("IMRE_TEST_VALUE", "controlled"),
            ("LC_ALL", "C.UTF-8"),
        ]);
        let result = success(
            &args(&["controlled-child"]),
            directory.0.to_str(),
            Some(&controlled),
        );
        assert_eq!(result.code(), 0);
        assert_eq!(
            result.text(2),
            "PATH=\nIMRE_TEST_VALUE=controlled\nLC_ALL=C.UTF-8\n"
        );

        let empty = environment(&[]);
        let result = success(&args(&["env"]), None, Some(&empty));
        assert_eq!(result.text(2), "");
        let ordered = environment(&[("IMRE_B", "two"), ("IMRE_A", "one")]);
        let result = success(&args(&["/usr/bin/env"]), None, Some(&ordered));
        assert_eq!(result.text(2), "IMRE_B=two\nIMRE_A=one\n");

        let executable = directory.0.join("no-header");
        fs::write(&executable, "printf forbidden > escaped\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let error = failure(
            &args(&["./no-header"]),
            directory.0.to_str(),
            Some(&empty),
            false,
        );
        assert_kind(&error, Kind::OSError);
        assert!(!directory.0.join("escaped").exists());
    }

    #[test]
    fn validation_and_signal_failures_remain_catchable() {
        let empty = environment(&[]);
        for (argv, cwd, env, kind) in [
            (args(&[]), None, empty.clone(), Kind::IndexError),
            (
                args(&["missing-imre-controlled-command"]),
                None,
                empty.clone(),
                Kind::FileNotFoundError,
            ),
            (
                args(&["/usr/bin/true", "a\0b"]),
                None,
                empty.clone(),
                Kind::ValueError,
            ),
            (
                args(&["/usr/bin/true"]),
                Some("a\0b"),
                empty.clone(),
                Kind::ValueError,
            ),
            (
                args(&["/usr/bin/true"]),
                None,
                environment(&[("bad=key", "value")]),
                Kind::ValueError,
            ),
            (
                args(&["/usr/bin/true"]),
                None,
                environment(&[("key", "bad\0value")]),
                Kind::ValueError,
            ),
        ] {
            assert_kind(&failure(&argv, cwd, Some(&env), false), kind);
        }
        let invalid = environment(&[("early", "null\0value"), ("bad=key", "value")]);
        for (argv, cwd) in [
            (args(&["/usr/bin/true", "null\0arg"]), None),
            (args(&["/usr/bin/true"]), Some("null\0cwd")),
            (args(&["/usr/bin/true"]), None),
        ] {
            let error = failure(&argv, cwd, Some(&invalid), false);
            assert_kind(&error, Kind::ValueError);
            assert_eq!(
                message(&error).as_str(),
                "illegal environment variable name"
            );
        }
        let error = failure(&args(&[]), Some("null\0cwd"), Some(&invalid), false);
        assert_kind(&error, Kind::IndexError);
        assert_eq!(message(&error).as_str(), "list index out of range");
        let error = failure(
            &args(&[
                "/usr/bin/python3",
                "-c",
                "import os, signal; os.kill(os.getpid(), signal.SIGTERM)",
            ]),
            None,
            Some(&empty),
            true,
        );
        assert_kind(&error, Kind::CalledProcessError);
        assert!(message(&error).ends_with("died with <Signals.SIGTERM: 15>."));
        // SAFETY: the error accessor transfers one canonical tagged integer.
        let code = unsafe { Int(imre_subprocess_error_field(Rc::as_ptr(&error), 1)).as_small() };
        assert_eq!(code, Some(-15));
        crate::initialize_io();
        let signaled = success(
            &args(&["/bin/sh", "-c", "kill -PIPE $$; printf incorrect"]),
            None,
            Some(&empty),
        );
        assert_eq!(signaled.code(), -13);
        assert_eq!(signaled.text(2), "");
    }

    #[test]
    fn guarded_error_fields_preserve_attribute_names_and_pending_ownership() {
        // SAFETY: this creates one owned ordinary exception from zero arguments.
        let ordinary = unsafe {
            Rc::from_raw(imre_exception_new(
                Kind::ValueError as u32,
                std::ptr::null(),
            ))
        };
        for (field, name) in ["cmd", "returncode", "stdout", "stderr", "output"]
            .iter()
            .enumerate()
        {
            // SAFETY: an ordinary rooted exception is a supported guarded input;
            // the field id is valid, and the failed call returns no field owner.
            let error = unsafe {
                assert_eq!(
                    imre_subprocess_error_field(Rc::as_ptr(&ordinary), field as u32),
                    0
                );
                assert_eq!(imre_error_pending(), 1);
                Rc::from_raw(imre_error_take())
            };
            assert_kind(&error, Kind::AttributeError);
            assert_eq!(
                message(&error).as_str(),
                format!("'ValueError' object has no attribute '{name}'")
            );
        }
    }

    #[test]
    fn inherited_descriptors_close_and_failed_capture_guard_reaps() {
        let input = fs::File::open("/dev/null").unwrap();
        // SAFETY: duplicate this live descriptor into a separately owned, deliberately
        // inheritable high fd; the parent owner closes it exactly once after testing.
        let fd = unsafe { libc::fcntl(input.as_raw_fd(), libc::F_DUPFD, 211) };
        assert!(fd >= 211);
        // SAFETY: successful F_DUPFD transfers the fresh descriptor to this owner.
        let inherited = unsafe { OwnedFd::from_raw_fd(fd) };
        let code = format!(
            "import os\ntry: os.fstat({fd})\nexcept OSError: print('closed')\nelse: print('leaked')"
        );
        let argv = args(&["/usr/bin/python3", "-c", &code]);
        let result = success(&argv, None, Some(&environment(&[])));
        assert_eq!(result.text(2), "closed\n");
        let mut launch = match Launch::new(
            ["/usr/bin/python3", "-c", &code]
                .iter()
                .map(|arg| CString::new(*arg).unwrap())
                .collect(),
            Some(Vec::new()),
        ) {
            Ok(launch) => launch,
            Err(_) => panic!("controlled launch preparation failed"),
        };
        launch.descriptors = Descriptors::Snapshot(vec![inherited.as_raw_fd()]);
        let (status, stdout, _) = capture(launch.spawn(None, 0x11).unwrap()).unwrap();
        assert!(status.success());
        assert_eq!(stdout, b"closed\n");

        let child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        drop(ChildGuard {
            child,
            reaped: false,
        });
        let mut status = 0;
        // SAFETY: this queries only our former child pid and writable status slot;
        // the guard must already have collected it, leaving ECHILD.
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        assert_eq!(waited, -1);
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[test]
    fn environment_snapshot_keeps_empty_names_first_duplicates_and_valid_text() {
        let entries = CStrings::new(
            [
                b"=allowed".as_slice(),
                b"IMRE_KEY=first",
                b"IMRE_KEY=second",
                b"IMRE_KEY=\xff",
                b"IMRE_VALUE=left=right",
                b"=ignored",
                b"malformed",
                b"\xff",
            ]
            .into_iter()
            .map(|entry| CString::new(entry).unwrap())
            .collect(),
        );
        // SAFETY: the immutable CStrings owner roots the terminated native array.
        let copy = unsafe { environment_snapshot(entries.pointers.as_ptr()) }
            .unwrap_or_else(|_| panic!("controlled environment unexpectedly failed"));
        // SAFETY: the owned dictionary roots its exact strings while they are copied.
        let pairs = unsafe {
            copy.0.with(|storage| {
                let Storage::Dictionary(mapping) = storage else {
                    panic!("environment snapshot must be a dictionary");
                };
                mapping.string_pairs()
            })
        };
        assert_eq!(
            pairs,
            [
                (Str::new(""), Str::new("allowed")),
                (Str::new("IMRE_KEY"), Str::new("first")),
                (Str::new("IMRE_VALUE"), Str::new("left=right")),
            ]
        );
        let entries = CStrings::new(vec![CString::new(b"KEY=\xff".as_slice()).unwrap()]);
        // SAFETY: the immutable array is valid but contains undecodable value text.
        match unsafe { environment_snapshot(entries.pointers.as_ptr()) } {
            Ok(_) => panic!("non-UTF-8 environment unexpectedly succeeded"),
            Err(error) => error.raise(),
        }
        // SAFETY: decoding published exactly one owned pending exception.
        let error = unsafe { Rc::from_raw(imre_error_take()) };
        assert_kind(&error, Kind::UnicodeDecodeError);
        assert_eq!(
            message(&error).as_str(),
            "environment contains non-UTF-8 text"
        );
    }

    #[test]
    fn environment_copy_is_owned_without_logging_host_values() {
        let copy = imre_environ_copy();
        if std::env::vars_os()
            .any(|(key, value)| key.to_str().is_none() || value.to_str().is_none())
        {
            assert!(copy.is_null());
            // SAFETY: failed decoding publishes one pending exception owner.
            let error = unsafe { Rc::from_raw(imre_error_take()) };
            assert_kind(&error, Kind::UnicodeDecodeError);
            return;
        }
        assert!(!copy.is_null());
        // SAFETY: the snapshot transfers one exact dictionary owner.
        let copy = unsafe { Rc::from_raw(copy) };
        let key = Rc::new(Str::new("IMRE_RUNTIME_CONTROLLED_COPY_KEY"));
        let value = Rc::new(Str::new("controlled-copy-only"));
        let previous = std::env::var_os(key.as_str());
        // SAFETY: insertion acquires the two exact rooted strings into the owned
        // dictionary; it has no link back to the process's native environment.
        unsafe {
            imre_dict_set(
                Rc::as_ptr(&copy),
                Rc::as_ptr(&key) as usize,
                Rc::as_ptr(&value) as usize,
            )
        };
        // A boolean-only assertion avoids printing any inherited environment value.
        assert!(std::env::var_os(key.as_str()) == previous);
        assert_eq!(imre_error_pending(), 0);
    }
}
