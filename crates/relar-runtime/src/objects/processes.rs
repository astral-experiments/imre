//! Sealed synchronous Linux processes with captured strict UTF-8 text.
//!
//! Native launch data owns its bytes before fork. The child performs only
//! async-signal-safe descriptor, signal, and exec operations; language owners
//! remain in the parent. Both output pipes are drained before decoding or check.

use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::rc::Rc;

use super::exceptions::{Error, Kind, Result, finish, raise_process};
use super::files::{decode_utf8_text, os_error};
use super::mappings::{relar_dict_contains, relar_dict_new, relar_dict_set};
use super::records::{RecordDescriptor, relar_record_new};
use super::words::WordKind;
use super::{Object, Storage, Str, acquire_rc, fail, object};

unsafe extern "C" {
    static environ: *const *const libc::c_char;
}

/// Rich failures keep the original argv list, so later aliases affect str(error).
/// The sealed string list cannot point back to the exception or form a cycle.
pub(super) struct ProcessError {
    cmd: Rc<Object>,
    returncode: i32,
    stdout: Rc<Str>,
    stderr: Rc<Str>,
}

impl ProcessError {
    pub(super) fn field(&self, field: u32) -> usize {
        match field {
            0 => Rc::into_raw(self.cmd.clone()) as usize,
            1 => exit_word(self.returncode),
            2 | 4 => Rc::into_raw(self.stdout.clone()) as usize,
            3 => Rc::into_raw(self.stderr.clone()) as usize,
            _ => fail("SystemError", "invalid process exception field"),
        }
    }

