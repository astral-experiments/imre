//! Synchronous context managers suspend their native frame at a bare yield.
//! Each resume restores the caller's dynamically active exception and yielder.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use corosensei::{Coroutine, CoroutineResult, Yielder};

use super::exceptions::{active_exception, replace_pending, swap_active, take_pending};
use super::records::imre_record_retain;
use super::words::WordKind;
use super::*;

type Signal = Option<Rc<Object>>;
type Fiber = Coroutine<Signal, (), Signal>;
type Callback = unsafe extern "C" fn(*const usize) -> usize;

thread_local! { static CURRENT: Cell<*const Yielder<Signal, ()>> = const { Cell::new(std::ptr::null()) }; }

pub(super) struct Manager {
    fiber: RefCell<Option<Fiber>>,
    active: RefCell<Signal>,
    entered: Cell<bool>,
}

impl Manager {
    fn resume(&self, signal: Signal) -> CoroutineResult<(), Signal> {
        let previous = CURRENT.get();
        let mut saved = self.active.take();
        swap_active(&mut saved);
        let result = self
            .fiber
            .borrow_mut()
            .as_mut()
            .expect("live context")
            .resume(signal);
        swap_active(&mut saved);
        *self.active.borrow_mut() = saved;
        CURRENT.set(previous);
        if matches!(result, CoroutineResult::Return(_)) {
            self.fiber.take();
        }
        result
    }
}

unsafe fn manager(handle: *const Object) -> Rc<Manager> {
    // SAFETY: clone the private owner before entering arbitrary language code.
    unsafe {
        object(handle).0.with(|storage| match storage {
            Storage::GeneratorContext(manager) => manager.clone(),
            _ => unreachable!("typed context manager"),
        })
    }
}

/// # Safety
/// The caller roots an exact argument tuple for the supplied native thunk.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_context_new(
    callback: Callback,
    frame: *const usize,
) -> *const Object {
    // SAFETY: the fiber keeps one independent frame owner until its body completes.
    unsafe { imre_record_retain(frame) };
    // SAFETY: the preceding retain transferred one native record owner.
    let frame = unsafe { OwnedWord::from_owned(frame as usize, WordKind::new(6)) };
    let fiber = Coroutine::new(move |yielder, _| {
        CURRENT.set(yielder);
        // SAFETY: the frame roots this thunk's exact parameters.
        unsafe { callback(frame.word as *const usize) };
        drop(frame);
        take_pending()
    });
    let manager = Rc::new(Manager {
        fiber: RefCell::new(Some(fiber)),
        active: RefCell::default(),
        entered: Cell::new(false),
    });
    Rc::into_raw(Rc::new(Object(StorageCell::new(
        Storage::GeneratorContext(manager),
    ))))
}

/// # Safety
/// The handle roots a native context manager throughout its entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_context_enter(handle: *const Object) {
    // SAFETY: the caller roots the exact manager handle through this operation.
    let manager = unsafe { manager(handle) };
    if manager.entered.replace(true) {
        return Error::new(Kind::RuntimeError, "context manager already entered").returning(());
    }
    *manager.active.borrow_mut() = active_exception();
    match manager.resume(None) {
        CoroutineResult::Yield(()) => {}
        CoroutineResult::Return(Some(error)) => replace_pending(Some(error)),
        CoroutineResult::Return(None) => {
            Error::new(Kind::RuntimeError, "generator didn't yield").returning(())
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_context_yield() {
    let yielder = CURRENT.get();
    assert!(!yielder.is_null(), "yield outside its context manager");
    // SAFETY: CURRENT is installed only while this native generator is running.
    let signal = unsafe { &*yielder }.suspend(());
    CURRENT.set(yielder);
    replace_pending(signal);
}

fn exit(manager: &Manager, error: Signal) -> bool {
    if manager.fiber.borrow().is_none() {
        return true;
    }
    match manager.resume(error.clone()) {
        CoroutineResult::Return(None) => true,
        CoroutineResult::Return(Some(raised)) if error.is_some_and(|e| Rc::ptr_eq(&e, &raised)) => {
            false
        }
        CoroutineResult::Return(Some(raised)) => {
            replace_pending(Some(raised));
            false
        }
        CoroutineResult::Yield(()) => {
            Error::new(Kind::RuntimeError, "generator didn't stop").returning(false)
        }
    }
}

/// # Safety
/// The handle roots an entered or already exhausted native context manager.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_context_exit(handle: *const Object) {
    // SAFETY: the caller roots the exact manager handle through this operation.
    let manager = unsafe { manager(handle) };
    exit(&manager, None);
}

/// # Safety
/// Both handles remain rooted until the generator finishes its exception path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_context_throw(handle: *const Object, error: *const Object) -> u32 {
    // SAFETY: acquire the injected exception independently of the caller's handler.
    let error = unsafe { acquire_rc(error as usize) };
    // SAFETY: the caller roots the exact manager handle through this operation.
    let manager = unsafe { manager(handle) };
    u32::from(exit(&manager, Some(error)))
}
