//! Synchronous child handles. Reading returncode does not poll; wait and poll
//! cache an observed status so a later signal cannot target a recycled PID.

use super::*;
use crate::StorageCell;
use crate::objects::files::{FileState, errno_error};
use std::cell::RefCell;

mod active;
mod communicate;
mod control;
mod poll;
mod selector;

pub(super) use active::cleanup;
pub(in crate::objects) use active::retain_live_owner;
use control::{ChildControl, ReturnCode};
use std::sync::Arc;

type Pipes = [Option<crate::ObjectOwner>; 3];

#[derive(Default)]
pub(super) struct ChildPipes {
    pub(super) ends: [Option<Stdio>; 3],
    pub(super) closed_descriptor: bool,
    pub(super) devnull: Option<fs::File>,
}

impl ChildPipes {
    /// Open all pipe handles before allocating their Python stream buffers.
    fn open(
        modes: [u32; 3],
        descriptors: [RawFd; 3],
        pipe_capacity: Option<u32>,
    ) -> Result<(Self, [Option<OwnedFd>; 3])> {
        let mut child = Self::default();
        let mut parents = [None, None, None];
        if modes.iter().any(|mode| matches!(mode, 1 | 2)) {
            // Remember closed explicit descriptors before a new pipe can reuse their number.
            child.closed_descriptor = descriptors.into_iter().any(|fd| {
                // SAFETY: F_GETFD checks the integer without taking ownership.
                fd >= 0 && unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0
            });
        }
        for (i, mode) in modes.into_iter().enumerate() {
            if mode == 1 {
                let (read, write) = super::pipe_size::open(pipe_capacity).map_err(errno_error)?;
                let (read, write): (OwnedFd, OwnedFd) = (read.into(), write.into());
                let (parent, other) = if i == 0 { (write, read) } else { (read, write) };
                parents[i] = Some(parent);
                child.ends[i] = Some(Stdio::from(other));
            } else if mode == 2 && child.devnull.is_none() {
                child.devnull = Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open("/dev/null")
                        .map_err(errno_error)?,
                );
            }
        }
        Ok((child, parents))
    }
}

/// Explicit descriptors and any handles already opened by Popen's constructor.
pub(super) struct Redirects {
    pub(super) fds: [RawFd; 3],
    pub(super) oversized: bool,
    pub(super) prepared: Option<ChildPipes>,
}

impl Default for Redirects {
    fn default() -> Self {
        Self {
            fds: [-1; 3],
            oversized: false,
            prepared: None,
        }
    }
}

pub(in crate::objects) struct Popen {
    args: OwnedWord,
    poll_override: Option<poll::Override>,
    control: Arc<ChildControl>,
    pipes: Pipes,
    communication: communicate::Communication,
    finalized: bool,
}

impl Popen {
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

    fn signal(&self, signal: impl FnOnce() -> Result<i32>) -> Result<()> {
        self.control.signal(signal)
    }

    fn wait(&self, timeout: Option<&Deadline>) -> Result<Option<ReturnCode>> {
        self.control.wait(timeout)
    }
}

impl Popen {
    /// Python calls __del__ only once, including when _active keeps self alive.
    fn finalize(&mut self) -> bool {
        if self.finalized {
            return false;
        }
        self.finalized = true;
        // The public warning is about an unobserved returncode. Even a child
        // which has already exited can still be waiting for its parent to reap it.
        if self.control.status().is_none() {
            crate::objects::warnings::unobserved_child(self.control.pid);
        }
        !matches!(self.control.poll(), Ok(Some(_)))
    }
}

