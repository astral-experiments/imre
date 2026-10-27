//! Reentrant I/O locks for logging handlers. The mutex protects bookkeeping only;
//! Python code runs with an owner recorded, never with the Rust guard borrowed.

use super::{Error, Int, Kind, Object, Result, Storage, StorageCell, finish, object};
use crate::objects::locks::{timeout_float, timeout_int};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct HandlerLock {
    owner: Mutex<Option<(ThreadId, usize)>>,
    available: Condvar,
}

impl HandlerLock {
    /// An owner may acquire recursively; other callers wait for the final release.
    pub(crate) fn acquire(&self, blocking: bool, timeout: Option<Duration>) -> Result<bool> {
        let id = thread::current().id();
        let mut owner = self.owner.lock().unwrap();
        if let Some((thread, depth)) = owner.as_mut()
            && *thread == id
        {
            *depth = depth
                .checked_add(1)
                .ok_or_else(|| Error::new(Kind::OverflowError, "Internal lock count overflowed"))?;
            return Ok(true);
        }
        if owner.is_some() {
            if !blocking {
                return Ok(false);
            }
            owner = if let Some(timeout) = timeout {
                let (state, _) = self
                    .available
                    .wait_timeout_while(owner, timeout, |s| s.is_some())
                    .unwrap();
                if state.is_some() {
                    return Ok(false);
                }
                state
            } else {
                self.available.wait_while(owner, |s| s.is_some()).unwrap()
            };
        }
        *owner = Some((id, 1));
        Ok(true)
    }

    /// Wake a waiting thread only after the matching outer release.
    pub(crate) fn release(&self) -> Result<()> {
        let mut owner = self.owner.lock().unwrap();
        if let Some((id, depth)) = owner.as_mut()
            && *id == thread::current().id()
        {
            *depth -= 1;
            if *depth == 0 {
                *owner = None;
                self.available.notify_one();
            }
            return Ok(());
        }
        Err(Error::new(
            Kind::RuntimeError,
            "cannot release un-acquired lock",
        ))
    }

    /// Report the current thread's recursion count, even if another thread owns it.
    fn depth(&self) -> usize {
        self.owner
            .lock()
            .unwrap()
            .filter(|(id, _)| *id == thread::current().id())
            .map_or(0, |(_, depth)| depth)
    }

    /// Reinitialize ownership, as Handler._at_fork_reinit requests in the child.
    fn reset(&self) {
        *self.owner.lock().unwrap() = None;
        self.available.notify_all();
    }
}

/// Allocate an unlocked handle owned independently of its handler.
#[unsafe(no_mangle)]
pub extern "C" fn imre_log_lock_new() -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::HandlerLock(Arc::default()),
    ))))
}

/// # Safety
/// The caller roots an exact lock. No object-storage borrow crosses a possible wait.
unsafe fn lock(value: *const Object) -> Arc<HandlerLock> {
    // SAFETY: the rooted handle contains this exact native lock; only an Arc escapes.
    unsafe {
        object(value).0.with(|s| match s {
            Storage::HandlerLock(lock) => Arc::clone(lock),
            _ => unreachable!("a handler lock has fixed storage"),
        })
    }
}

/// # Safety
/// The caller keeps the lock alive until the acquisition returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_acquire(
    value: *const Object,
    blocking: bool,
    seconds: f64,
) -> bool {
    let result = timeout_float(blocking, seconds).and_then(|wait| {
        // SAFETY: the caller supplies a live lock.
        unsafe { lock(value) }.acquire(blocking, wait)
    });
    finish(result, false)
}

/// # Safety
/// The caller roots the exact integer and handler lock for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_acquire_int(
    value: *const Object,
    blocking: bool,
    seconds: usize,
) -> bool {
    // SAFETY: the integer word is borrowed only until its nanosecond value is extracted.
    let seconds = unsafe { Int::borrow(seconds) };
    let result = timeout_int(blocking, &seconds)
        // SAFETY: the caller supplies a live lock.
        .and_then(|wait| unsafe { lock(value) }.acquire(blocking, wait));
    finish(result, false)
}

/// # Safety
/// A present pointer is a rooted lock. An absent optional manager cannot enter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_enter(value: usize) -> bool {
    let value = value as *const Object;
    if value.is_null() {
        return Error::new(
            Kind::TypeError,
            "'NoneType' object does not support the context manager protocol (missed __exit__ method)",
        )
        .returning(false);
    }
    // SAFETY: a present manager is rooted through this operation.
    finish(unsafe { lock(value) }.acquire(true, None), false)
}

/// # Safety
/// The optional word retains a live lock after successful context entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_exit(value: usize) {
    // SAFETY: the compiler exits only after successful acquisition.
    unsafe { imre_log_lock_release(value as *const Object) };
}

/// # Safety
/// The receiver is a rooted handler lock; a non-owner release sets RuntimeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_release(value: *const Object) {
    // SAFETY: no storage reference escapes release.
    finish(unsafe { lock(value) }.release(), ());
}

/// # Safety
/// The receiver is a rooted handler lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_owned(value: *const Object) -> bool {
    // SAFETY: only the current thread's ownership flag escapes.
    unsafe { lock(value) }.depth() != 0
}

/// # Safety
/// The receiver is a rooted handler lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_locked(value: *const Object) -> bool {
    // SAFETY: only the owner-present flag escapes.
    unsafe { lock(value) }.owner.lock().unwrap().is_some()
}

/// # Safety
/// The receiver is a rooted handler lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_count(value: *const Object) -> usize {
    // SAFETY: the returned integer owns its thread-local count independently.
    Int::from_usize(unsafe { lock(value) }.depth()).into_raw()
}

/// # Safety
/// The receiver is a rooted handler lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_reset(value: *const Object) {
    // SAFETY: the lock handle stays live across the reset.
    unsafe { lock(value) }.reset();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn only_the_outermost_release_unblocks_another_thread() {
        let lock = Arc::new(HandlerLock::default());
        assert_eq!(lock.acquire(true, None).ok(), Some(true));
        assert_eq!(lock.acquire(false, None).ok(), Some(true));
        let peer = Arc::clone(&lock);
        let (stage, ready) = mpsc::channel();
        let (proceed, next) = mpsc::channel();
        let child = thread::spawn(move || {
            assert_eq!(peer.depth(), 0);
            assert!(peer.release().is_err());
            assert_eq!(peer.acquire(false, None).ok(), Some(false));
            stage.send(()).unwrap();
            next.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(peer.acquire(true, Some(Duration::ZERO)).ok(), Some(false));
            stage.send(()).unwrap();
            assert_eq!(
                peer.acquire(true, Some(Duration::from_secs(10))).ok(),
                Some(true)
            );
            assert_eq!(peer.depth(), 1);
            assert!(peer.release().is_ok());
        });
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(lock.release().is_ok());
        assert_eq!(lock.depth(), 1);
        proceed.send(()).unwrap();
        ready.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(lock.release().is_ok());
        child.join().unwrap();
        assert_eq!(lock.depth(), 0);
    }
}
