//! A manager owns the level limit used by the loggers in its hierarchy.
use super::{Int, LOGGERS, Object, Rc, RefCell, Storage, StorageCell, object};

pub(in crate::objects) struct Manager {
    root: Option<Rc<Object>>,
    pub(super) disable: RefCell<Int>,
}

impl Manager {
    pub(super) fn new(root: Option<Rc<Object>>) -> Rc<Self> {
        Rc::new(Self {
            root,
            disable: RefCell::new(Int::small(0).unwrap()),
        })
    }

    pub(super) fn handle(self: &Rc<Self>) -> Rc<Object> {
        Rc::new(Object(StorageCell::new(Storage::LogManager(self.clone()))))
    }
}

unsafe fn state(handle: *const Object) -> Rc<Manager> {
    // SAFETY: the caller holds an exact manager handle throughout this borrow.
    unsafe {
        object(handle).0.with(|storage| match storage {
            Storage::LogManager(manager) => manager.clone(),
            _ => super::super::fail("SystemError", "expected logging.Manager"),
        })
    }
}

/// # Safety
/// A nonzero root is a borrowed native logger; the result owns a new manager.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_new(root: usize) -> *const Object {
    // SAFETY: only the present branch acquires the rooted logger.
    let root = (root != 0).then(|| unsafe { super::super::acquire_rc(root) });
    Rc::into_raw(Manager::new(root).handle())
}

/// Directly constructed loggers inherit the same class manager as the root.
#[unsafe(no_mangle)]
pub extern "C" fn imre_log_manager(_logger: *const Object) -> *const Object {
    LOGGERS.with_borrow(|logs| Rc::into_raw(logs.manager_handle.clone()))
}

/// # Safety
/// The caller roots the manager and receives an owned optional logger.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_root(manager: *const Object) -> usize {
    // SAFETY: the cloned state retains the root during the copy.
    unsafe { state(manager) }
        .root
        .clone()
        .map_or(0, |root| Rc::into_raw(root) as usize)
}

/// # Safety
/// The manager remains rooted while the level is copied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_disable(manager: *const Object) -> usize {
    // SAFETY: the caller supplies the live manager.
    unsafe { state(manager) }
        .disable
        .borrow()
        .clone()
        .into_raw()
}

/// Direct property assignment preserves cached decisions; logging.disable clears them.
///
/// # Safety
/// The manager and checked level remain rooted throughout the write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_manager_set_disable(manager: *const Object, level: usize) {
    // SAFETY: the level is copied before either caller-owned reference is released.
    let (manager, level) = unsafe { (state(manager), Int::borrow(level)) };
    manager.disable.replace((*level).clone());
}
