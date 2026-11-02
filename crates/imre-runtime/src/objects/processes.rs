//! Sealed synchronous Unix processes with inherited, redirected, or captured streams.
//!
//! Native launch data owns its bytes before fork. The child performs only
//! async-signal-safe descriptor, signal, and exec operations; language owners
//! remain in the parent. All requested pipes are drained before decoding or check.

use crate::StrOwner;
use std::ffi::{CStr, CString, OsStr};
use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::exceptions::{Error, Kind, Result, finish, raise_process, raise_timeout};
use super::files::{decode_utf8_text, os_error};
use super::mappings::{StringPairs, imre_dict_contains, imre_dict_new};
use super::records::{RecordDescriptor, imre_record_new};
use super::values::ReprWriter;
use super::words::{OwnedWord, WordKind};
use super::{Bytes, Codec, Int, Object, Storage, Str, fail, object};

mod argv;
mod asynchronous;
mod environ_item;
mod launch_error;
mod pass_fds;
mod pipe_size;
mod popen;
mod shell_output;
pub(super) use asynchronous::AsyncProcess;
pub(crate) use popen::{BoxedPipe, BoxedPopen};
pub(super) use popen::{Popen, retain_live_owner};

#[cfg(target_os = "linux")]
unsafe extern "C" {
    static environ: *const *const libc::c_char;
}

/// Rich failures keep the original argv list, so later aliases affect str(error).
/// Its sealed string and path elements cannot point back to the exception.
#[derive(Clone)]
pub(super) struct ProcessError {
    positional_streams: bool,
    cmd: OwnedWord,
    returncode: i32,
    stdout: OwnedWord,
    stderr: OwnedWord,
}

impl ProcessError {
    /// run(check=True) passes output by keyword; CompletedProcess.check_returncode uses positional streams.
    pub(super) fn arguments(&self) -> smallvec::SmallVec<[OwnedWord; 5]> {
        let mut fields = smallvec::smallvec![
            OwnedWord {
                kind: WordKind::new(2),
                word: exit_word(self.returncode)
            },
            self.cmd.clone()
        ];
        if self.positional_streams {
            fields.extend([self.stdout.clone(), self.stderr.clone()]);
        }
        fields
    }
    pub(super) fn field(&self, field: u32, tagged: bool) -> usize {
        match field {
            0 => self.cmd.clone().into_word(),
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

    pub(super) fn message(&self) -> std::result::Result<Str, fmt::Error> {
        let mut text = argv::message_prefix(&self.cmd)?;
        let mut output = ReprWriter(&mut text);
        if self.returncode < 0 {
            let signal = -self.returncode;
            if let Some(name) = signal_name(signal) {
                write!(output, " died with <Signals.{name}: {signal}>.")?;
            } else {
                write!(output, " died with unknown signal {signal}.")?;
            }
        } else {
            write!(
                output,
                " returned non-zero exit status {}.",
                self.returncode
            )?;
        }
        Ok(text.into())
    }
}

#[derive(Clone)]
enum Deadline {
    Integer(Int),
    Float(f64),
}

impl Deadline {
    fn seconds(&self) -> Result<f64> {
        match self {
            Self::Integer(seconds) => crate::floats::int_to_float(seconds),
            Self::Float(seconds) => Ok(*seconds),
        }
    }

    /// # Safety
    /// A nonzero word is a rooted integer (2) or a nullable float pointer (7).
    unsafe fn from_word(kind: u32, word: usize) -> Option<Self> {
        // SAFETY: the caller supplies the representation selected by the kind.
        unsafe {
            match kind {
                0 => None,
                2 => Some(Self::Integer(Int::clone(&Int::borrow(word)))),
                7 => (word as *const f64).as_ref().copied().map(Self::Float),
                _ => fail("SystemError", "invalid process deadline kind"),
            }
        }
    }
}

/// A deadline failure owns the original argv and undecoded pipe snapshots.
/// Empty streams distinguish an observed EOF from no data observed before expiry.
#[derive(Clone)]
pub(super) struct TimeoutError {
    cmd: OwnedWord,
    timeout: Deadline,
    // communicate resets .timeout after the final wait; args keeps that wait's budget.
    wait_budget: Option<f64>,
    stdout: Option<crate::BytesOwner>,
    stderr: Option<crate::BytesOwner>,
}

impl TimeoutError {
    pub(super) fn arguments(&self) -> smallvec::SmallVec<[OwnedWord; 5]> {
        let timeout = match (&self.timeout, self.wait_budget) {
            (_, Some(n)) => OwnedWord {
                kind: WordKind::new(7),
                word: n.to_bits() as usize,
            },
            (Deadline::Integer(n), None) => OwnedWord {
                kind: WordKind::new(2),
                word: n.clone().into_raw(),
            },
            (Deadline::Float(n), None) => OwnedWord {
                kind: WordKind::new(7),
                word: n.to_bits() as usize,
            },
        };
        smallvec::smallvec![self.cmd.clone(), timeout]
    }
    /// The compiler proves the numeric field's exact kind at every read.
    pub(super) fn field(&self, field: u32) -> usize {
        match field {
            0 => self.cmd.clone().into_word(),
            1 => match &self.timeout {
                Deadline::Integer(seconds) => seconds.clone().into_raw(),
                Deadline::Float(seconds) => seconds.to_bits() as usize,
            },
            2 | 4 => self.stdout.as_ref().map_or(0, |value| {
                crate::BytesOwner::into_raw(value.clone()) as usize
            }),
            3 => self.stderr.as_ref().map_or(0, |value| {
                crate::BytesOwner::into_raw(value.clone()) as usize
            }),
            _ => fail("SystemError", "invalid timeout exception field"),
        }
    }

    /// Formatting observes later mutations of the same rooted argv list.
    pub(super) fn message(&self) -> std::result::Result<Str, fmt::Error> {
        let mut text = argv::message_prefix(&self.cmd)?;
        let mut output = ReprWriter(&mut text);
        match &self.timeout {
            Deadline::Integer(seconds) => {
                write!(output, " timed out after {} seconds", seconds.big())?
            }
            Deadline::Float(seconds) => write!(
                output,
                " timed out after {} seconds",
                crate::floats::render(*seconds)
            )?,
        }
        Ok(text.into())
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

/// A traced stop can be reported by waitpid without WUNTRACED. Popen
/// exposes the negative stop signal just as it exposes a terminating signal.
fn returncode(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        -status
            .stopped_signal()
            .or_else(|| status.signal())
            .unwrap_or(0)
    })
}

fn exit_word(returncode: i32) -> usize {
    // Every signed 32-bit exit code fits the signed 63-bit small representation.
    ((returncode as usize) << 1) | 1
}

fn cstring(bytes: &[u8]) -> Result<CString> {
    let mut owned = Vec::new();
    owned
        .try_reserve_exact(bytes.len() + 1)
        .map_err(|_| Error::allocation())?;
    owned.extend_from_slice(bytes);
    owned.push(0);
    CString::from_vec_with_nul(owned)
        .map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))
}

