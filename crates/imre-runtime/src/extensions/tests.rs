use super::*;
use crate::objects::exceptions::{imre_exception_cause, imre_exception_matches};

thread_local! {
    static INITIALIZED: Cell<usize> = const { Cell::new(0) };
    static RELEASED: Cell<usize> = const { Cell::new(0) };
    static REENTERED: Cell<bool> = const { Cell::new(false) };
    static DEFINITION: ModuleDef = const { ModuleDef {
        base: 0, name: c"probe".as_ptr(), doc: std::ptr::null(), size: -1,
        methods: std::ptr::null(), slots: std::ptr::null(), traverse: std::ptr::null(),
        clear: std::ptr::null(), free: Some(free),
    }};
}

fn take_matches(kind: Kind) -> bool {
    let pending = imre_error_take();
    if pending.is_null() {
        return false;
    }
    // SAFETY: take transferred one owned exception.
    unsafe {
        let matches = imre_exception_matches(pending, kind as u32) != 0;
        imre_exception_release(pending);
        matches
    }
}

unsafe extern "C" fn init() -> Py {
    INITIALIZED.set(INITIALIZED.get() + 1);
    // SAFETY: the test keeps this thread's definition alive through explicit teardown.
    DEFINITION.with(|definition| unsafe { imre_py_module_create(definition) })
}

unsafe extern "C" fn free(_: *mut c_void) {
    RELEASED.set(RELEASED.get() + 1);
    // SAFETY: the same live initializer is called while the module is being freed.
    unsafe { imre_extension_import(init) };
    REENTERED.set(take_matches(Kind::RuntimeError));
    Error::new(Kind::IndexError, "cleanup error").raise();
}

#[test]
fn cleanup_reentry_keeps_original_exception() {
    // SAFETY: these two calls share one definition and static initializer.
    unsafe {
        imre_extension_import(init);
        imre_extension_import(init);
    }
    assert_eq!(INITIALIZED.get(), 1);
    Error::new(Kind::ValueError, "program error").raise();
    imre_extension_clear();
    assert!(take_matches(Kind::ValueError));
    assert_eq!(INITIALIZED.get(), 1);
    assert_eq!(RELEASED.get(), 1);
    assert!(REENTERED.get());
    imre_extension_clear();
    assert_eq!(RELEASED.get(), 1);
    assert_eq!(imre_error_pending(), 0);
}

#[test]
fn c_results_obey_the_pending_exception_contract() {
    Error::new(Kind::ValueError, "callback error").raise();
    // SAFETY: NULL with an existing exception propagates it unchanged.
    assert!(unsafe { checked_result(Py::NULL) }.is_none());
    assert!(take_matches(Kind::ValueError));
    // SAFETY: NULL without an existing exception is a C method protocol error.
    assert!(unsafe { checked_result(Py::NULL) }.is_none());
    assert!(take_matches(Kind::SystemError));
    Error::new(Kind::ValueError, "callback error").raise();
    // SAFETY: a returned immortal None represents success with an illegal pending error.
    assert!(unsafe { checked_result(Py::NONE) }.is_none());
    let error = imre_error_take();
    // SAFETY: the pending slot transfers the SystemError, which owns its original cause.
    unsafe {
        assert_ne!(imre_exception_matches(error, Kind::SystemError as u32), 0);
        let cause = imre_exception_cause(error);
        assert!(!cause.is_null());
        assert_ne!(imre_exception_matches(cause, Kind::ValueError as u32), 0);
        imre_exception_release(cause);
        imre_exception_release(error);
    }
}

#[test]
fn only_unobserved_argument_tuples_are_reused() {
    clear_tuple_cache();
    let args = argument_tuple(SmallVec::new());
    let retained = Rc::clone(&args);
    release_arguments(args);
    let independent = argument_tuple(SmallVec::new());
    assert!(!Rc::ptr_eq(&independent, &retained));
    let address = Rc::as_ptr(&independent);
    release_arguments(independent);
    let next = argument_tuple(SmallVec::new());
    assert_eq!(Rc::as_ptr(&next), address);
    clear_tuple_cache();
}
