//! Reentrant I/O locks for logging handlers. The mutex protects bookkeeping only;
//! Python code runs with an owner recorded, never with the Rust guard borrowed.

use super::{Error, Int, Kind, Object, Result, Storage, StorageCell, finish, object};
use crate::objects::locks::{WaitPhase, timeout_float, timeout_int};
use std::sync::{Arc, Condvar, Mutex};
#[cfg(test)]
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct State {
    owner: Option<(usize, usize)>,
    // Reinitializing clears the parking bit without waking already parked threads.
    parked: bool,
}

pub(crate) struct HandlerLock {
    creator_pid: u32,
    state: Mutex<State>,
    available: Condvar,
}

impl Default for HandlerLock {
    fn default() -> Self {
        Self {
            creator_pid: std::process::id(),
            state: Mutex::default(),
            available: Condvar::new(),
        }
    }
}

fn owner_ident() -> usize {
    // SAFETY: pthread_self has no preconditions and returns the calling thread's identifier.
    unsafe { libc::pthread_self() as usize }
}

impl HandlerLock {
    /// An owner may acquire recursively; other callers wait for the final release.
    pub(crate) fn acquire(&self, blocking: bool, timeout: Option<Duration>) -> Result<bool> {
        Ok(self.acquire_slice(blocking, timeout, WaitPhase::Initial))
    }

    pub(crate) fn acquire_slice(
        &self,
        blocking: bool,
        timeout: Option<Duration>,
        phase: WaitPhase,
    ) -> bool {
        let id = owner_ident();
        let mut state = self.state.lock().unwrap();
        if phase == WaitPhase::Initial
            && let Some((thread, depth)) = state.owner.as_mut()
            && *thread == id
        {
            // CPython's unsigned recursion counter also wraps after a forged saved count.
            *depth = depth.wrapping_add(1);
            return true;
        }
        if state.owner.is_some() || (phase == WaitPhase::Parked && !state.parked) {
            if !blocking || timeout == Some(Duration::ZERO) {
                return false;
            }
            // A polling timeout does not re-enroll a waiter after _at_fork_reinit.
            if phase != WaitPhase::Parked {
                state.parked = true;
            }
            let must_wait = |s: &mut State| s.owner.is_some() || !s.parked;
            state = if let Some(timeout) = timeout {
                let (state, timed) = self
                    .available
                    .wait_timeout_while(state, timeout, must_wait)
                    .unwrap();
                if timed.timed_out() {
                    return false;
                }
                state
            } else {
                self.available.wait_while(state, must_wait).unwrap()
            };
        }
        state.owner = Some((id, 1));
        true
    }

