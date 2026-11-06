//! Child status is shared independently of the thread that owns the Python
//! object, its pipe wrappers, and its argument list.
use super::*;
use num_bigint::BigInt;
use std::sync::Mutex;

pub struct ChildControl {
    pub(super) pid: libc::pid_t,
    state: Mutex<State>,
}

struct State {
    child: LaunchedChild,
    // Independent of the writable Python returncode. A stop still leaves us the child.
    reaped: bool,
    status: Option<ReturnCode>,
}

/// Kernel statuses fit a C int. Users may assign any Python integer to returncode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ReturnCode {
    /// Another waiter consumed the kernel status. Public poll/wait then report zero.
    NoChild,
    /// A finalizer could not obtain the status and recorded sys.maxsize.
    DeadState,
    Small(i32),
    Large(BigInt),
}

impl ReturnCode {
    pub(super) fn from_int(value: &Int) -> Self {
        value
            .as_small()
            .and_then(|n| i32::try_from(n).ok())
            .map_or_else(|| Self::Large(value.big().into_owned()), Self::Small)
    }

    pub(super) fn into_word(self) -> usize {
        match self {
            Self::NoChild => exit_word(0),
            Self::DeadState => Int::from_usize(isize::MAX as usize).into_raw(),
            Self::Small(value) => exit_word(value),
            Self::Large(value) => Int::new(value).into_raw(),
        }
    }

    pub(super) fn observed(self) -> Option<i32> {
        match self {
            Self::NoChild | Self::DeadState => None,
            code => Some(code.kernel()),
        }
    }

    /// Synchronous run/call own a fresh Popen that never escapes to user code.
    pub(super) fn kernel(self) -> i32 {
        match self {
            Self::NoChild | Self::DeadState => 0,
            Self::Small(code) => code,
            Self::Large(_) => unreachable!("kernel status fits in a C int"),
        }
    }
}

impl State {
    fn signal_if_running(&self, value: impl FnOnce() -> Result<i32>) -> Result<()> {
        if self.status.is_none() && !self.reaped {
            let value = value()?;
            // SAFETY: the locked state still owns this PID; kill checks the signal.
            if unsafe { libc::kill(self.child.pid(), value) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(errno_error(error));
                }
            }
        }
        Ok(())
    }

    /// Poll the kernel when the public cache is empty.
    fn refresh(&mut self) -> Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if self.reaped {
            self.status = Some(ReturnCode::NoChild);
            return Ok(());
        }
        let pid = self.child.pid();
        let mut status = 0;
        loop {
            // SAFETY: status is writable, and the mutex serializes all Popen reapers.
            match unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } {
                0 => return Ok(()),
                -1 => {
                    let error = io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::EINTR) => continue,
                        Some(libc::ECHILD) => {
                            self.reaped = true;
                            self.status = Some(ReturnCode::NoChild);
                        }
                        _ => return Err(errno_error(error)),
                    }
                }
                _ => {
                    self.reaped = libc::WIFEXITED(status) || libc::WIFSIGNALED(status);
                    self.status = Some(ReturnCode::Small(returncode(ExitStatus::from_raw(status))));
                }
            }
            return Ok(());
        }
    }
}

impl ChildControl {
    pub(super) fn new(child: LaunchedChild) -> Self {
        Self {
            pid: child.pid(),
            state: Mutex::new(State {
                child,
                reaped: false,
                status: None,
            }),
        }
    }

    pub(super) fn status(&self) -> Option<ReturnCode> {
        self.state.lock().unwrap().status.clone()
    }

    pub(super) fn set_status(&self, status: Option<ReturnCode>) {
        self.state.lock().unwrap().status = status;
    }

    pub(super) fn poll(&self) -> Result<Option<ReturnCode>> {
        let mut state = self.state.lock().unwrap();
        state.refresh()?;
        Ok(state.status.clone())
    }

    /// Transfer an observation to a boxed Python owner's own returncode cache.
    /// Another owner can still wait after a stop or an inconclusive error.
    pub(super) fn take_observation(&self) -> Result<Option<ReturnCode>> {
        let mut state = self.state.lock().unwrap();
        state.refresh()?;
        Ok(state.status.take())
    }

    /// Cleanup uses a distinct fallback only when it first observes a lost
    /// status. Preserve any returncode already reported by public wait or poll.
    pub(super) fn poll_for_cleanup(&self) -> Option<ReturnCode> {
        let mut state = self.state.lock().unwrap();
        if state.status.is_none()
            && (state.refresh().is_err() || matches!(state.status, Some(ReturnCode::NoChild)))
        {
            state.status = Some(ReturnCode::DeadState);
        }
        state.status.clone()
    }

