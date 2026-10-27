//! Python.h operations supported by the rebuilt-extension ABI.

use super::handles::{Boxed, Owned};
use super::{Py, extensions_error};
use crate::objects::exceptions::imre_error_pending;
use crate::{Error, Int, Kind, imre_list_append, imre_list_set_index};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use std::ffi::{CStr, c_char, c_int, c_long, c_ulong, c_void};

// These distinct immortal addresses allow ordinary C variables of type
// PyObject * to hold the supported PyExc_* exception classes.
static EXCEPTIONS: [usize; 4] = [0; 4];
const CLASSES: [Kind; 4] = [
    Kind::ValueError,
    Kind::RuntimeError,
    Kind::TypeError,
    Kind::SystemError,
];

impl Py {
    pub(super) fn exception_kind(self) -> Option<Kind> {
        let offset = self.0.addr().wrapping_sub(EXCEPTIONS.as_ptr().addr());
        if offset < size_of_val(&EXCEPTIONS) && offset.is_multiple_of(size_of::<usize>()) {
            Some(CLASSES[offset / size_of::<usize>()])
        } else {
            None
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn imre_py_exception_class(kind: c_int) -> Py {
    let index = CLASSES
        .iter()
        .position(|value| *value as c_int == kind)
        .unwrap_or(3);
    Py(std::ptr::from_ref(&EXCEPTIONS[index]).cast_mut().cast())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_set_error(class: Py, message: *const c_char) {
    // SAFETY: PyErr_SetString accepts a NUL-terminated string valid for the call.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    extensions_error(
        class.exception_kind().unwrap_or(Kind::SystemError),
        message.as_ref(),
        (),
    );
}

#[unsafe(no_mangle)]
extern "C" fn imre_py_none() -> Py {
    Py::NONE
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_incref(value: Py) {
    // SAFETY: the extension roots a borrowed value, or supplies NULL for XINCREF.
    unsafe { value.retain() };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_decref(value: Py) {
    // SAFETY: the extension transfers ownership, or supplies NULL for XDECREF.
    unsafe { value.release() };
}

#[unsafe(no_mangle)]
extern "C" fn imre_py_long_from_unsigned_long(value: c_ulong) -> Py {
    Owned::integer(Int::from_usize(value as usize)).into_py()
}

#[unsafe(no_mangle)]
extern "C" fn imre_py_long_from_long(value: c_long) -> Py {
    Owned::integer(Int::new(BigInt::from(value))).into_py()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_bytes_from_string_and_size(data: *const c_char, size: isize) -> Py {
    let Ok(size) = usize::try_from(size) else {
        return Error::return_static_message(
            Kind::SystemError,
            "Negative size passed to PyBytes_FromStringAndSize",
            Py::NULL,
        );
    };
    if size > crate::MAX_BYTES_LEN {
        return Error::return_static_message(
            Kind::OverflowError,
            "byte string is too large",
            Py::NULL,
        );
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(size).is_err() {
        return Error::memory_error().returning(Py::NULL);
    }
    if data.is_null() {
        // C permits unspecified contents here. Do not expose uninitialized Rust memory.
        bytes.resize(size, 0);
    } else {
        // SAFETY: C supplies a readable buffer of size bytes, with no termination requirement.
        bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) });
    }
    Owned::try_bytes(bytes)
        .map_or_else(|| Error::memory_error().returning(Py::NULL), Owned::into_py)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_parse_count(args: Py, count: usize) -> c_int {
    // SAFETY: an extension must pass a rooted argument tuple.
    if let Some(Boxed::Tuple(values)) = unsafe { args.boxed() } {
        if values.len() == count {
            return 1;
        }
        return extensions_error(
            Kind::TypeError,
            format!("expected {count} arguments, received {}", values.len()),
            0,
        );
    }
    extensions_error(
        Kind::SystemError,
        "PyArg_ParseTuple requires an argument tuple",
        0,
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_tuple_item(args: Py, index: usize) -> Py {
    // SAFETY: Python.h's format parser holds its argument tuple during this read.
    if let Some(Boxed::Tuple(values)) = unsafe { args.boxed() }
        && let Some(value) = values.get(index)
    {
        return value.as_py();
    }
    extensions_error(
        Kind::IndexError,
        "argument tuple index out of range",
        Py::NULL,
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_parse_unsigned_long(value: Py, out: *mut c_ulong) -> c_int {
    let Some(word) = value.int_word() else {
        return extensions_error(Kind::TypeError, "an integer is required", 0);
    };
    // SAFETY: the C argument tuple roots the exact native integer.
    let value = unsafe { Int::borrow(word) };
    let masked = if let Some(small) = value.as_small() {
        small as c_ulong
    } else {
        // Python's k unit masks modulo the host's unsigned long width, including
        // negative and arbitrarily wide inputs.
        (&*value.big() & BigInt::from(c_ulong::MAX))
            .to_u64()
            .unwrap() as c_ulong
    };
    // SAFETY: the caller passed a writable unsigned long for this format unit.
    unsafe { out.write(masked) };
    1
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_parse_bytes(
    value: Py,
    data: *mut *const c_char,
    length: *mut isize,
) -> c_int {
    let Some(bytes) = value.bytes() else {
        return extensions_error(
            Kind::TypeError,
            "a read-only bytes-like object is required",
            0,
        );
    };
    // SAFETY: Python.h's argument tuple roots immutable bytes until the C
    // method returns; y# promises two writable out-pointers of these types.
    unsafe {
        data.write((*bytes).as_ptr().cast());
        length.write((*bytes).len() as isize);
    }
    1
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_list_size(value: Py) -> isize {
    let Some(list) = value.list() else {
        return extensions_error(Kind::TypeError, "expected list[int]", -1);
    };
    // SAFETY: the extension roots the list for this nonreentrant scalar read.
    unsafe { (*list).0.with(|items| items.len() as isize) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_list_get_item(value: Py, index: isize) -> Py {
    let Some(list) = value.list() else {
        return extensions_error(Kind::TypeError, "expected list[int]", Py::NULL);
    };
    // SAFETY: no storage reference escapes this access. An integer handle is
    // borrowed from the actual slot until the list or that element is replaced.
    let word = unsafe {
        (*list)
            .0
            .with(|items| items.get(index as usize).map(|item| item.0))
    };
    word.map_or_else(|| Error::index().returning(Py::NULL), Py::integer)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_list_set_item(value: Py, index: isize, item: Py) -> c_int {
    // SAFETY: PyList_SetItem steals one item reference, including on failure.
    let item = unsafe { Owned::take(item) };
    let (Some(list), Some(word)) = (value.list(), item.as_py().int_word()) else {
        return extensions_error(Kind::TypeError, "Imre list[int] requires an int", -1);
    };
    // SAFETY: value roots the list, item roots the integer, and the runtime
    // acquires it before replacing and releasing the previous element.
    if unsafe { imre_list_set_index(list, index as usize, word) } == 0 {
        -1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_list_append(value: Py, item: Py) -> c_int {
    let (Some(list), Some(word)) = (value.list(), item.int_word()) else {
        return extensions_error(Kind::TypeError, "Imre list[int] requires an int", -1);
    };
    // SAFETY: the extension roots both inputs. Append independently owns the item.
    if unsafe { imre_list_append(list, word) } == 0 {
        -1
    } else {
        0
    }
}

#[repr(C)]
struct IntCallback {
    invoke: unsafe extern "C" fn(*const c_void, usize) -> usize,
    context: *const c_void,
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_call_one_arg(callable: Py, argument: Py) -> Py {
    // SAFETY: both C arguments are initially rooted. Take independent owners
    // because a callback can clear C's globals or replace its argument's list slot.
    let (callable_owner, argument_owner) = unsafe {
        callable.retain();
        argument.retain();
        (Owned::take(callable), Owned::take(argument))
    };
    // SAFETY: the independent owner roots the record during inspection.
    let Some(Boxed::IntCallback(record)) = (unsafe { callable_owner.as_py().boxed() }) else {
        return extensions_error(
            Kind::TypeError,
            "expected a callable with signature (int) -> int",
            Py::NULL,
        );
    };
    let Some(word) = argument_owner.as_py().int_word() else {
        return extensions_error(Kind::TypeError, "callback requires an int", Py::NULL);
    };
    // SAFETY: LLVM emits the immutable two-word payload for Callable[[int], int].
    // No mutable storage access spans the call, and both frame owners live through it.
    let output = unsafe {
        let record = &*record.cast::<IntCallback>();
        (record.invoke)(record.context, word)
    };
    if imre_error_pending() != 0 {
        Py::NULL
    } else {
        Py::integer(output)
    }
}
