//! Synchronous child handles. Reading returncode does not poll; wait and poll
//! cache an observed status so a later signal cannot target a recycled PID.

use super::*;
use crate::StorageCell;
use crate::objects::files::{FileState, errno_error};
use std::cell::RefCell;

mod communicate;

type Pipes = [Option<Rc<Object>>; 3];

thread_local! {
    // Like subprocess._active: dropping the last handle neither kills a live
    // child nor blocks. A subsequent launch collects its status and pipes.
    static ACTIVE: RefCell<Vec<(Child, Pipes)>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn cleanup() {
    ACTIVE.with_borrow_mut(|children| {
        children.retain_mut(|(child, _)| !matches!(poll_child(child), Ok(Some(_))));
    });
}

fn poll_child(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    match child.try_wait() {
        Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
            Ok(Some(ExitStatus::from_raw(0)))
        }
        result => result,
    }
}

fn returncode(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0))
}

pub(in crate::objects) struct Popen {
    args: OwnedWord,
    child: Option<Child>,
    status: Option<i32>,
    pipes: Pipes,
    communication: communicate::Communication,
}

impl Popen {
    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("child stays owned until drop")
    }

    /// Close the parent's stream ends and reap even if a pipe flush fails.
    fn close_wait(&mut self) -> Result<()> {
        let mut failure = None;
        for index in [1, 2, 0] {
            if let Some(file) = &self.pipes[index]
                // SAFETY: the pipe is rooted and close cannot call language code.
                && let Err(err) = unsafe { file.0.with_mut(|s| s.file_mut().close()) }
            {
                failure = Some(errno_error(err));
            }
        }
        self.wait(None)?;
        failure.map_or(Ok(()), Err)
    }

    fn signal(&mut self, signal: impl FnOnce() -> Result<i32>) -> Result<()> {
        if self.poll()?.is_none() {
            let signal = signal()?;
            // SAFETY: our handle cannot be reaped on another language thread.
            if unsafe { libc::kill(self.child().id() as libc::pid_t, signal) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(os_error(error));
                }
            }
        }
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<i32>> {
        if self.status.is_none() {
            self.status = poll_child(self.child()).map_err(os_error)?.map(returncode);
        }
        Ok(self.status)
    }

    fn wait(&mut self, timeout: Option<&Deadline>) -> Result<Option<i32>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        if let Some(timeout) = timeout {
            let seconds = timeout.seconds()?;
            let start = Instant::now();
            let mut delay = Duration::from_millis(1);
            loop {
                if let Some(status) = self.poll()? {
                    return Ok(Some(status));
                }
                let remaining = seconds - start.elapsed().as_secs_f64();
                if remaining <= 0.0 {
                    return Ok(None);
                }
                std::thread::sleep(
                    delay.min(Duration::try_from_secs_f64(remaining).unwrap_or(delay)),
                );
                delay = (delay * 2).min(Duration::from_millis(50));
            }
        }
        let status = match self.child().wait() {
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => 0,
            status => returncode(status.map_err(os_error)?),
        };
        self.status = Some(status);
        Ok(self.status)
    }
}

impl Drop for Popen {
    fn drop(&mut self) {
        if self.status.is_none() && !matches!(self.poll(), Ok(Some(_))) {
            let child = self.child.take().expect("owned child");
            ACTIVE.with_borrow_mut(|children| {
                children.push((child, std::mem::take(&mut self.pipes)))
            });
        }
    }
}

/// # Safety
/// The handle must be a rooted Popen. The closure cannot call generated code
/// or let a reference to the storage escape.
unsafe fn with_process<T>(handle: *const Object, work: impl FnOnce(&mut Popen) -> T) -> T {
    // SAFETY: callers uphold the root and exclusive access through the closure.
    unsafe {
        object(handle).0.with_mut(|storage| match storage {
            Storage::Popen(process) => work(process),
            _ => fail("SystemError", "expected a Popen handle"),
        })
    }
}

