//! Exercise the actual object ABI when getattr retains a default or cannot allocate a field view.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;

use imre_runtime::{Str, imre_str_literal, imre_str_release};

thread_local! {
    static REFUSE: Cell<bool> = const { Cell::new(false) };
    static WATCH: Cell<*const u8> = const { Cell::new(ptr::null()) };
}

struct ObserveObject;

// SAFETY: every successful allocation and its deallocation use System with the caller's layout.
unsafe impl GlobalAlloc for ObserveObject {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if REFUSE.try_with(|f| f.replace(false)).unwrap_or(false) {
            ptr::null_mut()
        } else {
            // SAFETY: the caller supplies a valid allocation layout.
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn dealloc(&self, address: *mut u8, layout: Layout) {
        let _ = WATCH.try_with(|p| {
            if p.get() == address {
                p.set(ptr::null());
            }
        });
        // SAFETY: this allocation came from System with the same layout.
        unsafe { System.dealloc(address, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: ObserveObject = ObserveObject;

unsafe extern "C" {
    fn imre_record_error_init();
    fn imre_box_value(kind: u32, word: usize) -> *const c_void;
    fn imre_logrecord_new(
        name: usize,
        nk: u32,
        level: usize,
        lk: u32,
        path: usize,
        pk: u32,
        line: usize,
        ik: u32,
        message: usize,
        mk: u32,
        args: usize,
        ak: u32,
        exc: usize,
        ek: u32,
        func: usize,
        fk: u32,
        stack: usize,
        sk: u32,
    ) -> *const c_void;
    fn imre_logrecord_dictionary(record: *const c_void) -> *const c_void;
    fn imre_logrecord_hasattr(record: *const c_void, key: *const c_void) -> bool;
    fn imre_logrecord_field_view(
        record: *const c_void,
        key: *const c_void,
        default: *const c_void,
    ) -> *const c_void;
    fn imre_value_repr(word: usize, kind: u32) -> *const c_void;
    fn imre_error_take() -> *const c_void;
    fn imre_exception_repr(value: *const c_void) -> *const c_void;
    fn imre_object_release(value: *const c_void);
}

#[test]
fn retained_default_and_refused_field_owner() {
    let present = Str::from("msg");
    let missing = Str::from("a missing name too long for an inline string");
    let qualname = Str::from("__qualname__");
    // SAFETY: each call borrows rooted inputs and each successful transfer is released once.
    unsafe {
        imre_record_error_init();
        let word = imre_str_literal(b"held".as_ptr(), 4);
        let record = imre_logrecord_new(
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            word as usize,
            3,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        );
        let other = imre_str_literal(b"fallback".as_ptr(), 8);
        let default = imre_box_value(3, other as usize);
        imre_str_release(other);
        imre_str_release(word);
        assert!(!record.is_null() && !default.is_null());
        assert!(imre_logrecord_hasattr(record, (&raw const qualname).cast()));

        // Missing data requires no wrapper or temporary AttributeError.
        REFUSE.set(true);
        let held = imre_logrecord_field_view(record, (&raw const missing).cast(), default);
        assert!(REFUSE.replace(false));
        assert_eq!(held, default);
        assert!(imre_error_take().is_null());
        WATCH.set(default.cast());
        imre_object_release(default);
        assert_eq!(WATCH.get(), held.cast());

        // Looking up the same long name in a live dictionary must borrow the caller's key.
        let fields = imre_logrecord_dictionary(record);
        assert!(!fields.is_null());
        imre_object_release(fields);
        REFUSE.set(true);
        let again = imre_logrecord_field_view(record, (&raw const missing).cast(), held);
        assert!(REFUSE.replace(false));
        assert_eq!(again, held);
        imre_object_release(again);

        // The present field needs its own view. Refuse that allocation and inspect the exact error.
        REFUSE.set(true);
        let failed = imre_logrecord_field_view(record, (&raw const present).cast(), held);
        assert!(!REFUSE.replace(false));
        assert!(failed.is_null());
        let error = imre_error_take();
        assert!(!error.is_null());
        let repr = imre_exception_repr(error).cast::<Str>();
        assert!(!repr.is_null());
        assert_eq!((*repr).as_str(), "MemoryError()");
        imre_str_release(repr);
        imre_object_release(error);

        let result = imre_logrecord_field_view(record, (&raw const present).cast(), held);
        assert!(!result.is_null());
        assert_ne!(result, held);
        imre_object_release(record);
        for (view, expected) in [(result, "'held'"), (held, "'fallback'")] {
            let value = imre_value_repr(view as usize, 5).cast::<Str>();
            assert!(!value.is_null());
            assert_eq!((*value).as_str(), expected);
            imre_str_release(value);
            imre_object_release(view);
        }
        assert!(WATCH.get().is_null());
        assert!(imre_error_take().is_null());
    }
}