impl Drop for Popen {
    fn drop(&mut self) {
        if self.finalize() {
            // Internal run/call temporaries need not have a language-level owner.
            active::defer_internal(self.control.clone(), std::mem::take(&mut self.pipes));
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

/// Callers setting bit 18 supply this extension of both earlier layouts.
#[repr(C)]
struct ChildConfig {
    redirect: RedirectConfig,
    group: usize,
    mask: usize,
    new_session: bool,
}

/// Bit 19 adds the exact pass_fds collection after the bit-18 prefix.
#[repr(C)]
struct PassFdsConfig {
    child: ChildConfig,
    keep: usize,
    kind: u32,
}

/// Bit 21 appends the original pipesize word and its exact representation.
#[repr(C)]
struct SizedPipeConfig {
    pass: PassFdsConfig,
    size: usize,
    kind: u32,
}

#[derive(Default)]
struct LaunchExtras {
    redirects: [usize; 3],
    child: Option<(usize, usize, bool)>,
    keep: Option<(usize, u32)>,
    pipe_size: Option<(usize, u32)>,
}

impl LaunchConfig {
    fn has_redirects(streams: u32) -> bool {
        [12, 0, 4]
            .iter()
            .any(|shift| matches!((streams >> shift) & 15, 6 | 7))
    }

    /// # Safety
    /// Bit 18 selects ChildConfig; stream modes 6/7 select RedirectConfig.
    /// Older callers using fixed streams supply only the original prefix.
    unsafe fn extras(config: *const Self) -> LaunchExtras {
        // SAFETY: the launch prefix is present for all three ABI layouts.
        let streams = unsafe { (*config).streams };
        if streams & (1 << 18) != 0 {
            let pipe_size = (streams & (1 << 21) != 0).then(|| {
                // SAFETY: bit 21 promises the complete sized pipe trailer.
                let full = unsafe { &*config.cast::<SizedPipeConfig>() };
                (full.size, full.kind)
            });
            let keep = (streams & (1 << 19) != 0).then(|| {
                // SAFETY: bit 19 promises the complete pass_fds trailer.
                let full = unsafe { &*config.cast::<PassFdsConfig>() };
                (full.keep, full.kind)
            });
            // SAFETY: bit 18 promises the full child settings layout.
            let config = unsafe { &*config.cast::<ChildConfig>() };
            LaunchExtras {
                redirects: config.redirect.words,
                child: Some((config.group, config.mask, config.new_session)),
                keep,
                pipe_size,
            }
        } else if Self::has_redirects(streams) {
            LaunchExtras {
                // SAFETY: a borrowed stream promises the three-word extension.
                redirects: unsafe { (*config.cast::<RedirectConfig>()).words },
                child: None,
                keep: None,
                pipe_size: None,
            }
        } else {
            LaunchExtras::default()
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
        if Self::has_redirects(streams) || streams & (1 << 18) != 0 {
            fail(
                "SystemError",
                "these child settings require a launch configuration trailer",
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
/// A borrowed stream requires RedirectConfig; bit 18 requires ChildConfig.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_config(config: *const LaunchConfig) -> *const Object {
    // SAFETY: the compiler supplies the exact settings layout and rooted owners.
    let result = unsafe { spawn(&*config, LaunchConfig::extras(config)) }.map(|child| {
        crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
            Storage::Popen(Box::new(child)),
        ))))
    });
    finish(result, std::ptr::null())
}

/// Check only on a validation failure: a successful launch creates its own handles.
fn check_handles(
    modes: &[u32],
    fork_error_pipe: bool,
    pipe_capacity: Option<u32>,
) -> io::Result<()> {
    let mut pipes = [const { None }; 3];
    let mut devnull = None;
    for (pipe, mode) in pipes.iter_mut().zip(modes) {
        match mode {
            1 => *pipe = Some(super::pipe_size::open(pipe_capacity)?),
            2 if devnull.is_none() => {
                devnull = Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open("/dev/null")?,
                );
            }
            _ => {}
        }
    }
    if fork_error_pipe {
        let (_read, write) = io::pipe()?;
        // The fork child needs its error writer above the standard descriptors.
        if write.as_raw_fd() < 3 {
            let _above_stdio = write.try_clone()?;
        }
    }
    Ok(())
}

pub(super) fn error_after_handles(modes: &[u32], fork_error_pipe: bool, error: Error) -> Error {
    check_handles(modes, fork_error_pipe, None)
        .err()
        .map_or(error, errno_error)
}

/// Create a native child with independently owned pipe ends and the original argv owner.
///
/// # Safety
/// The caller roots the argv, optional directory/environment and buffer size.
unsafe fn spawn(config: &LaunchConfig, extras: LaunchExtras) -> Result<Popen> {
    // CPython runs _cleanup before validating the new Popen's arguments.
    cleanup();
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
    let pipe_capacity = match extras.pipe_size {
        Some((word, kind)) => {
            // SAFETY: the launch argument stays rooted until spawn returns.
            unsafe { super::pipe_size::capacity(word, WordKind::new(kind)) }?
        }
        None => None,
    };
    if !config.close_fds
        && let Some((word, kind)) = extras.keep
        // SAFETY: the launch trailer retains the original collection.
        && unsafe { super::pass_fds::nonempty(word, kind) }
    {
        crate::objects::warnings::pass_fds_override()?;
    }
    let c_int = |n: &Int| n.as_small().and_then(|n| i32::try_from(n).ok());
    let mut descriptors = [-1; 3];
    let mut oversized = false;
    for (i, mode) in modes.into_iter().enumerate() {
        descriptors[i] = match mode {
            6 => {
                // SAFETY: mode 6 denotes the caller's rooted integer descriptor.
                let fd = unsafe { Int::borrow(extras.redirects[i]) };
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
                object(extras.redirects[i] as *const Object)
                    .0
                    .with(|s| s.file().fileno())
            }
            .map_err(|error| {
                check_handles(&modes[..i], false, pipe_capacity)
                    .err()
                    .map_or(error, errno_error)
            })?,
            _ => -1,
        };
    }
    let (prepared, parents) = ChildPipes::open(modes, descriptors, pipe_capacity)?;
    // Python opens the parent-side pipe wrappers after it collects the file handles.
    let bufsize = if modes.contains(&1) {
        // SAFETY: the caller roots the buffer-size integer.
        let size = unsafe { Int::borrow(config.bufsize) };
        match c_int(&size) {
            Some(size) => size,
            None => return Err(c_int_overflow()),
        }
    } else {
        -1
    };
    let child_args = extras.child.map(|(group, mask, new_session)| {
        let group = (group != 0).then(|| {
            // SAFETY: a non-null group is a rooted Python integer.
            unsafe { Int::borrow(group) }
        });
        // SAFETY: the launch trailer roots the mask through argument conversion.
        (group, unsafe { Int::borrow(mask) }, new_session)
    });
    let allow_spawn = child_args
        .as_ref()
        .is_none_or(|(group, mask, new_session)| {
            !new_session
                && group.as_ref().is_none_or(|n| n.as_small() == Some(-1))
                && mask.big().sign() == num_bigint::Sign::Minus
        })
        && extras.keep.is_none_or(|(word, kind)| {
            // SAFETY: the bit-19 trailer supplies the collection's exact kind.
            !unsafe { super::pass_fds::nonempty(word, kind) }
        });
    let child = |direct| {
        let Some((group, mask, new_session)) = child_args else {
            return Ok(ChildAttributes::default());
        };
        if direct {
            return Ok(ChildAttributes::default());
        }
        // Fork/exec iterates pass_fds before C narrows the group and mask.
        if extras.keep.is_some_and(|(_, kind)| kind == 0) {
            return Err(Error::new(
                Kind::TypeError,
                "'NoneType' object is not iterable",
            ));
        }
        let group = group
            .as_ref()
            .map_or(Ok(-1), |n| c_int(n).ok_or_else(c_int_overflow))?;
        let mask = c_int(&mask).ok_or_else(c_int_overflow)?;
        let keep = match extras.keep {
            // SAFETY: the bit-19 trailer retains the collection and its exact kind.
            Some((word, kind)) => unsafe { super::pass_fds::copy(word, kind)? },
            None => Vec::new(),
        };
        Ok(ChildAttributes {
            new_session,
            group,
            mask: (mask >= 0).then_some(mask as libc::mode_t),
            keep,
        })
    };
    let text = streams & (1 << 16) != 0;
    let mut pipes: Pipes = [None, None, None];
    for (i, parent) in parents.into_iter().enumerate() {
        if let Some(fd) = parent {
            pipes[i] = Some(FileState::pipe(fd.into(), i != 0, text, bufsize)?);
        }
    }
    let redirects = Redirects {
        fds: descriptors,
        oversized,
        prepared: Some(prepared),
    };
    // SAFETY: the compiler supplies the same exact owners as subprocess.run.
    let child =
        unsafe { launch_error::spawn(&args, config, streams, redirects, allow_spawn, child) }?;
    Ok(Popen {
        pipes,
        args,
        control: Arc::new(ChildControl::new(child)),
        communication: Default::default(),
        finalized: false,
        poll_override: None,
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
/// The rooted settings (including any redirect or child trailer) and deadline word
/// must match the compiler's layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_call_config(
    config: *const LaunchConfig,
    kind: u32,
    word: usize,
    check: bool,
) -> usize {
    // SAFETY: the settings and the values to which they point are rooted.
    let result = unsafe { spawn(&*config, LaunchConfig::extras(config)) }.and_then(|mut p| {
        // SAFETY: the deadline kind was selected for this rooted word by the compiler.
        let timeout = unsafe { Deadline::from_word(kind, word) };
        let waited = p
            .wait(timeout.as_ref())
            .map(|code| code.map(ReturnCode::kernel));
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
                    positional_streams: false,
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
                    wait_budget: None,
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
                .map_or(std::ptr::null(), |file| {
                    crate::ObjectOwner::into_raw(file.clone())
                })
        })
    }
}

/// # Safety
/// The caller roots this exact Popen until the PID is read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_pid(handle: *const Object) -> usize {
    // SAFETY: no language code runs while reading the child ID.
    unsafe { with_process(handle, |p| exit_word(p.control.pid)) }
}

/// # Safety
/// The caller roots this Popen; the returned reference owns its original argv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_args(handle: *const Object) -> *const () {
    // SAFETY: the handle owns this exact command throughout the reference acquisition.
    unsafe { with_process(handle, |p| p.args.clone().into_word() as *const ()) }
}

/// Borrow the separate, thread-safe child control. The Python handle and its
/// argument/pipe owners are accessed only by their owning language thread.
///
/// # Safety
/// Call on the Popen owner thread. Keep that Popen rooted until every borrowed
/// control operation has finished, including operations on other threads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_control(handle: *const Object) -> *const ChildControl {
    // SAFETY: the control has its own stable allocation for the lifetime of the handle.
    unsafe { with_process(handle, |p| Arc::as_ptr(&p.control)) }
}