    pub(super) fn message(&self) -> Str {
        let mut cmd = String::new();
        // SAFETY: this owner roots an exact acyclic list[str] during repr.
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

/// Uses Python's canonical Linux enum names, including real-time endpoints only.
fn signal_name(signal: i32) -> Option<&'static str> {
    const NAMES: [&str; 31] = [
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
    ];
    if signal == libc::SIGRTMIN() {
        Some("SIGRTMIN")
    } else if signal == libc::SIGRTMAX() {
        Some("SIGRTMAX")
    } else {
        usize::try_from(signal - 1)
            .ok()
            .and_then(|index| NAMES.get(index).copied())
    }
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
    Range,
    Snapshot(Vec<RawFd>),
}

impl Descriptors {
    /// Probe without touching a real descriptor. Older or restricted kernels use
    /// a pre-fork /proc snapshot, including open fds above a lowered RLIMIT_NOFILE.
    fn prepare() -> io::Result<Self> {
        // SAFETY: UINT_MAX exceeds every valid signed RawFd, so the probe changes
        // no descriptor. Only support for the CLOEXEC flag is being tested.
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
        for entry in fs::read_dir("/proc/self/fd")? {
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
            // is async-signal-safe and both arguments are valid Linux constants.
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

    fn spawn(self, cwd: Option<&CString>) -> io::Result<Child> {
        let mut command = Command::new(OsStr::from_bytes(self.argv.strings[0].as_bytes()));
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(OsStr::from_bytes(cwd.as_bytes()));
        }
        // SAFETY: this private closure contains only owned immutable native data;
        // exec_child performs no allocation, locking, callback, or language drop.
        // It never returns Ok, so Command cannot fall through to libc execvp.
        unsafe { command.pre_exec(move || self.exec_child()) };
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
    let mut child = ChildGuard {
        child,
        reaped: false,
    };
    let mut stdout = child
        .child
        .stdout
        .take()
        .expect("captured stdout was configured");
    let mut stderr = child
        .child
        .stderr
        .take()
        .expect("captured stderr was configured");
    nonblocking(stdout.as_raw_fd())?;
    nonblocking(stderr.as_raw_fd())?;
    let mut polls = [stdout.as_raw_fd(), stderr.as_raw_fd()].map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    let mut outputs = [Vec::new(), Vec::new()];
    let mut remaining = 2;
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
                stdout.read(&mut buffer)
            } else {
                stderr.read(&mut buffer)
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

/// Copies native entries in order, preserving empty names and the first duplicate.
/// Entries without '=' are ignored, as in CPython's POSIX environment snapshot.
///
/// # Safety
/// `entries` is null or a live null-terminated array of C-string pointers. The array
/// and strings remain immutable until this function returns.
unsafe fn environment_snapshot(mut entries: *const *const libc::c_char) -> Result<Rc<Object>> {
    // SAFETY: new transfers one fresh exact dict[str, str] owner.
    let result = unsafe { Rc::from_raw(relar_dict_new(3, 3)) };
    let decode = |bytes: &[u8]| {
        std::str::from_utf8(bytes).map(Str::from).map_err(|_| {
            Error::new(
                Kind::UnicodeDecodeError,
                "environment contains non-UTF-8 text",
            )
        })
    };
    while !entries.is_null() {
        // SAFETY: the live array has a terminating null pointer.
        let entry = unsafe { entries.read() };
        if entry.is_null() {
            break;
        }
        // SAFETY: this nonnull entry is a terminated C string, and the following
        // array element exists until the terminating null entry is reached.
        let bytes = unsafe {
            entries = entries.add(1);
            CStr::from_ptr(entry).to_bytes()
        };
        let Some(split) = bytes.iter().position(|&byte| byte == b'=') else {
            continue;
        };
        let key = Rc::new(decode(&bytes[..split])?);
        // SAFETY: both the new dictionary and exact string key remain rooted.
        if unsafe { relar_dict_contains(Rc::as_ptr(&result), Rc::as_ptr(&key) as usize) } == 0 {
            let value = Rc::new(decode(&bytes[split + 1..])?);
            // SAFETY: insertion acquires both rooted strings; no language alias
            // can observe this new acyclic dictionary before it is complete.
            unsafe {
                relar_dict_set(
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
pub extern "C" fn relar_environ_copy() -> *const Object {
    // SAFETY: Linux supplies a terminated environment, and this standalone runtime
    // has no environment mutation or language threads. Rust vars_os omits empty
    // names, so the raw POSIX entries are required for Python-compatible copies.
    let result = unsafe { environment_snapshot(environ) };
    finish(result.map(Rc::into_raw), std::ptr::null())
}

/// Runs one argv command with inherited stdin and two captured UTF-8 text streams.
/// `env=None` inherits; an explicit dictionary replaces the environment. No shell
/// interpretation occurs, including executable files without a valid format.
///
/// # Safety
/// Root an exact list[str], nullable UTF-8 cwd, and nullable dict[str, str]. `check`
/// is 0 or 1. `descriptor` is immutable static metadata with fields [5, 2, 3, 3].
/// Enter with pending clear. Success transfers one fully initialized record owner;
/// null means pending must be checked before using the result. Inputs are borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_subprocess_run(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    check: u8,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: only independent native strings escape the exact input storage.
    let prepared = unsafe {
        (|| {
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
            let argv = object(args).0.with(|storage| {
                let values = storage.sequence();
                values.check_kind(3);
                values
                    .values
                    .iter()
                    .map(|&word| cstring((&*(word as *const Str)).as_bytes()))
                    .collect::<Result<Vec<_>>>()
            })?;
            let cwd = (!cwd.is_null())
                .then(|| cstring((&*cwd).as_bytes()))
                .transpose()?;
            Ok((Launch::new(argv, env)?, cwd))
        })()
    };
    let (launch, cwd) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return error.returning(std::ptr::null_mut()),
    };
    let result = launch
        .spawn(cwd.as_ref())
        .and_then(capture)
        .map_err(os_error)
        .and_then(|(status, stdout, stderr)| {
            let returncode = status
                .code()
                .unwrap_or_else(|| -status.signal().unwrap_or(0));
            // Python decodes stdout before stderr, even when check=True would fail.
            Ok((
                returncode,
                Rc::new(decode_utf8_text(stdout)?),
                Rc::new(decode_utf8_text(stderr)?),
            ))
        });
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
        let result = relar_record_new(descriptor);
        result.cast::<*const Object>().write(Rc::into_raw(args));
        result.add(1).write(exit_word(returncode));
        result
            .add(2)
            .cast::<*const Str>()
            .write(Rc::into_raw(stdout));
        result
            .add(3)
            .cast::<*const Str>()
            .write(Rc::into_raw(stderr));
        result
    }
}

/// Raises the same rich failure as run(check=True), retaining the result fields.
///
/// # Safety
/// `record` is a rooted, fully initialized sealed CompletedProcess record with
/// fields [5, 2, 3, 3]. Enter with pending clear and check it immediately afterward.
/// Neither the record nor any field owner is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_subprocess_check(record: *const usize) {
    // SAFETY: the sealed readonly returncode was produced by exit_word from i32.
    let returncode = unsafe { (record.add(1).read() as isize >> 1) as i32 };
    if returncode == 0 {
        return;
    }
    // SAFETY: each exact pointer field remains rooted while acquiring its owner;
    // no field reference or record borrow escapes into pending error replacement.
    let error = unsafe {
        ProcessError {
            cmd: acquire_rc(record.cast::<*const Object>().read() as usize),
            returncode,
            stdout: acquire_rc(record.add(2).cast::<*const Str>().read() as usize),
            stderr: acquire_rc(record.add(3).cast::<*const Str>().read() as usize),
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
        relar_error_pending, relar_error_take, relar_exception_matches, relar_exception_message,
        relar_exception_new, relar_subprocess_error_field,
    };
    use super::super::records::relar_record_release;
    use super::super::{relar_seq_append, relar_seq_new};
    use crate::Int;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "relar-runtime-processes-{}-{}",
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
            unsafe { relar_record_release(self.payload) };
        }
    }

    fn args(values: &[&str]) -> Rc<Object> {
        // SAFETY: new transfers one fresh exact list[str] owner.
        let result = unsafe { Rc::from_raw(relar_seq_new(3)) };
        for value in values {
            let value = Rc::new(Str::new(value));
            // SAFETY: insertion acquires the rooted exact string into this list.
            unsafe { relar_seq_append(Rc::as_ptr(&result), Rc::as_ptr(&value) as usize, 3) };
        }
        result
    }

    fn environment(values: &[(&str, &str)]) -> Rc<Object> {
        // SAFETY: new transfers one fresh exact dict[str, str] owner.
        let result = unsafe { Rc::from_raw(relar_dict_new(3, 3)) };
        for (key, value) in values {
            let key = Rc::new(Str::new(key));
            let value = Rc::new(Str::new(value));
            // SAFETY: both strings remain rooted while insertion acquires them.
            unsafe {
                relar_dict_set(
                    Rc::as_ptr(&result),
                    Rc::as_ptr(&key) as usize,
                    Rc::as_ptr(&value) as usize,
                );
            }
        }
        result
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
            let payload = relar_subprocess_run(
                Rc::as_ptr(args),
                cwd.as_ref().map_or(std::ptr::null(), |cwd| cwd),
                env.map_or(std::ptr::null(), Rc::as_ptr),
                u8::from(check),
                &*descriptor,
            );
            if payload.is_null() {
                assert_eq!(relar_error_pending(), 1);
                Err(Rc::from_raw(relar_error_take()))
            } else {
                assert_eq!(relar_error_pending(), 0);
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
        let matches = unsafe { relar_exception_matches(Rc::as_ptr(error), kind as u32) };
        assert_eq!(matches, 1);
    }

    fn message(error: &Rc<Object>) -> Rc<Str> {
        // SAFETY: formatting returns one owned string from the rooted exception.
        unsafe { Rc::from_raw(relar_exception_message(Rc::as_ptr(error))) }
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
            relar_seq_append(Rc::as_ptr(&args), Rc::as_ptr(&addition) as usize, 3);
            relar_subprocess_check(result.payload);
            assert_eq!(relar_error_pending(), 1);
            Rc::from_raw(relar_error_take())
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
                relar_subprocess_error_field(Rc::as_ptr(&error), 0) as *const Object
            );
            assert!(Rc::ptr_eq(&cmd, &args));
            assert_eq!(
                Int(relar_subprocess_error_field(Rc::as_ptr(&error), 1)).as_small(),
                Some(7)
            );
            let stdout = Rc::<Str>::from_raw(
                relar_subprocess_error_field(Rc::as_ptr(&error), 2) as *const Str
            );
            let output = Rc::<Str>::from_raw(
                relar_subprocess_error_field(Rc::as_ptr(&error), 4) as *const Str
            );
            assert!(Rc::ptr_eq(&stdout, &output));
            assert_eq!(stdout.as_str(), "é\n🦀\n");
            let stderr = Rc::<Str>::from_raw(
                relar_subprocess_error_field(Rc::as_ptr(&error), 3) as *const Str
            );
            assert_eq!(stderr.as_str(), "err\n");
            let addition = Rc::new(Str::new("newest"));
            relar_seq_append(Rc::as_ptr(&args), Rc::as_ptr(&addition) as usize, 3);
        }
        assert!(message(&error).contains("'newest']' returned non-zero exit status 7."));
        drop(result);
        drop(args);
        assert!(weak.upgrade().is_some());
        drop(error);
        assert!(weak.upgrade().is_none());
        assert_eq!(relar_error_pending(), 0);
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
        unsafe { relar_subprocess_check(result.payload) };
        assert_eq!(relar_error_pending(), 0);

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
        std::os::unix::fs::symlink("/usr/bin/python3", directory.0.join("controlled-child"))
            .unwrap();
        let controlled = environment(&[
            ("PATH", ""),
            ("RELAR_TEST_VALUE", "controlled"),
            ("LC_ALL", "C.UTF-8"),
        ]);
        let result = success(
            &args(&[
                "controlled-child",
                "-c",
                "import os; print(os.environ['RELAR_TEST_VALUE']); print(len(os.environ))",
            ]),
            directory.0.to_str(),
            Some(&controlled),
        );
        assert_eq!(result.code(), 0);
        assert_eq!(result.text(2), "controlled\n3\n");

        let empty = environment(&[]);
        let result = success(&args(&["env"]), None, Some(&empty));
        assert_eq!(result.text(2), "");
        let ordered = environment(&[("RELAR_B", "two"), ("RELAR_A", "one")]);
        let result = success(&args(&["/usr/bin/env"]), None, Some(&ordered));
        assert_eq!(result.text(2), "RELAR_B=two\nRELAR_A=one\n");

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
                args(&["missing-relar-controlled-command"]),
                None,
                empty.clone(),
                Kind::FileNotFoundError,
            ),
            (
                args(&["/bin/true", "a\0b"]),
                None,
                empty.clone(),
                Kind::ValueError,
            ),
            (
                args(&["/bin/true"]),
                Some("a\0b"),
                empty.clone(),
                Kind::ValueError,
            ),
            (
                args(&["/bin/true"]),
                None,
                environment(&[("bad=key", "value")]),
                Kind::ValueError,
            ),
            (
                args(&["/bin/true"]),
                None,
                environment(&[("key", "bad\0value")]),
                Kind::ValueError,
            ),
        ] {
            assert_kind(&failure(&argv, cwd, Some(&env), false), kind);
        }
        let invalid = environment(&[("early", "null\0value"), ("bad=key", "value")]);
        for (argv, cwd) in [
            (args(&["/bin/true", "null\0arg"]), None),
            (args(&["/bin/true"]), Some("null\0cwd")),
            (args(&["/bin/true"]), None),
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
        let code = unsafe { Int(relar_subprocess_error_field(Rc::as_ptr(&error), 1)).as_small() };
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
            Rc::from_raw(relar_exception_new(
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
                    relar_subprocess_error_field(Rc::as_ptr(&ordinary), field as u32),
                    0
                );
                assert_eq!(relar_error_pending(), 1);
                Rc::from_raw(relar_error_take())
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
        let (status, stdout, _) = capture(launch.spawn(None).unwrap()).unwrap();
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
                b"RELAR_KEY=first",
                b"RELAR_KEY=second",
                b"RELAR_KEY=\xff",
                b"RELAR_VALUE=left=right",
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
                (Str::new("RELAR_KEY"), Str::new("first")),
                (Str::new("RELAR_VALUE"), Str::new("left=right")),
            ]
        );
        let entries = CStrings::new(vec![CString::new(b"KEY=\xff".as_slice()).unwrap()]);
        // SAFETY: the immutable array is valid but contains undecodable value text.
        match unsafe { environment_snapshot(entries.pointers.as_ptr()) } {
            Ok(_) => panic!("non-UTF-8 environment unexpectedly succeeded"),
            Err(error) => error.raise(),
        }
        // SAFETY: decoding published exactly one owned pending exception.
        let error = unsafe { Rc::from_raw(relar_error_take()) };
        assert_kind(&error, Kind::UnicodeDecodeError);
        assert_eq!(
            message(&error).as_str(),
            "environment contains non-UTF-8 text"
        );
    }