/// Self-contained immutable C arrays: moving the owner never moves CString data.
pub(crate) struct CStrings {
    strings: Vec<CString>,
    pointers: Vec<*const libc::c_char>,
}

// SAFETY: pointers refer only to immutable allocations owned by `strings`; the
// array is terminated, never mutated, and cannot outlive those allocations.
unsafe impl Send for CStrings {}
// SAFETY: shared access only reads the same immutable owned strings and pointers.
unsafe impl Sync for CStrings {}

impl CStrings {
    pub(crate) fn new(strings: Vec<CString>) -> Result<Self> {
        let size = strings.len().checked_add(1).ok_or_else(Error::allocation)?;
        let mut pointers = Vec::new();
        pointers
            .try_reserve_exact(size)
            .map_err(|_| Error::allocation())?;
        pointers.extend(strings.iter().map(|value| value.as_ptr()));
        pointers.push(std::ptr::null());
        Ok(Self { strings, pointers })
    }

    /// Spawn a waitable child, optionally searching the parent's PATH for the executable.
    #[cfg(target_os = "linux")]
    pub(crate) fn spawn(
        &self,
        executable: &CStr,
        search_path: bool,
        environment: Option<&Self>,
        actions: Option<&libc::posix_spawn_file_actions_t>,
        attributes: &libc::posix_spawnattr_t,
    ) -> io::Result<libc::pid_t> {
        let launch = if search_path {
            libc::posix_spawnp
        } else {
            libc::posix_spawn
        };
        let _guard = ENVIRONMENT.lock().unwrap();
        let envp = environment.map_or_else(native_environ, |env| env.pointers.as_ptr());
        let mut pid = 0;
        // SAFETY: libc only reads these live, terminated argv/envp arrays and the
        // caller's initialized actions and attributes. The lock also protects PATH
        // lookup by posix_spawnp when the child has an explicit environment.
        let result = unsafe {
            launch(
                &mut pid,
                executable.as_ptr(),
                actions.map_or(std::ptr::null(), std::ptr::from_ref),
                attributes,
                self.pointers.as_ptr().cast(),
                envp.cast(),
            )
        };
        if result == 0 {
            Ok(pid)
        } else {
            Err(io::Error::from_raw_os_error(result))
        }
    }

    /// Replace the process with its current environment, returning the errno on failure.
    pub(crate) fn execv(&self, executable: &CStr) -> Error {
        let _guard = ENVIRONMENT.lock().unwrap();
        // SAFETY: the path and all argv entries remain live and NUL-terminated,
        // and the environment lock excludes runtime changes to libc's environ.
        unsafe { libc::execv(executable.as_ptr(), self.pointers.as_ptr()) };
        super::errno_error(io::Error::last_os_error())
    }

    /// Fork with the inherited environment and descriptors; failed execs exit with 127.
    #[cfg(target_os = "linux")]
    pub(crate) fn fork_execv(command: Option<(&CStr, &Self)>) -> Result<libc::pid_t> {
        let image = command.map(|(path, argv)| (path.as_ptr(), argv.pointers.as_ptr()));
        let _guard = ENVIRONMENT.lock().unwrap();
        // SAFETY: the environment lock excludes concurrent changes at fork. The
        // child only calls async-signal-safe functions on the inherited C arrays
        // and never returns to Rust to drop the guard or the arrays' owners.
        unsafe {
            match libc::fork() {
                0 => {
                    if let Some((path, argv)) = image {
                        libc::execv(path, argv);
                    }
                    libc::_exit(127)
                }
                -1 => Err(super::errno_error(io::Error::last_os_error())),
                pid => Ok(pid),
            }
        }
    }

    fn from_environment(env: StringPairs) -> Result<Self> {
        Self::from_pairs(env.iter().map(|(k, v)| (k.as_bytes(), v.as_bytes())))
    }

    fn from_pairs<'a>(env: impl ExactSizeIterator<Item = (&'a [u8], &'a [u8])>) -> Result<Self> {
        let mut strings = Vec::new();
        strings
            .try_reserve_exact(env.len())
            .map_err(|_| Error::allocation())?;
        for (key, value) in env {
            let size = key
                .len()
                .checked_add(value.len())
                .and_then(|n| n.checked_add(2))
                .ok_or_else(Error::allocation)?;
            let mut entry = Vec::new();
            entry
                .try_reserve_exact(size)
                .map_err(|_| Error::allocation())?;
            entry.extend_from_slice(key);
            entry.push(b'=');
            entry.extend_from_slice(value);
            entry.push(0);
            strings.push(
                CString::from_vec_with_nul(entry)
                    .map_err(|_| Error::new(Kind::ValueError, "embedded null byte"))?,
            );
        }
        Self::new(strings)
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

/// Reserve both buffers before fork so the child can reuse one candidate.
struct PathSearch {
    path: Vec<u8>,
    candidate: Vec<u8>,
}

impl PathSearch {
    fn new(path: &[u8], program: &CStr) -> Result<Self> {
        let longest = path
            .split(|&byte| byte == b':')
            .map(<[u8]>::len)
            .max()
            .unwrap_or(0);
        let size = longest
            .checked_add(program.to_bytes_with_nul().len())
            .and_then(|size| size.checked_add(1))
            .ok_or_else(Error::allocation)?;
        let mut candidate = Vec::new();
        candidate
            .try_reserve_exact(size)
            .map_err(|_| Error::allocation())?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(path.len())
            .map_err(|_| Error::allocation())?;
        owned.extend_from_slice(path);
        Ok(Self {
            path: owned,
            candidate,
        })
    }

    fn visit(&mut self, program: &CStr, mut attempt: impl FnMut(*const libc::c_char)) {
        for directory in self.path.split(|&byte| byte == b':') {
            self.candidate.clear();
            if !directory.is_empty() {
                self.candidate.extend_from_slice(directory);
                self.candidate.push(b'/');
            }
            self.candidate
                .extend_from_slice(program.to_bytes_with_nul());
            attempt(self.candidate.as_ptr().cast());
        }
    }
}

struct ChildAttributes {
    new_session: bool,
    group: libc::pid_t,
    mask: Option<libc::mode_t>,
    keep: Vec<RawFd>,
}

impl Default for ChildAttributes {
    fn default() -> Self {
        Self {
            new_session: false,
            group: -1,
            mask: None,
            keep: Vec::new(),
        }
    }
}

const DEFAULT_SHELL: &std::ffi::CStr = if cfg!(target_os = "android") {
    c"/system/bin/sh"
} else {
    c"/bin/sh"
};

struct Launch {
    argv: CStrings,
    executable: Option<CString>,
    env: Option<CStrings>,
    search: Option<PathSearch>,
    descriptors: Descriptors,
}

impl Launch {
    /// Prepares native launch data after validating nonempty argv and env names.
    #[cfg(test)]
    fn new(argv: Vec<CString>, env: Option<StringPairs>) -> Result<Self> {
        Self::with_executable(
            argv,
            env.map(CStrings::from_environment).transpose()?,
            None,
            None,
        )
    }