/// # Safety
/// The caller roots the Popen that owns control. Zero denotes an unobserved status.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_child_status(control: *const ChildControl, poll: bool) -> usize {
    // SAFETY: only synchronized child state is accessed, on any caller thread.
    let control = unsafe { &*control };
    let result = if poll {
        control.poll()
    } else {
        Ok(control.status())
    };
    finish(result.map(|s| s.map_or(0, ReturnCode::into_word)), 0)
}

/// # Safety
/// The caller roots the Popen's control and the optional Python integer through the store.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_child_set_status(control: *const ChildControl, word: usize) {
    let value = (word != 0).then(|| {
        // SAFETY: a nonzero word is a rooted integer on the calling thread.
        let value = unsafe { Int::borrow(word) };
        ReturnCode::from_int(&value)
    });
    // SAFETY: the Popen continues to own this control while its mutex is held.
    unsafe { (*control).set_status(value) };
}

/// # Safety
/// The control must stay rooted. A non-None timeout must be rooted on the owning
/// thread and supplied with its original Popen for constructing TimeoutExpired.
/// An unbounded wait may run on another thread with a null owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_child_wait(
    control: *const ChildControl,
    kind: u32,
    word: usize,
    owner: *const Object,
) -> usize {
    // SAFETY: the timeout and control have the lifetimes described above. The
    // language object is touched only on an actual timeout, after releasing the lock.
    unsafe {
        let timeout = Deadline::from_word(kind, word);
        match (*control).wait(timeout.as_ref()) {
            Ok(Some(code)) => code.into_word(),
            Ok(None) => {
                let cmd = with_process(owner, |p| p.args.clone());
                raise_timeout(TimeoutError {
                    cmd,
                    timeout: timeout.expect("timed wait"),
                    wait_budget: None,
                    stdout: None,
                    stderr: None,
                });
                0
            }
            Err(error) => error.returning(0),
        }
    }
}

