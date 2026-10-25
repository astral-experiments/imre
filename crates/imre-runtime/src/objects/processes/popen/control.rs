//! Child status is shared independently of the thread that owns the Python
//! object, its pipe wrappers, and its argument list.
use super::*;
use std::sync::Mutex;

pub struct ChildControl {
    pub(super) pid: libc::pid_t,
    state: Mutex<State>,
}

struct State {
    child: Child,
    status: Option<i32>,
}

impl State {
    /// Only this lock holder can reap the child or change its cached returncode.
    fn poll(&mut self) -> Result<Option<i32>> {
        if self.status.is_none() {
            self.status = match self.child.try_wait() {
                Ok(status) => status.map(returncode),
                Err(e) if e.raw_os_error() == Some(libc::ECHILD) => Some(0),
                Err(e) => return Err(os_error(e)),
            };
        }
        Ok(self.status)
    }
}

impl ChildControl {
    pub(super) fn new(child: Child) -> Self {
        Self {
            pid: child.id() as libc::pid_t,
            state: Mutex::new(State {
                child,
                status: None,
            }),
        }
    }

    pub(super) fn status(&self) -> Option<i32> {
        self.state.lock().unwrap().status
    }

    pub(super) fn poll(&self) -> Result<Option<i32>> {
        self.state.lock().unwrap().poll()
    }

    /// Hold the reap lock across the check and signal: another handle waiter
    /// cannot recycle this PID between observing it alive and sending to it.
    pub(super) fn signal(&self, value: impl FnOnce() -> Result<i32>) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.poll()?.is_none() {
            let value = value()?;
            // SAFETY: this PID is the child owned by state; kill checks the signal.
            if unsafe { libc::kill(self.pid, value) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(os_error(error));
                }
            }
        }
        Ok(())
    }

    pub(super) fn wait(&self, timeout: Option<&Deadline>) -> Result<Option<i32>> {
        if let Some(status) = self.poll()? {
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
                std::thread::sleep(
                    delay.min(Duration::try_from_secs_f64(remaining).unwrap_or(delay)),
                );
                if let Some(status) = self.poll()? {
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
                    Some(libc::EINTR) => continue,
                    Some(libc::ECHILD) => {}
                    _ => return Err(os_error(error)),
                }
            }
            if let Some(status) = self.poll()? {
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
    fn concurrent_waiters_cache_the_signal_status() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 10"])
            .spawn()
            .unwrap();
        let control = ChildControl::new(child);
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
                assert_eq!(reader.join().unwrap(), Some(-libc::SIGKILL));
            }
        });
        assert_eq!(control.wait(None).ok().flatten(), Some(-libc::SIGKILL));
        assert_eq!(control.status(), Some(-libc::SIGKILL));
        assert!(
            control
                .signal(|| panic!("a reaped child cannot receive another signal"))
                .is_ok()
        );
    }
}