    /// Wake a waiting thread only after the matching outer release.
    pub(crate) fn release(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some((id, depth)) = state.owner.as_mut()
            && *id == owner_ident()
        {
            *depth = depth.wrapping_sub(1);
            if *depth == 0 {
                state.owner = None;
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
    pub(crate) fn depth(&self) -> usize {
        self.state()
            .filter(|(id, _)| *id == owner_ident())
            .map_or(0, |(_, depth)| depth)
    }

    /// Ownership is independent of depth when a private saved count wraps to zero.
    pub(crate) fn owned(&self) -> bool {
        self.state()
            .is_some_and(|(owner, _)| owner == owner_ident())
    }

    /// Read the pthread owner and raw recursion count together for repr.
    pub(crate) fn state(&self) -> Option<(usize, usize)> {
        self.state.lock().unwrap().owner
    }

    /// Fully release a recursively acquired lock for a condition wait.
    pub(crate) fn release_save(&self) -> Result<(usize, usize)> {
        let mut state = self.state.lock().unwrap();
        let Some((owner, depth)) = state.owner.filter(|(owner, _)| *owner == owner_ident()) else {
            return Err(Error::new(
                Kind::RuntimeError,
                "cannot release un-acquired lock",
            ));
        };
        state.owner = None;
        self.available.notify_one();
        Ok((depth, owner))
    }

    /// Wait for the underlying lock, then restore its saved recursion depth and owner.
    pub(crate) fn acquire_restore(&self, depth: usize, owner: usize) {
        let current = owner_ident();
        let mut state = self.state.lock().unwrap();
        if state.owner.is_some_and(|(owner, _)| owner != current) {
            state.parked = true;
            state = self
                .available
                .wait_while(state, |s| {
                    s.owner.is_some_and(|(owner, _)| owner != current) || !s.parked
                })
                .unwrap();
        }
        state.owner = Some((owner, depth));
    }

    /// Preserve native aliases in this process. A forked child must never acquire
    /// the parent's bookkeeping mutex, which could be held by a vanished thread.
    pub(crate) fn at_fork_reinit(lock: &mut Arc<Self>) {
        if lock.creator_pid == std::process::id() {
            *lock.state.lock().unwrap() = State::default();
        } else {
            *lock = Arc::default();
        }
    }

    /// Reinitialize ownership, as Handler._at_fork_reinit requests in the child.
    fn reset(&self) {
        self.state.lock().unwrap().owner = None;
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
    unsafe { lock(value) }.owned()
}

/// # Safety
/// The receiver is a rooted handler lock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_lock_locked(value: *const Object) -> bool {
    // SAFETY: only the owner-present flag escapes.
    unsafe { lock(value) }.state().is_some()
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
    fn condition_save_and_restore_hands_the_lock_to_another_thread() {
        let lock = HandlerLock::default();
        assert_eq!(lock.acquire(true, None).ok(), Some(true));
        assert_eq!(lock.acquire(true, None).ok(), Some(true));
        let (stage, progress) = mpsc::channel();
        let (release, finish) = mpsc::channel();
        let limit = Duration::from_secs(5);
        thread::scope(|scope| {
            let lock = &lock;
            scope.spawn(move || {
                assert_eq!(lock.acquire(false, None).ok(), Some(false));
                assert_eq!(
                    lock.acquire(true, Some(Duration::from_millis(10))).ok(),
                    Some(false)
                );
                assert!(lock.release_save().is_err());
                stage.send(()).unwrap();
                assert_eq!(lock.acquire(true, Some(limit)).ok(), Some(true));
                stage.send(()).unwrap();
                finish.recv_timeout(limit).unwrap();
                assert!(lock.release().is_ok());
            });
            progress.recv_timeout(limit).unwrap();
            let (depth, owner) = lock.release_save().ok().unwrap();
            assert_eq!(depth, 2);
            assert_eq!(owner, owner_ident());
            progress.recv_timeout(limit).unwrap();
            assert!(!lock.owned());
            assert!(lock.release().is_err());
            release.send(()).unwrap();
            lock.acquire_restore(depth, owner);
            assert!(lock.owned());
            assert_eq!(lock.depth(), 2);
        });
        assert!(lock.release().is_ok());
        assert!(lock.release().is_ok());
        assert!(lock.state().is_none());
    }

    #[test]
    fn restored_state_can_name_another_live_owner_and_wrap_its_count() {
        let lock = HandlerLock::default();
        let (identify, ids) = mpsc::channel();
        let (restored, proceed) = mpsc::channel();
        let limit = Duration::from_secs(5);
        thread::scope(|scope| {
            let lock = &lock;
            let child = scope.spawn(move || {
                let me = owner_ident();
                identify.send(me).unwrap();
                proceed.recv_timeout(limit).unwrap();
                assert!(lock.owned());
                assert_eq!(lock.depth(), usize::MAX);
                assert_eq!(lock.acquire(false, None).ok(), Some(true));
                assert_eq!(lock.depth(), 0);
                assert!(lock.owned());
                assert!(lock.release().is_ok());
                assert_eq!(lock.release_save().ok(), Some((usize::MAX, me)));
            });
            let peer = ids.recv_timeout(limit).unwrap();
            assert_ne!(peer, owner_ident());
            assert_eq!(lock.acquire(false, None).ok(), Some(true));
            lock.acquire_restore(usize::MAX, peer);
            assert_eq!(lock.state(), Some((peer, usize::MAX)));
            assert!(!lock.owned());
            assert_eq!(lock.depth(), 0);
            assert_eq!(lock.acquire(false, None).ok(), Some(false));
            assert!(lock.release().is_err());
            restored.send(()).unwrap();
            child.join().unwrap();
        });
        assert!(lock.state().is_none());
        lock.acquire_restore(2, 0);
        assert_eq!(lock.state(), Some((0, 2)));
        assert!(!lock.owned());
        assert_eq!(lock.acquire(false, None).ok(), Some(false));
        assert!(lock.release_save().is_err());
        lock.reset();
        assert!(lock.state().is_none());
    }

    #[test]
    fn reset_leaves_a_parked_waiter_enrolled_until_timeout() {
        let mut lock = Arc::new(HandlerLock::default());
        assert_eq!(lock.acquire(true, None).ok(), Some(true));
        thread::scope(|scope| {
            let peer = Arc::clone(&lock);
            let waiter = scope.spawn(move || peer.acquire(true, Some(Duration::from_secs(1))).ok());
            let started = std::time::Instant::now();
            while !lock.state.lock().unwrap().parked {
                assert!(started.elapsed() < Duration::from_secs(5));
                thread::yield_now();
            }

            HandlerLock::at_fork_reinit(&mut lock);
            assert_eq!(lock.acquire(false, None).ok(), Some(true));
            // A zero-timeout probe must not re-arm the cleared parking bit.
            let peer = Arc::clone(&lock);
            let probe = scope.spawn(move || peer.acquire(true, Some(Duration::ZERO)).ok());
            assert_eq!(probe.join().unwrap(), Some(false));
            assert!(lock.release().is_ok());
            assert_eq!(waiter.join().unwrap(), Some(false));
        });
    }

    #[test]
    fn polling_preserves_waiter_enrollment_and_interrupts_do_not_recurse() {
        let mut lock = Arc::new(HandlerLock::default());
        lock.acquire_restore(1, !owner_ident());
        let slice = Some(Duration::from_millis(1));
        assert!(!lock.acquire_slice(true, slice, WaitPhase::Initial));
        HandlerLock::at_fork_reinit(&mut lock);
        assert!(!lock.acquire_slice(true, slice, WaitPhase::Parked));

        // A returning signal handler restarts the underlying mutex acquisition.
        assert!(lock.acquire_slice(true, slice, WaitPhase::Interrupted));
        assert!(!lock.acquire_slice(true, slice, WaitPhase::Interrupted));
        assert_eq!(lock.depth(), 1);
        assert!(lock.release().is_ok());
        assert!(lock.acquire_slice(true, Some(Duration::ZERO), WaitPhase::Parked));
    }

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