/// Borrowed launch settings, filled by the compiler in parameter order after
/// evaluating all Python expressions. All referenced values are rooted by the caller.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LaunchConfig {
    args: *const (),
    pub(super) cwd: *const (),
    pub(super) env: *const Object,
    pub(super) executable: *const (),
    bufsize: usize,
    arg_kind: u32,
    pub(super) cwd_kind: u32,
    pub(super) executable_kind: u32,
    streams: u32,
    close_fds: bool,
    pub(super) shell: bool,
}

/// The existing ABI prefix is sufficient for fixed streams. Only callers
/// supplying a borrowed descriptor (6) or file (7) append the three words.
#[repr(C)]
struct RedirectConfig {
    launch: LaunchConfig,
    words: [usize; 3],
}

impl LaunchConfig {
    fn has_redirects(streams: u32) -> bool {
        [12, 0, 4]
            .iter()
            .any(|shift| matches!((streams >> shift) & 15, 6 | 7))
    }

    /// # Safety
    /// A configuration requesting mode 6 or 7 must be the first field of a
    /// RedirectConfig. Older callers use only fixed modes and need no trailer.
    unsafe fn redirect_words(config: *const Self) -> [usize; 3] {
        // SAFETY: the launch prefix is present for either ABI layout.
        let streams = unsafe { (*config).streams };
        if Self::has_redirects(streams) {
            // SAFETY: the mode proves that the caller provided the trailer.
            unsafe { (*config.cast::<RedirectConfig>()).words }
        } else {
            [0; 3]
        }
    }
    pub(super) fn basic(
        args: *const (),
        arg_kind: u32,
        cwd: *const Str,
        env: *const Object,
        streams: u32,
        bufsize: usize,
        close_fds: bool,
    ) -> Self {
        if Self::has_redirects(streams) {
            fail(
                "SystemError",
                "borrowed descriptors require a launch configuration trailer",
            );
        }
        Self {
            args,
            arg_kind,
            cwd: cwd.cast(),
            cwd_kind: 3,
            env,
            streams,
            bufsize,
            close_fds,
            executable: std::ptr::null(),
            executable_kind: 3,
            shell: false,
        }
    }
}

/// # Safety
/// Args, optional cwd, and optional environment are rooted through spawn.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    streams: u32,
) -> *const Object {
    // SAFETY: the legacy ABI uses the normal default buffer and closes inherited fds.
    unsafe { imre_popen_pipes(args, cwd, env, streams, (-1_isize as usize) << 1 | 1, true) }
}

/// # Safety
/// As imre_popen, with a rooted buffer size integer and statically selected stream modes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_pipes(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    streams: u32,
    bufsize: usize,
    close_fds: bool,
) -> *const Object {
    // SAFETY: the original entry point accepts only list[str].
    unsafe { imre_popen_command(args.cast(), 5, cwd, env, streams, bufsize, close_fds) }
}

/// # Safety
/// The command pointer is rooted with exactly the supplied representation kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_command(
    args: *const (),
    arg_kind: u32,
    cwd: *const Str,
    env: *const Object,
    streams: u32,
    bufsize: usize,
    close_fds: bool,
) -> *const Object {
    let config = LaunchConfig::basic(args, arg_kind, cwd, env, streams, bufsize, close_fds);
    // SAFETY: callers root every borrowed launch argument.
    unsafe { imre_popen_config(&config) }
}

/// # Safety
/// The settings and the owners of every argument must live through spawn.
/// A stream of mode 6 or 7 requires the RedirectConfig trailer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_config(config: *const LaunchConfig) -> *const Object {
    // SAFETY: the compiler supplies the exact settings layout and rooted owners.
    let result = unsafe { spawn(&*config, LaunchConfig::redirect_words(config)) }.map(|child| {
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Popen(Box::new(
            child,
        ))))))
    });
    finish(result, std::ptr::null())
}