/// # Safety
/// The caller roots the integer until conversion; conversion can raise OverflowError.
unsafe fn signal_number(signal: usize) -> Result<i32> {
    // SAFETY: the caller supplied a rooted Python integer.
    unsafe { Int::borrow(signal) }
        .as_small()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C int",
            )
        })
}

/// # Safety
/// The control stays rooted, and the signal integer belongs to the caller thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_child_signal(control: *const ChildControl, signal: usize) {
    // SAFETY: no language state from the Popen owner is accessed while signaling.
    let result = unsafe { (*control).signal(|| signal_number(signal)) };
    finish(result, ());
}

/// # Safety
/// Legacy entry point: the caller roots this Popen on its owner thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_status(handle: *const Object, poll: bool) -> usize {
    // SAFETY: clone the callable under the borrow, then invoke it after releasing the borrow.
    unsafe {
        if poll && let Some(callback) = with_process(handle, |p| p.poll_override.clone()) {
            return callback.invoke();
        }
        imre_child_status(imre_popen_control(handle), poll)
    }
}

/// # Safety
/// Legacy entry point: the handle and timeout word remain rooted on this thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_wait(handle: *const Object, kind: u32, word: usize) -> usize {
    // SAFETY: the same owner constructs TimeoutExpired if its deadline elapses.
    unsafe { imre_child_wait(imre_popen_control(handle), kind, word, handle) }
}

/// # Safety
/// Legacy entry point: the handle and signal integer stay rooted on this thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_signal(handle: *const Object, signal: usize) {
    // SAFETY: the handle and signal stay rooted across the optional user callback.
    unsafe {
        if !poll::imre_popen_signal_override(handle, signal) {
            imre_child_signal(imre_popen_control(handle), signal);
        }
    }
}

/// # Safety
/// The caller roots the entered Popen through descriptor close and wait.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_exit(handle: *const Object) {
    // SAFETY: closing the native pipe ends and reaping cannot invoke language code.
    let result = unsafe { with_process(handle, |p| p.close_wait()) };
    finish(result, ());
}