    #[test]
    fn environment_copy_is_owned_without_logging_host_values() {
        let copy = relar_environ_copy();
        if std::env::vars_os()
            .any(|(key, value)| key.to_str().is_none() || value.to_str().is_none())
        {
            assert!(copy.is_null());
            // SAFETY: failed decoding publishes one pending exception owner.
            let error = unsafe { Rc::from_raw(relar_error_take()) };
            assert_kind(&error, Kind::UnicodeDecodeError);
            return;
        }
        assert!(!copy.is_null());
        // SAFETY: the snapshot transfers one exact dictionary owner.
        let copy = unsafe { Rc::from_raw(copy) };
        let key = Rc::new(Str::new("RELAR_RUNTIME_CONTROLLED_COPY_KEY"));
        let value = Rc::new(Str::new("controlled-copy-only"));
        let previous = std::env::var_os(key.as_str());
        // SAFETY: insertion acquires the two exact rooted strings into the owned
        // dictionary; it has no link back to the process's native environment.
        unsafe {
            relar_dict_set(
                Rc::as_ptr(&copy),
                Rc::as_ptr(&key) as usize,
                Rc::as_ptr(&value) as usize,
            )
        };
        // A boolean-only assertion avoids printing any inherited environment value.
        assert!(std::env::var_os(key.as_str()) == previous);
        assert_eq!(relar_error_pending(), 0);
    }
}
