//! Synchronous child handles. Reading returncode does not poll; wait and poll
//! cache an observed status so a later signal cannot target a recycled PID.

use super::*;
use crate::StorageCell;
use std::cell::RefCell;

thread_local! {
    // Like subprocess._active: dropping the last handle neither kills a live
    // child nor blocks. A subsequent launch collects its status and pipes.
    static ACTIVE: RefCell<Vec<Child>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn cleanup() {
    ACTIVE.with_borrow_mut(|children| {
        children.retain_mut(|child| !matches!(poll_child(child), Ok(Some(_))));
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
    args: Rc<Object>,
    child: Option<Child>,
    status: Option<i32>,
}

impl Popen {
    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("child stays owned until drop")
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
            ACTIVE.with_borrow_mut(|children| children.push(child));
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

/// # Safety
/// Args, optional cwd, and optional environment are rooted through spawn.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen(
    args: *const Object,
    cwd: *const Str,
    env: *const Object,
    streams: u32,
) -> *const Object {
    if streams & 15 == 5 {
        return Error::new(Kind::ValueError, "STDOUT can only be used for stderr")
            .returning(std::ptr::null());
    }
    // SAFETY: the compiler supplies the same exact owners as subprocess.run.
    let result = unsafe { prepare_process(args, cwd, env) }
        .and_then(|(launch, cwd)| launch.spawn(cwd.as_ref(), streams).map_err(os_error))
        .map(|child| {
            let process = Popen {
                // SAFETY: the original argv stays rooted while we acquire it.
                args: unsafe { acquire_rc(args as usize) },
                child: Some(child),
                status: None,
            };
            Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Popen(Box::new(
                process,
            ))))))
        });
    finish(result, std::ptr::null())
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
pub unsafe extern "C" fn imre_popen_args(handle: *const Object) -> *const Object {
    // SAFETY: the handle owns the list throughout the reference acquisition.
    unsafe { with_process(handle, |p| Rc::into_raw(p.args.clone())) }
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
            if p.poll()?.is_some() {
                return Ok(());
            }
            let signal = Int::borrow(signal)
                .as_small()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new(
                        Kind::OverflowError,
                        "Python int too large to convert to C int",
                    )
                })?;
            if libc::kill(p.child().id() as libc::pid_t, signal) < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(os_error(error));
                }
            }
            Ok(())
        })
    };
    finish(result, ());
}

/// # Safety
/// The caller roots the entered Popen through descriptor close and wait.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_exit(handle: *const Object) {
    // SAFETY: closing the native pipe ends and reaping cannot invoke language code.
    let result = unsafe {
        with_process(handle, |p| {
            let child = p.child();
            drop(child.stdout.take());
            drop(child.stderr.take());
            drop(child.stdin.take());
            p.wait(None).map(|_| ())
        })
    };
    finish(result, ());
}