/// Create a native child with independently owned pipe ends and the original argv owner.
///
/// # Safety
/// The caller roots the argv, optional directory/environment and buffer size.
unsafe fn spawn(config: &LaunchConfig, words: [usize; 3]) -> Result<Popen> {
    // SAFETY: settings describe the rooted command, path and integer representations.
    let args = unsafe { WordKind::new(config.arg_kind).acquire(config.args as usize) };
    let streams = config.streams | (u32::from(!config.close_fds) << 17);
    let modes = [12, 0, 4].map(|shift| (streams >> shift) & 15);
    if modes[1] == 5 {
        return Err(Error::new(
            Kind::ValueError,
            "STDOUT can only be used for stderr",
        ));
    }
    let c_int = |n: &Int| n.as_small().and_then(|n| i32::try_from(n).ok());
    let mut descriptors = [-1; 3];
    let mut oversized = false;
    for (i, mode) in modes.into_iter().enumerate() {
        descriptors[i] = match mode {
            6 => {
                // SAFETY: mode 6 denotes the caller's rooted integer descriptor.
                let fd = unsafe { Int::borrow(words[i]) };
                c_int(&fd).unwrap_or_else(|| {
                    oversized = true;
                    // Preserve the launch-mode comparison to fd 2 without truncation.
                    if fd.big().sign() == num_bigint::Sign::Minus {
                        i32::MIN
                    } else {
                        i32::MAX
                    }
                })
            }
            // SAFETY: mode 7 denotes a rooted native file; only its fd escapes the borrow.
            7 => unsafe {
                object(words[i] as *const Object)
                    .0
                    .with(|s| s.file().fileno())
            }?,
            _ => -1,
        };
    }
    // Python opens the parent-side pipe wrappers after it collects the file handles.
    let bufsize = if modes.contains(&1) {
        // SAFETY: the caller roots the buffer-size integer.
        let size = unsafe { Int::borrow(config.bufsize) };
        c_int(&size).ok_or_else(c_int_overflow)?
    } else {
        -1
    };
    // SAFETY: the compiler supplies the same exact owners as subprocess.run.
    let child = unsafe { launch_error::spawn(&args, config, streams, descriptors, oversized) };
    child.map(|mut child| {
        let text = streams & (1 << 16) != 0;
        let pipe = |fd: OwnedFd, read| FileState::pipe(fd.into(), read, text, bufsize);
        let pipes = [
            child.stdin.take().map(|fd| pipe(fd.into(), false)),
            child.stdout.take().map(|fd| pipe(fd.into(), true)),
            child.stderr.take().map(|fd| pipe(fd.into(), true)),
        ];
        Popen {
            pipes,
            args,
            child: Some(child),
            status: None,
            communication: Default::default(),
        }
    })
}

/// Wait without draining pipes, as call() does. A failed or expired wait
/// kills the child and context exit closes its pipes and collects the status.
///
/// # Safety
/// Launch arguments and the optional deadline word remain rooted until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_call(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    streams: u32,
    bufsize: usize,
    close_fds: bool,
    kind: u32,
    word: usize,
    check: bool,
) -> usize {
    // SAFETY: the original call entry point accepts an exact list[str].
    unsafe {
        imre_call_command(
            args.cast(),
            5,
            cwd,
            env,
            streams,
            bufsize,
            close_fds,
            kind,
            word,
            check,
        )
    }
}

/// # Safety
/// The command pointer has the given representation; all launch and deadline arguments are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_call_command(
    args: *const (),
    arg_kind: u32,
    cwd: *const Str,
    env: *const Object,
    streams: u32,
    bufsize: usize,
    close_fds: bool,
    kind: u32,
    word: usize,
    check: bool,
) -> usize {
    let config = LaunchConfig::basic(args, arg_kind, cwd, env, streams, bufsize, close_fds);
    // SAFETY: the legacy settings and deadline have the supplied representations.
    unsafe { imre_call_config(&config, kind, word, check) }
}