    fn with_executable(
        argv: Vec<CString>,
        env: Option<CStrings>,
        executable: Option<CString>,
        search_path: Option<&[u8]>,
    ) -> Result<Self> {
        let program = executable.as_ref().unwrap_or_else(|| &argv[0]);
        let search = if program.as_bytes().contains(&b'/') {
            None
        } else {
            let inherited;
            let path = if search_path.is_some() {
                search_path
            } else if let Some(env) = &env {
                env.strings
                    .iter()
                    .find_map(|entry| entry.to_bytes().strip_prefix(b"PATH="))
            } else {
                inherited = std::env::var_os("PATH");
                inherited.as_deref().map(OsStrExt::as_bytes)
            };
            Some(PathSearch::new(path.unwrap_or(b"/bin:/usr/bin"), program)?)
        };
        Ok(Self {
            argv: CStrings::new(argv)?,
            executable,
            env,
            search,
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
    unsafe fn exec_child(
        &mut self,
        close_fds: bool,
        attributes: &ChildAttributes,
        inherited_pipe: Option<&libc::sigaction>,
    ) -> io::Result<()> {
        if let Some(mask) = attributes.mask {
            // SAFETY: this is the forked child; umask never fails.
            unsafe { libc::umask(mask) };
        }
        if let Some(action) = inherited_pipe {
            // Command can reset SIGPIPE before pre_exec. Restore the parent's
            // actual action; exec itself handles any caught signal normally.
            // SAFETY: sigaction is async-signal-safe and action was read before fork.
            if unsafe { libc::sigaction(libc::SIGPIPE, action, std::ptr::null_mut()) } < 0 {
                return Err(io::Error::last_os_error());
            }
        } else {
            for signal in [libc::SIGPIPE, libc::SIGXFSZ] {
                // SAFETY: signal is async-signal-safe and both constants are valid.
                if unsafe { libc::signal(signal, libc::SIG_DFL) } == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        // SAFETY: setsid and setpgid are async-signal-safe. Apply them in
        // CPython's order; a new session leader cannot also change its group.
        if attributes.new_session && unsafe { libc::setsid() } < 0 {
            return Err(io::Error::last_os_error());
        }
        if attributes.group >= 0
            // SAFETY: this is the forked child and group is a validated pid_t.
            && unsafe { libc::setpgid(0, attributes.group) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if close_fds {
            // SAFETY: the forked child owns its descriptor table.
            unsafe { self.descriptors.apply()? };
        }
        // A false-valued pass_fds iterable on the fork path can still yield descriptors.
        // SAFETY: this runs only after fork. No allocator or callback is used.
        unsafe { pass_fds::inherit(&attributes.keep)? };
        let mut saved_error = None;
        let mut last_error = libc::ENOENT;
        let mut attempt = |executable| {
            // SAFETY: the immutable C arrays are NUL-terminated and live through
            // exec. None preserves the inherited environment exactly as bytes.
            unsafe {
                if let Some(env) = &self.env {
                    libc::execve(
                        executable,
                        self.argv.pointers.as_ptr(),
                        env.pointers.as_ptr(),
                    );
                } else {
                    libc::execv(executable, self.argv.pointers.as_ptr());
                }
            }
            last_error = io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
            if saved_error.is_none() && !matches!(last_error, libc::ENOENT | libc::ENOTDIR) {
                saved_error = Some(last_error);
            }
        };
        let program = self
            .executable
            .as_ref()
            .unwrap_or_else(|| &self.argv.strings[0]);
        if let Some(search) = &mut self.search {
            search.visit(program, attempt);
        } else {
            attempt(program.as_ptr());
        }
        Err(launch_error::child_errno(
            launch_error::Site::Executable,
            saved_error.unwrap_or(last_error),
        ))
    }

    fn spawn(self, cwd: Option<&CString>, streams: u32) -> io::Result<Child> {
        self.spawn_tagged(cwd, streams, Default::default(), ChildAttributes::default())
            .map_err(|err| launch_error::decode(err).1)
    }

    fn spawn_tagged(
        mut self,
        cwd: Option<&CString>,
        streams: u32,
        redirects: popen::Redirects<'_>,
        attributes: ChildAttributes,
    ) -> io::Result<Child> {
        let descriptors = redirects.fds;
        let mut pipes = redirects.prepared.unwrap_or_default();
        // STDOUT in the stdin slot reaches child descriptor setup as fd -2.
        if (streams >> 12) & 15 == 5 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        // Check before opening any stdio or launch-error pipes: an invalid caller
        // fd must not accidentally become valid when Command allocates a pipe.
        for &fd in &attributes.keep {
            // SAFETY: F_GETFD validates the supplied fd without changing it.
            if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // Check the original descriptor table before duplicating anything. A
        // duplicate for stdout must not make a closed stderr descriptor valid.
        if pipes.closed_descriptor {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        for fd in descriptors.into_iter().filter(|&fd| fd >= 0) {
            // SAFETY: fcntl validates the supplied integer even when it is closed.
            if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // Duplicate the parent's explicit descriptors before setting up either
        // child stream. stderr=STDOUT instead follows the child's chosen stdout.
        fn output(mode: u32, descriptor: RawFd) -> io::Result<Stdio> {
            Ok(match mode {
                0 | 5 => Stdio::inherit(),
                1 => Stdio::piped(),
                2 => Stdio::null(),
                3 => Stdio::from(io::stdout().as_fd().try_clone_to_owned()?),
                4 => Stdio::from(io::stderr().as_fd().try_clone_to_owned()?),
                6 | 7 => {
                    // SAFETY: fcntl validates the source and returns a new owned
                    // descriptor with the same open file description (and offset).
                    let fd = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
                    if fd < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    // SAFETY: only the new descriptor is transferred to Command.
                    Stdio::from(unsafe { OwnedFd::from_raw_fd(fd) })
                }
                8 => Stdio::from(io::stdin().as_fd().try_clone_to_owned()?),
                _ => fail("SystemError", "invalid subprocess stream mode"),
            })
        }
        let program = self
            .executable
            .as_ref()
            .unwrap_or_else(|| &self.argv.strings[0]);
        let mut command = Command::new(OsStr::from_bytes(program.as_bytes()));
        let stderr = (streams >> 4) & 15;
        let modes = [(streams >> 12) & 15, streams & 15, stderr];
        // Move one shared devnull into its first target; the child can duplicate
        // that standard fd to the remaining targets without another parent fd.
        let devnull = pipes
            .devnull
            .as_ref()
            .and_then(|_| modes.iter().position(|&m| m == 2));
        let mut stream = |i: usize| {
            if let Some(end) = pipes.ends[i].take() {
                Ok(end)
            } else if Some(i) == devnull {
                Ok(Stdio::from(pipes.devnull.take().expect("prepared devnull")))
            } else if modes[i] == 2 && devnull.is_some() {
                Ok(Stdio::inherit())
            } else {
                output(modes[i], descriptors[i])
            }
        };
        command
            .stdin(stream(0)?)
            .stdout(stream(1)?)
            .stderr(stream(2)?);
        let cwd = cwd.cloned();
        let inherited_pipe = if streams & (1 << 20) != 0 {
            let mut action = std::mem::MaybeUninit::uninit();
            // SAFETY: a null new action queries the disposition without changing it.
            if unsafe { libc::sigaction(libc::SIGPIPE, std::ptr::null(), action.as_mut_ptr()) } < 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: sigaction successfully initialized its output.
            Some(unsafe { action.assume_init() })
        } else {
            None
        };
        // SAFETY: this private closure contains only owned immutable native data;
        // exec_child performs no allocation, locking, callback, or language drop.
        // It never returns Ok, so Command cannot fall through to libc execvp.
        unsafe {
            command.pre_exec(move || {
                if let Some(directory) = &cwd
                    && libc::chdir(directory.as_ptr()) < 0
                {
                    let errno = io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(libc::EIO);
                    return Err(launch_error::child_errno(
                        launch_error::Site::Directory,
                        errno,
                    ));
                }
                if let Some(source) = devnull {
                    for (i, mode) in modes.into_iter().enumerate().skip(source + 1) {
                        if mode == 2 && libc::dup2(source as RawFd, i as RawFd) < 0 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                }
                if stderr == 5 && libc::dup2(libc::STDOUT_FILENO, libc::STDERR_FILENO) < 0 {
                    return Err(io::Error::last_os_error());
                }
                self.exec_child(
                    streams & (1 << 17) == 0,
                    &attributes,
                    inherited_pipe.as_ref(),
                )
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

fn poll_milliseconds(timeout: f64) -> Result<i32> {
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
    let milliseconds = (timeout * 1000.0).ceil();
    if milliseconds >= 9_223_372_036_855.0 {
        return Err(Error::new(
            Kind::OverflowError,
            "timestamp too large to convert to C PyTime_t",
        ));
    }
    if milliseconds > f64::from(i32::MAX) {
        return Err(Error::new(Kind::OverflowError, "timeout is too large"));
    }
    Ok(milliseconds as i32)
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
fn append_output(output: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    output
        .try_reserve(bytes.len())
        .map_err(|_| io::ErrorKind::OutOfMemory)?;
    output.extend_from_slice(bytes);
    Ok(())
}

fn capture_allocation_failed(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::OutOfMemory && error.raw_os_error().is_none()
}

fn capture_error(error: io::Error) -> Error {
    if capture_allocation_failed(&error) {
        Error::allocation()
    } else {
        os_error(error)
    }
}

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
                Ok(count) => append_output(&mut outputs[index], &buffer[..count])?,
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

// POSIX may move its environ array during setenv or unsetenv. Hold this lock
// while walking the native array or changing it in either execution mode.
static ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Borrows the POSIX environment while no native environment change is in progress.
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

/// Copy the raw entries used by the POSIX module before its first environment mutation.
pub(crate) fn initial_environment() -> Vec<(Vec<u8>, Vec<u8>)> {
    let _guard = ENVIRONMENT.lock().unwrap();
    let mut seen = std::collections::HashSet::new();
    // SAFETY: the lock keeps the array stable until the copy completes.
    unsafe { environment_entries(native_environ()) }
        .filter(|(name, _)| seen.insert(*name))
        .map(|(name, value)| (name.to_vec(), value.to_vec()))
        .collect()
}

/// Borrow names without examining value bytes. The value pointer is only read by a
/// consumer that actually needs that entry's value.
///
/// # Safety
/// The array (possibly null) and its C strings must be live and immutable for `'a`.
unsafe fn environment_names<'a>(
    mut entries: *const *const libc::c_char,
) -> impl Iterator<Item = (&'a [u8], *const libc::c_char)> {
    std::iter::from_fn(move || {
        loop {
            // SAFETY: each live entry has a successor; the terminating pointer is null.
            let entry = unsafe { entries.as_ref().copied()? };
            if entry.is_null() {
                return None;
            }
            // SAFETY: search the live string only up to its first '=' or NUL.
            let split = unsafe {
                entries = entries.add(1);
                libc::strchr(entry, i32::from(b'='))
            };
            if !split.is_null() {
                // SAFETY: split belongs to the same C string. Its suffix is NUL-terminated.
                return Some(unsafe {
                    let len = split.cast_const().offset_from(entry) as usize;
                    (
                        std::slice::from_raw_parts(entry.cast::<u8>(), len),
                        split.add(1).cast_const(),
                    )
                });
            }
        }
    })
}

/// # Safety
/// The array (possibly null) and its C strings must be live and immutable for `'a`.
unsafe fn environment_entries<'a>(
    entries: *const *const libc::c_char,
) -> impl Iterator<Item = (&'a [u8], &'a [u8])> {
    // SAFETY: each value points to the suffix of a string rooted for `'a`.
    unsafe {
        environment_names(entries).map(|(name, value)| (name, CStr::from_ptr(value).to_bytes()))
    }
}

/// Inspect one POSIX environment value without allocating a temporary string.
pub(super) fn with_environment_value<T>(key: &[u8], inspect: impl FnOnce(&[u8]) -> T) -> Option<T> {
    let _guard = ENVIRONMENT.lock().unwrap();
    // SAFETY: the lock keeps the matching C string stable through the callback.
    unsafe {
        environment_names(native_environ())
            .find(|(name, _)| *name == key)
            .map(|(_, value)| inspect(CStr::from_ptr(value).to_bytes()))
    }
}

/// Update the same process environment inherited by newly spawned children.
pub(crate) fn change_environment(key: &CStr, value: Option<&CStr>) -> Result<()> {
    let guard = ENVIRONMENT.lock().unwrap();
    change_environment_locked(key, value, &guard)
}

fn change_environment_locked(
    key: &CStr,
    value: Option<&CStr>,
    _guard: &std::sync::MutexGuard<'_, ()>,
) -> Result<()> {
    let status = if let Some(value) = value {
        if key.to_bytes().contains(&b'=') {
            return Err(Error::new(
                Kind::ValueError,
                "illegal environment variable name",
            ));
        }
        // SAFETY: both inputs are live terminated strings; setenv copies them.
        unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) }
    } else {
        // SAFETY: key stays terminated and live through the synchronous call.
        unsafe { libc::unsetenv(key.as_ptr()) }
    };
    if status == 0 {
        Ok(())
    } else {
        Err(super::errno_error(io::Error::last_os_error()))
    }
}

fn set_environment_bytes(key: &[u8], value: &[u8]) -> Result<()> {
    change_environment(&cstring(key)?, Some(&cstring(value)?))
}

fn set_environment_text(key: &Str, value: &Str) -> Result<()> {
    let encoding = crate::sys::filesystem_encoding()?;
    let encode = |text: &Str| super::Codec::encode_str(text, &encoding, "surrogateescape");
    set_environment_bytes(&encode(key)?, &encode(value)?)
}

/// Assign a typed bytes key and value to the process environment.
///
/// # Safety
/// Both bytes objects are rooted until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_set_bytes(key: *const Bytes, value: *const Bytes) {
    // SAFETY: the caller supplies rooted bytes objects.
    finish(unsafe { set_environment_bytes(&*key, &*value) }, ());
}

/// Assign a typed text key and value to the process environment.
///
/// # Safety
/// Both strings are rooted by the caller until this function returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_set(key: *const Str, value: *const Str) {
    // SAFETY: the caller supplies rooted strings.
    finish(unsafe { set_environment_text(&*key, &*value) }, ());
}

/// Remove POSIX names in mapping order. Entries remain raw bytes, including names
/// inherited with an undecodable spelling.
#[unsafe(no_mangle)]
pub extern "C" fn imre_environ_clear() {
    for (key, _) in initial_environment() {
        if let Err(error) = cstring(&key).and_then(|key| change_environment(&key, None)) {
            error.raise();
            return;
        }
    }
}

/// # Safety
/// The caller roots an exact dict[str, str] or dict[bytes, bytes] matching `bytes`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_update(mapping: *const Object, bytes: u32) {
    // SAFETY: the exact dictionary stays rooted. Its items iterator owns its cursor.
    let items =
        unsafe { crate::ObjectOwner::from_raw(super::mappings::imre_dict_iter(mapping, 1)) };
    let update = || {
        while let Some((key, item)) = super::mappings::pull(&items)? {
            let value = item.expect("dictionary items have values");
            // SAFETY: the item owns both words with the caller's exact homogeneous types.
            unsafe {
                if bytes != 0 {
                    set_environment_bytes(
                        &*(key.word as *const Bytes),
                        &*(value.word as *const Bytes),
                    )?;
                } else {
                    set_environment_text(&*(key.word as *const Str), &*(value.word as *const Str))?;
                }
            }
        }
        Ok(())
    };
    finish(update(), ());
}

/// Decode representable filesystem text without exposing raw entries in lookup diagnostics.
fn decode_environment_text(codec: Codec, bytes: &[u8]) -> Result<Str> {
    codec.decode(bytes, None).map(Str::from).map_err(|error| {
        if error.is_kind(Kind::UnicodeDecodeError) {
            let message = if matches!(codec, Codec::Utf8 { .. }) {
                "environment contains non-UTF-8 text"
            } else {
                "environment text cannot be decoded with the filesystem encoding"
            };
            Error::new(Kind::UnicodeDecodeError, message)
        } else {
            error
        }
    })
}

/// Returns the first matching value, or the supplied nullable default.
///
/// # Safety
/// The key and any non-null default are rooted strings. A non-null result transfers one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_get(key: *const Str, default: *const Str) -> *const Str {
    // SAFETY: the compiler roots the key for this lookup.
    let key = unsafe { &*key };
    let value = environ_item::encoded_key(key).and_then(|key| {
        let codec = Codec::lookup(&crate::sys::filesystem_encoding()?)?;
        with_environment_value(&key, |bytes| decode_environment_text(codec, bytes)).transpose()
    });
    match value {
        Ok(Some(value)) => StrOwner::into_raw(StrOwner::new(value)),
        Ok(None) => {
            if !default.is_null() {
                // SAFETY: return an independent owner of the rooted default.
                unsafe { StrOwner::increment_strong_count(default) };
            }
            default
        }
        Err(error) => error.returning(std::ptr::null()),
    }
}

/// Copies native entries in order, preserving empty names and the first duplicate.
/// Entries without '=' are ignored, as in CPython's POSIX environment snapshot.
///
/// # Safety
/// `entries` is null or a live null-terminated array of C-string pointers. The array
/// and strings remain immutable until this function returns.
unsafe fn environment_snapshot(
    entries: *const *const libc::c_char,
    codec: Codec,
) -> Result<crate::ObjectOwner> {
    // SAFETY: new transfers one fresh exact dict[str, str] owner.
    let result = unsafe { crate::ObjectOwner::from_raw(imre_dict_new(3, 3)) };
    // SAFETY: the caller roots the terminated environment for this iteration.
    for (name, value) in unsafe { environment_entries(entries) } {
        let key = StrOwner::new(decode_environment_text(codec, name)?);
        // SAFETY: both the new dictionary and exact string key remain rooted.
        if unsafe {
            imre_dict_contains(
                crate::ObjectOwner::as_ptr(&result),
                StrOwner::as_ptr(&key) as usize,
            )
        } == 0
        {
            let value = StrOwner::new(decode_environment_text(codec, value)?);
            // SAFETY: insertion acquires both rooted strings; no language alias
            // can observe this new acyclic dictionary before it is complete.
            unsafe {
                super::mappings::dict_set(
                    crate::ObjectOwner::as_ptr(&result),
                    StrOwner::as_ptr(&key) as usize,
                    StrOwner::as_ptr(&value) as usize,
                )?;
            }
        }
    }
    Ok(result)
}

/// Copy the process environment using the interpreter’s filesystem codec.
#[unsafe(no_mangle)]
pub extern "C" fn imre_environ_copy() -> *const Object {
    let result = crate::sys::filesystem_encoding().and_then(|encoding| {
        let codec = Codec::lookup(&encoding)?;
        let _guard = ENVIRONMENT.lock().unwrap();
        // SAFETY: the lock keeps the terminated environment stable during the copy.
        unsafe { environment_snapshot(native_environ(), codec) }
    });
    finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// Copies the POSIX byte mapping without passing keys or values through a text codec.
#[unsafe(no_mangle)]
pub extern "C" fn imre_environb_copy() -> *const Object {
    // SAFETY: the dictionary owns exact bytes keys and values (word kind 8).
    let dict = unsafe { crate::ObjectOwner::from_raw(imre_dict_new(8, 8)) };
    for (name, value) in initial_environment() {
        let key = crate::BytesOwner::new(name);
        let value = crate::BytesOwner::new(value);
        // SAFETY: the rooted bytes owners live through insertion, which retains each.
        let result = unsafe {
            super::mappings::dict_set(
                crate::ObjectOwner::as_ptr(&dict),
                crate::BytesOwner::as_ptr(&key) as usize,
                crate::BytesOwner::as_ptr(&value) as usize,
            )
        };
        if let Err(error) = result {
            error.raise();
            return std::ptr::null();
        }
    }
    crate::ObjectOwner::into_raw(dict)
}

fn copy_argv(words: &[usize], copy: impl Fn(usize) -> Result<CString>) -> Result<Vec<CString>> {
    let mut argv = Vec::new();
    argv.try_reserve_exact(words.len())
        .map_err(|_| Error::allocation())?;
    for &word in words {
        argv.push(copy(word)?);
    }
    Ok(argv)
}

/// Validate rooted language arguments and materialize an independent native launch.
unsafe fn prepare_process(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
) -> Result<(Launch, Option<CString>)> {
    popen::cleanup();
    // SAFETY: existing run and asynchronous ABIs accept an exact list.
    let config = popen::LaunchConfig::basic(args.cast(), 5, cwd, env, 0, 0);
    // SAFETY: the legacy arguments have their original word representations.
    let (launch, cwd, _) = unsafe {
        let cmd = WordKind::new(5).acquire(args as usize);
        validate_command_shape(&cmd, &config)?;
        prepare_command(&cmd, &config, None, None, || Ok(ChildAttributes::default()))?
    };
    Ok((launch, cwd))
}

/// Conversion error shared by POSIX launch arguments that take a C int.
fn c_int_overflow() -> Error {
    Error::new(
        Kind::OverflowError,
        "Python int too large to convert to C int",
    )
}

/// Validate the argument shape before CPython chooses a launch API or opens its fork error pipe.
///
/// # Safety
/// The command and configuration must remain rooted.
unsafe fn validate_command_shape(cmd: &OwnedWord, config: &popen::LaunchConfig) -> Result<()> {
    // SAFETY: the caller roots the exact argument representation.
    unsafe {
        if config.shell
            && cmd.kind.raw() == 5
            && object(cmd.word as *const Object)
                .0
                .with(|s| matches!(s, Storage::Path { .. } | Storage::PurePath(_)))
        {
            return Err(Error::new(
                Kind::TypeError,
                "path-like args is not allowed when shell is true",
            ));
        }
        if !config.shell && config.executable.is_null() && argv::first(cmd).is_none() {
            return Err(Error::new(Kind::IndexError, "list index out of range"));
        }
        Ok(())
    }
}

type ByteEnvironment = Vec<(Vec<u8>, Vec<u8>)>;

/// # Safety
/// The exact optional directory and environment stay rooted across conversion.
unsafe fn prepare_command(
    cmd: &OwnedWord,
    config: &popen::LaunchConfig,
    byte_env: Option<ByteEnvironment>,
    search_path: Option<&[u8]>,
    child_attributes: impl FnOnce() -> Result<ChildAttributes>,
) -> Result<(Launch, Option<CString>, ChildAttributes)> {
    // SAFETY: the caller roots the exact argv, optional directory and environment.
    unsafe {
        // Python selects args[0], validates every environment name, then
        // converts native strings. Preserve that order for mixed failures.
        let env = if config.env.is_null() {
            None
        } else {
            Some(object(config.env).0.with(|storage| {
                let Storage::Dictionary(mapping) = storage else {
                    fail("SystemError", "expected an environment dictionary");
                };
                mapping.string_pairs()
            })?)
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
        // _execute_child validates environment names before _fork_exec converts
        // its integer arguments. Conversion of argv and cwd to C strings follows.
        let attributes = child_attributes()?;
        let executable = argv::optional_path(config.executable, config.executable_kind)?;
        let mut argv = argv::copy(cmd)?;
        if config.shell {
            let shell = executable
                .as_deref()
                .filter(|p| !p.to_bytes().is_empty())
                .unwrap_or(DEFAULT_SHELL);
            argv.try_reserve(2).map_err(|_| Error::allocation())?;
            argv.splice(..0, [shell.to_owned(), c"-c".to_owned()]);
        }
        let cwd = argv::optional_path(config.cwd, config.cwd_kind)?;
        let env = match byte_env {
            Some(pairs) => Some(CStrings::from_pairs(
                pairs.iter().map(|(k, v)| (k.as_slice(), v.as_slice())),
            )?),
            None => env.map(CStrings::from_environment).transpose()?,
        };
        Ok((
            Launch::with_executable(argv, env, executable, search_path)?,
            cwd,
            attributes,
        ))
    }
}

/// Convert the drained bytes into the exact field type declared by the call site.
fn completed_stream(data: Vec<u8>, kind: WordKind) -> Result<OwnedWord> {
    let word = match kind.raw() {
        0 => 0,
        3 => StrOwner::into_raw(StrOwner::new(decode_utf8_text(data)?)) as usize,
        8 => crate::BytesOwner::into_raw(crate::BytesOwner::new(data)) as usize,
        _ => fail("SystemError", "invalid process output field kind"),
    };
    // SAFETY: each arm transfers its correctly typed owned allocation, or None.
    Ok(unsafe { OwnedWord::from_owned(word, kind) })
}

/// Transfer the exact original command and completed streams to an owned result.
/// The caller guarantees that all four field owners match the static descriptor.
unsafe fn completed_record(
    descriptor: *const RecordDescriptor,
    args: OwnedWord,
    returncode: i32,
    stdout: OwnedWord,
    stderr: OwnedWord,
) -> *mut usize {
    // SAFETY: static metadata describes these four exact fields. No fallible work
    // remains; typed stores preserve reference provenance and transfer each owner.
    unsafe {
        let result = imre_record_new(descriptor);
        result
            .cast::<*const ()>()
            .write(args.into_word() as *const ());
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
/// the original command field, an integer, and two None/str/bytes stream fields. Enter with pending clear and check it immediately afterward.
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
            positional_streams: true,
            cmd: acquire_stream(0),
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

    pub(super) fn args(values: &[&str]) -> crate::ObjectOwner {
        // SAFETY: new transfers one fresh exact list[str] owner.
        let result = unsafe { crate::ObjectOwner::from_raw(imre_seq_new(3)) };
        for value in values {
            let value = StrOwner::new(Str::new(value));
            // SAFETY: insertion acquires the rooted exact string into this list.
            unsafe {
                imre_seq_append(
                    crate::ObjectOwner::as_ptr(&result),
                    StrOwner::as_ptr(&value) as usize,
                    3,
                )
            };
        }
        result
    }

    fn environment(values: &[(&str, &str)]) -> crate::ObjectOwner {
        // SAFETY: new transfers one fresh exact dict[str, str] owner.
        let result = unsafe { crate::ObjectOwner::from_raw(imre_dict_new(3, 3)) };
        for (key, value) in values {
            let key = StrOwner::new(Str::new(key));
            let value = StrOwner::new(Str::new(value));
            // SAFETY: both strings remain rooted while insertion acquires them.
            unsafe {
                crate::objects::mappings::imre_dict_set(
                    crate::ObjectOwner::as_ptr(&result),
                    StrOwner::as_ptr(&key) as usize,
                    StrOwner::as_ptr(&value) as usize,
                );
            }
        }
        result
    }

    #[test]
    fn decode_a_real_stopped_child() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "kill -STOP $$; exit 7"])
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        let mut raw = 0;
        // SAFETY: this test owns the child, and waitpid writes one int to raw.
        let stopped = unsafe { libc::waitpid(pid, &mut raw, libc::WUNTRACED) };
        // Resume and reap before checking the decoded result.
        // SAFETY: the stopped child has not exited and its PID is still owned.
        let resumed = unsafe { libc::kill(pid, libc::SIGCONT) };
        assert_eq!(resumed, 0);
        assert_eq!(child.wait().unwrap().code(), Some(7));
        assert_eq!(stopped, pid);
        assert_eq!(returncode(ExitStatus::from_raw(raw)), -libc::SIGSTOP);
    }

    #[test]
    fn environment_entries_preserve_values_and_reject_embedded_nuls() {
        let pairs = vec![(
            StrOwner::new(Str::from("KEY")),
            StrOwner::new(Str::from("value=λ")),
        )];
        let strings = CStrings::from_environment(pairs).ok().unwrap();
        assert_eq!(strings.strings[0].to_bytes(), "KEY=value=λ".as_bytes());
        let nul = vec![(
            StrOwner::new(Str::from("KEY")),
            StrOwner::new(Str::from("before\0after")),
        )];
        assert!(CStrings::from_environment(nul).is_err());
    }

    #[test]
    fn executable_search_reuses_one_buffer_and_preserves_empty_components() {
        let Ok(mut search) = PathSearch::new(b":bin:/longest/path::", c"tool") else {
            panic!("reserve search buffers");
        };
        let buffer = search.candidate.as_ptr().cast();
        let mut expected = [
            c"tool",
            c"bin/tool",
            c"/longest/path/tool",
            c"tool",
            c"tool",
        ]
        .into_iter();
        search.visit(c"tool", |candidate| {
            assert_eq!(candidate, buffer);
            // SAFETY: visit supplies a NUL-terminated candidate for the duration of the callback.
            assert_eq!(Some(unsafe { CStr::from_ptr(candidate) }), expected.next());
        });
        assert!(expected.next().is_none());
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
                let child = imre_async_process_spawn(
                    crate::ObjectOwner::as_ptr(&argv),
                    &cwd,
                    std::ptr::null(),
                    1,
                    1,
                );
                assert!(!child.is_null());
                crate::ObjectOwner::from_raw(child)
            }
        };
        let first = spawn("first", "second");
        assert_eq!(
            // SAFETY: the first process is rooted and waits for a missing file.
            unsafe { imre_async_process_status(crate::ObjectOwner::as_ptr(&first), false) },
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
            let process = crate::ObjectOwner::as_ptr(&owner);
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
        args: &crate::ObjectOwner,
        cwd: Option<&str>,
        env: Option<&crate::ObjectOwner>,
        check: bool,
        timeout: Option<f64>,
    ) -> std::result::Result<Completed, crate::ObjectOwner> {
        const KINDS: [u32; 4] = [5, 2, 3, 3];
        let descriptor = Box::new(RecordDescriptor {
            length: KINDS.len(),
            kinds: KINDS.as_ptr(),
            semantics: std::ptr::null(),
        });
        let cwd = cwd.map(Str::new);
        let config = popen::LaunchConfig::basic(
            crate::ObjectOwner::as_ptr(args).cast(),
            5,
            cwd.as_ref().map_or(std::ptr::null(), |cwd| cwd),
            env.map_or(std::ptr::null(), crate::ObjectOwner::as_ptr),
            0x11 | (1 << 16), // capture both streams as text
            Int::small(-1).unwrap().0,
        );
        let deadline = timeout
            .as_ref()
            .map_or(0, |value| std::ptr::from_ref(value).addr());
        // SAFETY: the inputs remain rooted and the descriptor is retained with
        // every returned record; pending errors transfer into an independent Rc.
        unsafe {
            let payload =
                popen::imre_run_config(&config, 7, deadline, 0, 0, 0, check, &*descriptor)
                    .cast_mut()
                    .cast::<usize>();
            if payload.is_null() {
                assert_eq!(imre_error_pending(), 1);
                Err(crate::ObjectOwner::from_raw(imre_error_take()))
            } else {
                assert_eq!(imre_error_pending(), 0);
                Ok(Completed {
                    payload,
                    _descriptor: descriptor,
                })
            }
        }
    }

    fn success(
        args: &crate::ObjectOwner,
        cwd: Option<&str>,
        env: Option<&crate::ObjectOwner>,
    ) -> Completed {
        match run(args, cwd, env, false, None) {
            Ok(result) => result,
            Err(_) => panic!("controlled child unexpectedly failed to run"),
        }
    }

    fn failure(
        args: &crate::ObjectOwner,
        cwd: Option<&str>,
        env: Option<&crate::ObjectOwner>,
        check: bool,
    ) -> crate::ObjectOwner {
        match run(args, cwd, env, check, None) {
            Ok(_) => panic!("controlled child unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    fn assert_kind(error: &crate::ObjectOwner, kind: Kind) {
        // SAFETY: the error remains rooted and the expected kind is a valid ABI id.
        let matches =
            unsafe { imre_exception_matches(crate::ObjectOwner::as_ptr(error), kind as u32) };
        assert_eq!(matches, 1);
    }

    fn message(error: &crate::ObjectOwner) -> StrOwner {
        // SAFETY: formatting returns one owned string from the rooted exception.
        unsafe { StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(error))) }
    }

    /// Exercises the public deadline ABI and transfers the resulting pending owner.
    fn timeout_failure(args: &crate::ObjectOwner, timeout: f64) -> crate::ObjectOwner {
        match run(args, None, None, true, Some(timeout)) {
            Err(error) => error,
            Ok(_) => panic!("controlled timeout unexpectedly succeeded"),
        }
    }

    /// Reads one nullable immutable bytes owner from an exact timeout view.
    fn timeout_bytes(error: &crate::ObjectOwner, field: u32) -> Option<crate::BytesOwner> {
        // SAFETY: the caller proves TimeoutExpired and selects a nullable byte field.
        unsafe {
            let word = imre_subprocess_timeout_field(crate::ObjectOwner::as_ptr(error), field);
            (word != 0).then(|| crate::BytesOwner::from_raw(word as *const Bytes))
        }
    }

    #[test]
    fn timeout_preserves_raw_bytes_and_reaps_the_direct_child() {
        let args = args(&[
            "/bin/sh",
            "-c",
            r#"printf 'raw\r\n\377\342\202'; printf %s "$$" >&2; exec sleep 10"#,
        ]);
        let weak = crate::ObjectOwner::downgrade(&args);
        let error = timeout_failure(&args, 0.5);
        assert_kind(&error, Kind::TimeoutExpired);
        assert_kind(&error, Kind::SubprocessError);
        assert_kind(&error, Kind::Exception);
        let stdout = timeout_bytes(&error, 2).unwrap();
        let output = timeout_bytes(&error, 4).unwrap();
        assert!(crate::BytesOwner::ptr_eq(&stdout, &output));
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
                imre_exception_matches(
                    crate::ObjectOwner::as_ptr(&error),
                    Kind::CalledProcessError as u32
                ),
                0
            );
            assert_eq!(
                f64::from_bits(
                    imre_subprocess_timeout_field(crate::ObjectOwner::as_ptr(&error), 1) as u64
                ),
                0.5
            );
            let cmd = crate::ObjectOwner::from_raw(imre_subprocess_timeout_field(
                crate::ObjectOwner::as_ptr(&error),
                0,
            ) as *const Object);
            assert!(crate::ObjectOwner::ptr_eq(&cmd, &args));
            let addition = StrOwner::new(Str::new("mutated"));
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&args),
                StrOwner::as_ptr(&addition) as usize,
                3,
            );
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
        let weak = crate::ObjectOwner::downgrade(&args);
        let result = success(&args, None, Some(&environment(&[])));
        assert_eq!(result.code(), 7);
        assert_eq!(result.text(2), "é\n🦀\n");
        assert_eq!(result.text(3), "err\n");
        let addition = StrOwner::new(Str::new("later"));
        // SAFETY: the original list and added string remain rooted; the result
        // owns the same list, and checking acquires all rich exception fields.
        let error = unsafe {
            assert_eq!(
                result.payload.cast::<*const Object>().read(),
                crate::ObjectOwner::as_ptr(&args)
            );
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&args),
                StrOwner::as_ptr(&addition) as usize,
                3,
            );
            imre_subprocess_check(result.payload);
            assert_eq!(imre_error_pending(), 1);
            crate::ObjectOwner::from_raw(imre_error_take())
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
            let cmd = crate::ObjectOwner::from_raw(imre_subprocess_error_field(
                crate::ObjectOwner::as_ptr(&error),
                0,
            ) as *const Object);
            assert!(crate::ObjectOwner::ptr_eq(&cmd, &args));
            assert_eq!(
                Int(imre_subprocess_error_field(
                    crate::ObjectOwner::as_ptr(&error),
                    1
                ))
                .as_small(),
                Some(7)
            );
            let stdout = StrOwner::from_raw(imre_subprocess_error_field(
                crate::ObjectOwner::as_ptr(&error),
                2,
            ) as *const Str);
            let output = StrOwner::from_raw(imre_subprocess_error_field(
                crate::ObjectOwner::as_ptr(&error),
                4,
            ) as *const Str);
            assert!(StrOwner::ptr_eq(&stdout, &output));
            assert_eq!(stdout.as_str(), "é\n🦀\n");
            let stderr = StrOwner::from_raw(imre_subprocess_error_field(
                crate::ObjectOwner::as_ptr(&error),
                3,
            ) as *const Str);
            assert_eq!(stderr.as_str(), "err\n");
            let addition = StrOwner::new(Str::new("newest"));
            imre_seq_append(
                crate::ObjectOwner::as_ptr(&args),
                StrOwner::as_ptr(&addition) as usize,
                3,
            );
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
        assert_eq!(
            message(&error).as_str(),
            "'utf-8' codec can't decode byte 0xff in position 3: invalid start byte"
        );
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
        let code = unsafe {
            Int(imre_subprocess_error_field(
                crate::ObjectOwner::as_ptr(&error),
                1,
            ))
            .as_small()
        };
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
            crate::ObjectOwner::from_raw(imre_exception_new(
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
                    imre_subprocess_error_field(
                        crate::ObjectOwner::as_ptr(&ordinary),
                        field as u32
                    ),
                    0
                );
                assert_eq!(imre_error_pending(), 1);
                crate::ObjectOwner::from_raw(imre_error_take())
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
                b"\xfe=\xff",
                b"=ignored",
                b"malformed",
                b"\xff",
            ]
            .into_iter()
            .map(|entry| CString::new(entry).unwrap())
            .collect(),
        )
        .ok()
        .unwrap();
        // SAFETY: the immutable CStrings owner roots the terminated native array.
        let copy = unsafe { environment_snapshot(entries.pointers.as_ptr(), Codec::Latin1) }
            .unwrap_or_else(|_| panic!("controlled environment unexpectedly failed"));
        // SAFETY: the owned dictionary roots its exact strings while they are copied.
        let pairs = unsafe {
            copy.0.with(|storage| {
                let Storage::Dictionary(mapping) = storage else {
                    panic!("environment snapshot must be a dictionary");
                };
                mapping.string_pairs().ok().unwrap()
            })
        };
        assert_eq!(
            pairs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>(),
            [
                ("", "allowed"),
                ("IMRE_KEY", "first"),
                ("IMRE_VALUE", "left=right"),
                ("þ", "ÿ"),
            ]
        );
        for (entry, codec, expected_message) in [
            (
                b"KEY=\xff".as_slice(),
                Codec::Utf8 { signature: false },
                "environment contains non-UTF-8 text",
            ),
            (
                b"KEY=\xc3\xa9".as_slice(),
                Codec::Ascii,
                "environment text cannot be decoded with the filesystem encoding",
            ),
        ] {
            let entries = CStrings::new(vec![CString::new(entry).unwrap()])
                .ok()
                .unwrap();
            // SAFETY: the array is valid, but the value is not representable in this codec.
            match unsafe { environment_snapshot(entries.pointers.as_ptr(), codec) } {
                Ok(_) => panic!("unrepresentable environment text unexpectedly succeeded"),
                Err(error) => error.raise(),
            }
            // SAFETY: decoding published exactly one owned pending exception.
            let error = unsafe { crate::ObjectOwner::from_raw(imre_error_take()) };
            assert_kind(&error, Kind::UnicodeDecodeError);
            assert_eq!(message(&error).as_str(), expected_message);
        }
    }

    #[test]
    fn environment_copy_is_owned_without_logging_host_values() {
        let copy = imre_environ_copy();
        let encoding = crate::sys::filesystem_encoding().ok().unwrap();
        let codec = Codec::lookup(&encoding).ok().unwrap();
        if initial_environment().iter().any(|(key, value)| {
            [key, value]
                .into_iter()
                .any(|bytes| codec.decode(bytes, None).is_err())
        }) {
            assert!(copy.is_null());
            // SAFETY: failed decoding publishes one pending exception owner.
            let error = unsafe { crate::ObjectOwner::from_raw(imre_error_take()) };
            assert_kind(&error, Kind::UnicodeDecodeError);
            return;
        }
        assert!(!copy.is_null());
        // SAFETY: the snapshot transfers one exact dictionary owner.
        let copy = unsafe { crate::ObjectOwner::from_raw(copy) };
        let key = StrOwner::new(Str::new("IMRE_RUNTIME_CONTROLLED_COPY_KEY"));
        let value = StrOwner::new(Str::new("controlled-copy-only"));
        let previous = std::env::var_os(key.as_str());
        // SAFETY: insertion acquires the two exact rooted strings into the owned
        // dictionary; it has no link back to the process's native environment.
        unsafe {
            crate::objects::mappings::imre_dict_set(
                crate::ObjectOwner::as_ptr(&copy),
                StrOwner::as_ptr(&key) as usize,
                StrOwner::as_ptr(&value) as usize,
            )
        };
        // A boolean-only assertion avoids printing any inherited environment value.
        assert!(std::env::var_os(key.as_str()) == previous);
        assert_eq!(imre_error_pending(), 0);
    }
}
