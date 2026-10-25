//! Synchronous context managers suspend their native frame at a bare yield.
//! Each resume restores the caller's dynamically active exception and yielder.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use corosensei::{Coroutine, CoroutineResult, Yielder};

use super::exceptions::{
    active_exception, propagates_context_exception, replace_pending, swap_active, take_pending,
    wrap_stop_iteration,
};
use super::records::imre_record_retain;
use super::words::WordKind;
use super::*;

type Signal = Option<crate::ObjectOwner>;
type Fiber = Coroutine<Signal, (), Signal>;
type Callback = unsafe extern "C" fn(*const usize) -> usize;

thread_local! {
    static CURRENT: Cell<*const Yielder<Signal, ()>> = const { Cell::new(std::ptr::null()) };
    static OWNER: Cell<*const Manager> = const { Cell::new(std::ptr::null()) };
}

pub(super) struct Manager {
    fiber: RefCell<Option<Fiber>>,
    active: RefCell<Signal>,
    entered: Cell<bool>,
    generator: Cell<bool>,
    closing: Cell<bool>,
    item: RefCell<Option<OwnedWord>>,
}

impl Manager {
    fn resume(&self, signal: Signal) -> CoroutineResult<(), Signal> {
        let previous = CURRENT.get();
        let owner = OWNER.replace(self);
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
        OWNER.set(owner);
        if matches!(result, CoroutineResult::Return(_)) {
            self.fiber.take();
        }
        result
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        if self.generator.get() && self.fiber.get_mut().is_some() {
            // A direct consumer can stop after any pull. Resume the compiler's
            // close edge so the native stack releases all of its local owners.
            let pending = take_pending();
            self.closing.set(true);
            *self.active.borrow_mut() = active_exception();
            let result = self.resume(None);
            debug_assert!(matches!(result, CoroutineResult::Return(_)));
            replace_pending(pending);
        }
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
        take_pending().map(|error| wrap_stop_iteration(error, "generator raised StopIteration"))
    });
    let manager = Rc::new(Manager {
        fiber: RefCell::new(Some(fiber)),
        active: RefCell::default(),
        entered: Cell::new(false),
        generator: Cell::new(false),
        closing: Cell::new(false),
        item: RefCell::default(),
    });
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
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
        CoroutineResult::Return(Some(raised))
            if error.is_some_and(|e| propagates_context_exception(&raised, &e)) =>
        {
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
    let error = unsafe { crate::ObjectOwner::acquire_word(error as usize) };
    // SAFETY: the caller roots the exact manager handle through this operation.
    let manager = unsafe { manager(handle) };
    u32::from(exit(&manager, Some(error)))
}

/// Suspends at a typed yield; false asks the generated frame to close.
///
/// # Safety
/// The generated caller roots a word of the specified representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_generator_yield(word: usize, kind: u32) -> u32 {
    let owner = OWNER.get();
    // SAFETY: resume installs the manager for the entire dynamic extent of its frame.
    let owner = unsafe { owner.as_ref() }.expect("yield outside a generator");
    // SAFETY: the caller roots a word of exactly this representation.
    let value = unsafe { WordKind::new(kind).acquire(word) };
    *owner.item.borrow_mut() = Some(value);
    imre_context_yield();
    u32::from(!owner.closing.get())
}

/// Runs the producer until it yields, raises, or finishes.
///
/// # Safety
/// The handle roots a generator created for this direct consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_generator_next(handle: *const Object) -> u32 {
    // SAFETY: the caller roots this exact manager throughout the pull.
    let owner = unsafe { manager(handle) };
    if owner.fiber.borrow().is_none() {
        return 0;
    }
    owner.generator.set(true);
    *owner.active.borrow_mut() = active_exception();
    match owner.resume(None) {
        CoroutineResult::Yield(()) => 1,
        CoroutineResult::Return(None) => 0,
        CoroutineResult::Return(Some(error)) => {
            replace_pending(Some(error));
            0
        }
    }
}

/// Transfers the new item to its consumer, without conflating None with exhaustion.
///
/// # Safety
/// The caller has completed one successful pull and takes the item exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_generator_take(handle: *const Object) -> usize {
    // SAFETY: the caller roots the exact generator throughout the transfer.
    let owner = unsafe { manager(handle) };
    owner
        .item
        .take()
        .expect("successful generator pull")
        .into_word()
}