    /// Hold the reap lock across the check and signal: another handle waiter
    /// cannot recycle this PID between observing it alive and sending to it.
    pub(super) fn signal(&self, value: impl FnOnce() -> Result<i32>) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        // send_signal uses public poll semantics. Only an observed status suppresses kill.
        let _ = state.refresh();
        state.signal_if_running(value)
    }

    /// A Python override has already run; check the cache and PID ownership under the lock.
    pub(super) fn signal_after_poll(&self, value: impl FnOnce() -> Result<i32>) -> Result<()> {
        self.state.lock().unwrap().signal_if_running(value)
    }

    pub(super) fn wait(&self, timeout: Option<&Deadline>) -> Result<Option<ReturnCode>> {
        self.wait_with(timeout, || Ok(()))
    }

    /// Run pending Python handlers outside the PID lock before retrying an interrupted wait.
    pub(super) fn wait_with<E: From<Error>>(
        &self,
        timeout: Option<&Deadline>,
        check: impl FnMut() -> std::result::Result<(), E>,
    ) -> std::result::Result<Option<ReturnCode>, E> {
        self.wait_polling(timeout, check, Self::poll)
    }

    pub(super) fn wait_polling<E: From<Error>>(
        &self,
        timeout: Option<&Deadline>,
        mut check: impl FnMut() -> std::result::Result<(), E>,
        poll: fn(&Self) -> Result<Option<ReturnCode>>,
    ) -> std::result::Result<Option<ReturnCode>, E> {
        check()?;
        if let Some(status) = poll(self)? {
            return Ok(Some(status));
        }
        if let Some(timeout) = timeout {
            let seconds = timeout.seconds()?;
            let start = Instant::now();
            let mut delay = Duration::from_millis(1);
            loop {
                let remaining = seconds - start.elapsed().as_secs_f64();
                if remaining <= 0.0 {
                    return Ok(None);
                }
                let sleep = delay.min(Duration::try_from_secs_f64(remaining).unwrap_or(delay));
                crate::objects::time::sleep_ns(sleep.as_nanos() as i64, &mut check)?;
                if let Some(status) = poll(self)? {
                    return Ok(Some(status));
                }
                delay = (delay * 2).min(Duration::from_millis(50));
            }
        }
        // Sleep without the lock and without consuming status. A signaling
        // thread can still acquire the lock, and two waiters cannot both reap.
        loop {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
            // SAFETY: waitid writes siginfo into the provided storage. WNOWAIT
            // leaves it to poll() to consume the status while holding the lock.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid as libc::id_t,
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EINTR) => {
                        check()?;
                        continue;
                    }
                    Some(libc::ECHILD) => {}
                    _ => return Err(errno_error(error).into()),
                }
            }
            if let Some(status) = poll(self)? {
                return Ok(Some(status));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn cleanup_of_an_externally_reaped_child_preserves_an_observed_status() {
        let reaped_child = || {
            let mut child = Command::new("/bin/true").spawn().unwrap();
            assert!(child.wait().unwrap().success());
            ChildControl::new(LaunchedChild::Process(child))
        };

        let abandoned = reaped_child();
        assert_eq!(abandoned.poll_for_cleanup(), Some(ReturnCode::DeadState));
        assert_eq!(abandoned.status(), Some(ReturnCode::DeadState));

        let observed = reaped_child();
        assert!(matches!(observed.poll(), Ok(Some(ReturnCode::NoChild))));
        assert_eq!(observed.poll_for_cleanup(), Some(ReturnCode::NoChild));
    }

    #[test]
    fn transferring_a_child_exit_releases_the_pid_once() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 37"])
            .spawn()
            .unwrap();
        let control = ChildControl::new(LaunchedChild::Process(child));
        let status =
            control.wait_polling(None, || Ok::<_, Error>(()), ChildControl::take_observation);
        assert_eq!(status.ok().flatten(), Some(ReturnCode::Small(37)));
        assert_eq!(control.status(), None);
        assert!(
            control
                .signal_after_poll(|| panic!("the PID was released"))
                .is_ok()
        );
        assert_eq!(
            control.take_observation().ok().flatten(),
            Some(ReturnCode::NoChild)
        );
    }

    #[test]
    fn concurrent_waiters_cache_the_signal_status() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 10"])
            .spawn()
            .unwrap();
        let control = ChildControl::new(LaunchedChild::Process(child));
        let ready = Barrier::new(3);
        std::thread::scope(|scope| {
            let readers: Vec<_> = [true, false]
                .into_iter()
                .map(|timed| {
                    let ready = &ready;
                    let control = &control;
                    scope.spawn(move || {
                        ready.wait();
                        let deadline = timed.then_some(Deadline::Float(4.0));
                        control.wait(deadline.as_ref()).ok().flatten()
                    })
                })
                .collect();
            ready.wait();
            assert!(control.signal(|| Ok(libc::SIGKILL)).is_ok());
            for reader in readers {
                assert_eq!(
                    reader.join().unwrap(),
                    Some(ReturnCode::Small(-libc::SIGKILL))
                );
            }
        });
        assert_eq!(
            control.wait(None).ok().flatten(),
            Some(ReturnCode::Small(-libc::SIGKILL))
        );
        assert_eq!(control.status(), Some(ReturnCode::Small(-libc::SIGKILL)));
        assert!(
            control
                .signal(|| panic!("a reaped child cannot receive another signal"))
                .is_ok()
        );
    }
}
