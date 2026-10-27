//! Primitive Python locks can be released by any thread.
use super::exceptions::Result;
use super::{Error, Int, Kind};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct PrimitiveLock {
    held: Mutex<bool>,
    available: Condvar,
}

impl PrimitiveLock {
    pub(crate) fn locked(&self) -> bool {
        *self.held.lock().unwrap()
    }

    /// Wait for an unlocked state and claim it under the same mutex.
    pub(crate) fn acquire(&self, blocking: bool, timeout: Option<Duration>) -> bool {
        let mut held = self.held.lock().unwrap();
        if *held && blocking {
            held = match timeout {
                Some(limit) => {
                    self.available
                        .wait_timeout_while(held, limit, |v| *v)
                        .unwrap()
                        .0
                }
                None => self.available.wait_while(held, |v| *v).unwrap(),
            };
        }
        if *held {
            return false;
        }
        *held = true;
        true
    }

    /// The thread releasing a primitive lock need not be the acquiring thread.
    pub(crate) fn release(&self) -> Result<()> {
        let mut held = self.held.lock().unwrap();
        if !*held {
            return Err(Error::new(Kind::RuntimeError, "release unlocked lock"));
        }
        *held = false;
        self.available.notify_one();
        Ok(())
    }
}

/// Validate the requested wait before inspecting the lock, including nonblocking calls.
fn wait_duration(blocking: bool, ns: i64) -> Result<Option<Duration>> {
    if ns == -1_000_000_000 {
        return Ok(None);
    }
    if !blocking {
        return Err(Error::new(
            Kind::ValueError,
            "can't specify a timeout for a non-blocking call",
        ));
    }
    if ns < 0 {
        return Err(Error::new(
            Kind::ValueError,
            "timeout value must be a non-negative number",
        ));
    }
    // CPython rounds up to microseconds. Windows reserves 0xffffffff milliseconds for INFINITE.
    let limit = if cfg!(windows) {
        0xffff_fffe * 1_000_000
    } else {
        i64::MAX / 1_000 * 1_000
    };
    if ns > limit {
        return Err(Error::new(
            Kind::OverflowError,
            "timeout value is too large",
        ));
    }
    Ok(Some(Duration::from_nanos(ns as u64)))
}

/// CPython rounds fractional timeouts away from zero before converting to PyTime_t.
pub(crate) fn timeout_float(blocking: bool, seconds: f64) -> Result<Option<Duration>> {
    if seconds.is_nan() {
        return Err(Error::new(
            Kind::ValueError,
            "Invalid value NaN (not a number)",
        ));
    }
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    let ns = seconds * 1e9;
    let ns = if ns < 0.0 { ns.floor() } else { ns.ceil() };
    if !(-LIMIT..LIMIT).contains(&ns) {
        return Err(Error::new(
            Kind::OverflowError,
            "timestamp out of range for platform time_t",
        ));
    }
    wait_duration(blocking, ns as i64)
}

/// Convert integer seconds without losing precision before validating the timeout.
pub(crate) fn timeout_int(blocking: bool, seconds: &Int) -> Result<Option<Duration>> {
    let ns = seconds
        .as_small()
        .and_then(|s| s.checked_mul(1_000_000_000));
    ns.ok_or_else(|| {
        Error::new(
            Kind::OverflowError,
            "timestamp too large to convert to C PyTime_t",
        )
    })
    .and_then(|ns| wait_duration(blocking, ns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Instant;

    #[test]
    fn another_thread_can_release_after_a_failed_acquire() {
        let lock = Arc::new(PrimitiveLock::default());
        assert!(lock.acquire(true, None));
        assert!(!lock.acquire(false, None));
        let other = Arc::clone(&lock);
        thread::spawn(move || {
            let timeout = Duration::from_millis(20);
            let start = Instant::now();
            assert!(!other.acquire(true, Some(timeout)));
            assert!(start.elapsed() >= timeout);
            assert!(other.locked());
            assert!(other.release().is_ok());
        })
        .join()
        .unwrap();
        assert!(!lock.locked());
        assert!(lock.acquire(false, None));
        assert!(lock.release().is_ok());
        assert!(lock.release().is_err());
    }

    #[test]
    fn a_blocking_reacquire_needs_a_foreign_release() {
        let lock = Arc::new(PrimitiveLock::default());
        let (acquired, events) = mpsc::channel();
        let other = Arc::clone(&lock);
        let worker = thread::spawn(move || {
            assert!(other.acquire(true, None));
            acquired.send(()).unwrap();
            assert!(other.acquire(true, None));
            acquired.send(()).unwrap();
            assert!(other.release().is_ok());
        });
        let limit = Duration::from_secs(5);
        events.recv_timeout(limit).unwrap();
        assert_eq!(
            events.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert!(lock.release().is_ok());
        events.recv_timeout(limit).unwrap();
        worker.join().unwrap();
        assert!(!lock.locked());
    }

    #[test]
    fn release_wakes_contenders_one_at_a_time() {
        let lock = PrimitiveLock::default();
        let (trying, attempts) = mpsc::channel();
        let (acquired, holders) = mpsc::channel();
        let limit = Duration::from_secs(5);
        assert!(lock.acquire(true, None));
        thread::scope(|scope| {
            for _ in 0..5 {
                let (trying, acquired, lock) = (trying.clone(), acquired.clone(), &lock);
                scope.spawn(move || {
                    assert!(!lock.acquire(false, None));
                    trying.send(()).unwrap();
                    assert!(lock.acquire(true, Some(limit)));
                    let (release, permission) = mpsc::channel();
                    acquired.send(release).unwrap();
                    permission.recv_timeout(limit).unwrap();
                    assert!(lock.release().is_ok());
                });
            }
            for _ in 0..5 {
                attempts.recv_timeout(limit).unwrap();
            }
            assert!(holders.try_recv().is_err());
            assert!(lock.release().is_ok());
            for _ in 0..5 {
                let release = holders.recv_timeout(limit).unwrap();
                assert!(lock.locked());
                assert!(!lock.acquire(false, None));
                assert!(holders.try_recv().is_err());
                release.send(()).unwrap();
            }
        });
        assert!(!lock.locked());
    }
}
