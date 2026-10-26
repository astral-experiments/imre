//! Fail allocations inside the exported C constructor, leaving all other threads alone.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::ptr;

use imre_runtime::Str;

thread_local! { static FAIL_AT: Cell<usize> = const { Cell::new(0) }; }

struct FailNth;

// SAFETY: successful allocations use System's layout and matching deallocator.
unsafe impl GlobalAlloc for FailNth {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if FAIL_AT
            .try_with(|n| {
                let remaining = n.get();
                n.set(remaining.saturating_sub(1));
                remaining == 1
            })
            .unwrap_or(false)
        {
            ptr::null_mut()
        } else {
            // SAFETY: the caller supplied a valid allocation layout.
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: every non-null allocation came from System with this layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: FailNth = FailNth;

unsafe extern "C" {
    fn imre_py_bytes_from_string_and_size(data: *const c_char, size: isize) -> *const c_void;
    fn imre_py_decref(value: *const c_void);
    fn imre_record_error_init();
    fn imre_error_take() -> *const c_void;
    fn imre_exception_repr(value: *const c_void) -> *const c_void;
    fn imre_object_release(value: *const c_void);
    fn imre_str_release(value: *const c_void);
}

fn assert_memory_error() {
    // SAFETY: take transfers a rooted exception, and repr transfers its rooted string.
    unsafe {
        let error = imre_error_take();
        assert!(!error.is_null());
        let message = imre_exception_repr(error);
        assert!(!message.is_null());
        assert_eq!((&*message.cast::<Str>()).as_str(), "MemoryError()");
        imre_str_release(message);
        imre_object_release(error);
    }
}

#[test]
fn invalid_size_diagnostics_remain_catchable() {
    // SAFETY: invalid lengths are diagnosed before the null source can be read.
    unsafe {
        imre_record_error_init();
        for size in [-1, isize::MAX] {
            for allocation in [1, 2] {
                FAIL_AT.set(allocation);
                let value = imre_py_bytes_from_string_and_size(ptr::null(), size);
                assert_eq!(FAIL_AT.replace(0), 0);
                assert!(value.is_null());
                assert_memory_error();
            }
        }
    }
}

#[test]
fn payload_and_owner_failures_reach_the_caller() {
    let data = [42_u8; 257];
    // SAFETY: data stays readable; each returned exception, string, and C
    // object remains rooted until released exactly once with its matching API.
    unsafe {
        for allocation in [1, 2] {
            FAIL_AT.set(allocation);
            let value =
                imre_py_bytes_from_string_and_size(data.as_ptr().cast(), data.len() as isize);
            assert_eq!(FAIL_AT.replace(0), 0);
            assert!(value.is_null());
            assert_memory_error();

            let value =
                imre_py_bytes_from_string_and_size(data.as_ptr().cast(), data.len() as isize);
            assert!(!value.is_null());
            assert!(imre_error_take().is_null());
            imre_py_decref(value);
        }
    }
}

#[test]
fn held_errors_remain_independent_until_the_reserve_is_exhausted() {
    fn fail() -> *const c_void {
        // SAFETY: the readable source stays in place until the C constructor returns.
        unsafe {
            let data = [42_u8; 257];
            FAIL_AT.set(1);
            let value =
                imre_py_bytes_from_string_and_size(data.as_ptr().cast(), data.len() as isize);
            assert_eq!(FAIL_AT.replace(0), 0);
            assert!(value.is_null());
            let error = imre_error_take();
            assert!(!error.is_null());
            error
        }
    }

    // SAFETY: the generated entry point prepares the reserve before user code.
    unsafe { imre_record_error_init() };
    let mut held = [ptr::null(); 18];
    for index in 0..held.len() {
        held[index] = fail();
        if index < 17 {
            assert!(!held[..index].contains(&held[index]));
        }
    }
    assert_eq!(held[16], held[17]);
    // SAFETY: each take transfers one reference, including both last-resort aliases.
    unsafe {
        imre_object_release(held[3]);
        held[3] = fail();
        assert_ne!(held[3], held[16]);
        for error in held {
            imre_object_release(error);
        }
    }
}
