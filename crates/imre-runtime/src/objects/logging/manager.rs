//! Shared Logger managers refer to registry state. A private manager owns its root and limit.
use super::{Error, Int, LOGGERS, Object, RefCell, Result, Storage, StorageCell, finish, object};
use crate::ObjectOwner;
use std::ptr;

pub(in crate::objects) struct Manager {
    root: Option<ObjectOwner>,
    disable: RefCell<Int>,
}

thread_local! { static SHARED: RefCell<Option<ObjectOwner>> = const { RefCell::new(None) }; }

/// A missing payload denotes the registry's manager. Both Python objects use the
/// ordinary ObjectOwner allocation and release any retained root if it fails.
fn handle(state: Option<Manager>) -> Result<ObjectOwner> {
    ObjectOwner::try_new(Object(StorageCell::new(Storage::LogManager(state))))
        .map_err(|_| Error::memory_error())
}

unsafe fn with_state<T>(handle: *const Object, f: impl FnOnce(Option<&Manager>) -> T) -> T {
    // SAFETY: the caller roots its exact manager object throughout the storage borrow.
    unsafe {
        object(handle).0.with(|storage| match storage {
            Storage::LogManager(state) => f(state.as_ref()),
            _ => unreachable!("expected a logging manager"),
        })
    }
}

/// # Safety
/// A nonzero root is a borrowed native logger. Success transfers a manager owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_new(root: usize) -> *const Object {
    // SAFETY: the caller roots the present logger until the new manager acquires it.
    let root = (root != 0).then(|| unsafe { ObjectOwner::acquire_word(root) });
    let state = Manager {
        root,
        disable: RefCell::new(Int::small(0).unwrap()),
    };
    finish(handle(Some(state)).map(ObjectOwner::into_raw), ptr::null())
}

/// Every logger shares this identity. A failed first allocation leaves the cache empty.
#[unsafe(no_mangle)]
pub extern "C" fn imre_log_manager(_logger: *const Object) -> *const Object {
    let result = SHARED.with_borrow_mut(|cached| {
        let owner = match cached {
            Some(owner) => owner,
            None => cached.insert(handle(None)?),
        };
        Ok(ObjectOwner::into_raw(owner.clone()))
    });
    finish(result, ptr::null())
}

/// # Safety
/// The caller roots the manager and receives an owned optional logger.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_root(manager: *const Object) -> usize {
    // SAFETY: the root is retained before leaving either storage borrow.
    unsafe {
        with_state(manager, |state| match state {
            Some(manager) => manager.root.clone(),
            None => Some(LOGGERS.with_borrow(|logs| logs.entries["root"].handle.clone())),
        })
    }
    .map_or(0, |root| ObjectOwner::into_raw(root) as usize)
}

/// # Safety
/// The manager remains rooted while the level is copied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_disable(manager: *const Object) -> usize {
    // SAFETY: both stores retain their integer until the returned value owns a copy.
    unsafe {
        with_state(manager, |state| match state {
            Some(manager) => manager.disable.borrow().clone(),
            None => LOGGERS.with_borrow(|logs| logs.disable.clone()),
        })
    }
    .into_raw()
}

/// Direct assignment preserves cached decisions. logging.disable also clears them.
///
/// # Safety
/// The manager and checked level remain rooted throughout the write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_set_disable(manager: *const Object, level: usize) {
    // SAFETY: the caller roots both handles until the selected manager retains the level.
    unsafe {
        let value = (*Int::borrow(level)).clone();
        with_state(manager, |state| match state {
            Some(manager) => *manager.disable.borrow_mut() = value,
            None => LOGGERS.with_borrow_mut(|logs| logs.disable = value),
        });
    }
}