/// # Safety
/// The rooted settings (including any RedirectConfig trailer) and deadline word
/// must match the compiler's layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_call_config(
    config: *const LaunchConfig,
    kind: u32,
    word: usize,
    check: bool,
) -> usize {
    // SAFETY: the settings and the values to which they point are rooted.
    let result =
        unsafe { spawn(&*config, LaunchConfig::redirect_words(config)) }.and_then(|mut p| {
            // SAFETY: the deadline kind was selected for this rooted word by the compiler.
            let timeout = unsafe { Deadline::from_word(kind, word) };
            let waited = p.wait(timeout.as_ref());
            let killed = if matches!(waited, Ok(Some(_))) {
                Ok(())
            } else {
                p.signal(|| Ok(libc::SIGKILL))
            };
            p.close_wait()?;
            killed?;
            match waited? {
                Some(code) if check && code != 0 => {
                    let absent = || OwnedWord {
                        word: 0,
                        kind: WordKind::new(0),
                    };
                    raise_process(ProcessError {
                        cmd: p.args.clone(),
                        returncode: code,
                        stdout: absent(),
                        stderr: absent(),
                    });
                    Ok(0)
                }
                Some(code) => Ok(exit_word(code)),
                None => {
                    raise_timeout(TimeoutError {
                        cmd: p.args.clone(),
                        timeout: timeout.expect("timed wait"),
                        stdout: None,
                        stderr: None,
                    });
                    Ok(0)
                }
            }
        });
    finish(result, 0)
}

/// # Safety
/// The handle is rooted. A present pipe returns an independent reference to
/// the same file, including buffered data and closed state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_pipe(handle: *const Object, index: u32) -> *const Object {
    // SAFETY: the statically captured pipe belongs to this handle.
    unsafe {
        with_process(handle, |p| {
            p.pipes[index as usize]
                .as_ref()
                .map_or(std::ptr::null(), |file| Rc::into_raw(file.clone()))
        })
    }
}

/// # Safety
/// The caller roots this exact Popen until the PID is read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_pid(handle: *const Object) -> usize {
    // SAFETY: no language code runs while reading the child ID.
    unsafe { with_process(handle, |p| exit_word(p.child().id() as i32)) }
}

/// # Safety
/// The caller roots this Popen; the returned reference owns its original argv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_args(handle: *const Object) -> *const () {
    // SAFETY: the handle owns this exact command throughout the reference acquisition.
    unsafe { with_process(handle, |p| p.args.clone().into_word() as *const ()) }
}

/// # Safety
/// The caller roots this Popen; zero denotes an unobserved status.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_status(handle: *const Object, poll: bool) -> usize {
    // SAFETY: polling is an OS operation with no language callbacks.
    let result = unsafe { with_process(handle, |p| if poll { p.poll() } else { Ok(p.status) }) };
    finish(result.map(|s| s.map_or(0, exit_word)), 0)
}

/// # Safety
/// The handle and deadline word stay rooted. Timeout leaves the child alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_wait(handle: *const Object, kind: u32, word: usize) -> usize {
    // SAFETY: exact argument owners and deadline kind are fixed by the call site.
    unsafe {
        let timeout = Deadline::from_word(kind, word);
        with_process(handle, |p| match p.wait(timeout.as_ref()) {
            Ok(Some(code)) => exit_word(code),
            Ok(None) => {
                raise_timeout(TimeoutError {
                    cmd: p.args.clone(),
                    timeout: timeout.expect("timed wait"),
                    stdout: None,
                    stderr: None,
                });
                0
            }
            Err(error) => error.returning(0),
        })
    }
}

/// # Safety
/// Both the Popen and integer signal remain rooted until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_signal(handle: *const Object, signal: usize) {
    // SAFETY: poll reaps only this child. The single language thread cannot
    // concurrently reap it between poll and kill; ESRCH means it already exited.
    let result = unsafe {
        with_process(handle, |p| {
            p.signal(|| {
                Int::borrow(signal)
                    .as_small()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| {
                        Error::new(
                            Kind::OverflowError,
                            "Python int too large to convert to C int",
                        )
                    })
            })
        })
    };
    finish(result, ());
}

/// # Safety
/// The caller roots the entered Popen through descriptor close and wait.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_exit(handle: *const Object) {
    // SAFETY: closing the native pipe ends and reaping cannot invoke language code.
    let result = unsafe { with_process(handle, |p| p.close_wait()) };
    finish(result, ());
}
