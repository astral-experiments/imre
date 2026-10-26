//! Native modules rebuilt against Imre's Python.h. C retains opaque handles;
//! shared lists keep the same Imre allocation and checked calls may reenter.

mod api;
mod handles;
#[cfg(test)]
mod tests;

use crate::objects::exceptions::{
    imre_error_pending, imre_error_set, imre_error_take, imre_exception_release,
};
use crate::{Error, Kind};
use handles::{Boxed, Owned, argument_tuple, clear_tuple_cache, release_arguments};
use smallvec::SmallVec;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::rc::Rc;

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Py(*mut c_void);

impl Py {
    const NULL: Self = Self(std::ptr::null_mut());
}

type Init = unsafe extern "C" fn() -> Py;
type Method = unsafe extern "C" fn(Py, Py) -> Py;

/// Matches the declarations in the installed Python.h.
#[repr(C)]
struct MethodDef {
    name: *const c_char,
    function: Option<Method>,
    flags: c_int,
    doc: *const c_char,
}

#[repr(C)]
struct ModuleDef {
    base: c_int,
    name: *const c_char,
    doc: *const c_char,
    size: isize,
    methods: *const MethodDef,
    slots: *const c_void,
    traverse: *const c_void,
    clear: *const c_void,
    free: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[cold]
fn extensions_error<T>(kind: Kind, message: impl Into<crate::Str>, sentinel: T) -> T {
    Error::new(kind, message).returning(sentinel)
}

thread_local! {
    // None marks a module whose initializer is running. Never hold a RefCell
    // borrow while C, a callable, or a module deallocator can run.
    static MODULES: RefCell<HashMap<usize, Option<Owned>>> = RefCell::default();
    static FINALIZING: Cell<bool> = const { Cell::new(false) };
}

/// Pairs a transferred C result with the existing Imre exception channel.
unsafe fn checked_result(value: Py) -> Option<Owned> {
    let pending = imre_error_pending() != 0;
    if value == Py::NULL {
        if !pending {
            extensions_error(
                Kind::SystemError,
                "extension returned NULL without an exception",
                (),
            );
        }
        return None;
    }
    // SAFETY: non-null extension returns transfer one owned reference.
    let result = unsafe { Owned::take(value) };
    if pending {
        Error::new(
            Kind::SystemError,
            "extension returned a value with an exception set",
        )
        .with_pending_cause()
        .returning(None)
    } else {
        Some(result)
    }
}

unsafe fn module(init: Init) -> Option<Owned> {
    if FINALIZING.get() {
        return extensions_error(
            Kind::RuntimeError,
            "extension modules are shutting down",
            None,
        );
    }
    let key = init as usize;
    if let Some(cached) = MODULES.with_borrow_mut(|modules| {
        if let Some(state) = modules.get(&key) {
            Some(state.clone())
        } else {
            modules.insert(key, None);
            None
        }
    }) {
        return cached.or_else(|| {
            extensions_error(
                Kind::RuntimeError,
                "recursive extension initialization",
                None,
            )
        });
    }
    // SAFETY: the linker supplied this extension's PyInit symbol; no runtime
    // storage or module-cache borrow spans the call.
    let loaded = unsafe { checked_result(init()) }.and_then(|result| {
        // SAFETY: this owned result stays rooted during inspection.
        if matches!(unsafe { result.as_py().boxed() }, Some(Boxed::Module(_))) {
            Some(result)
        } else {
            extensions_error(
                Kind::SystemError,
                "extension initializer did not return a module",
                None,
            )
        }
    });
    MODULES.with_borrow_mut(|modules| {
        if let Some(module) = &loaded {
            modules.insert(key, Some(module.clone()));
        } else {
            modules.remove(&key);
        }
    });
    loaded
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_extension_import(init: Init) {
    // SAFETY: generated imports pass a live extension initializer function.
    unsafe { module(init) };
}

/// Free hooks may call Python.h. Preserve the pending program exception and
/// release module state before the compiler tears down globals and literals.
#[unsafe(no_mangle)]
extern "C" fn imre_extension_clear() {
    if FINALIZING.replace(true) {
        return;
    }
    let pending = imre_error_take();
    let modules = MODULES.with_borrow_mut(std::mem::take);
    drop(modules);
    clear_tuple_cache();
    // SAFETY: take transfers one reference or null. Set acquires, and release
    // consumes the reference that kept the original error alive across free hooks.
    unsafe {
        imre_error_set(pending);
        imre_exception_release(pending);
    }
    FINALIZING.set(false);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn imre_py_module_create(def: *const ModuleDef) -> Py {
    // SAFETY: Python.h requires a static PyModuleDef.
    let Some(definition) = (unsafe { def.as_ref() }) else {
        return extensions_error(Kind::SystemError, "missing PyModuleDef", Py::NULL);
    };
    if definition.size > 0
        || !definition.slots.is_null()
        || !definition.traverse.is_null()
        || !definition.clear.is_null()
    {
        return extensions_error(
            Kind::SystemError,
            "unsupported PyModuleDef slots or per-module state",
            Py::NULL,
        );
    }
    Owned::new(Boxed::Module(def)).into_py()
}

/// Looks up a C method in a module's sentinel-terminated table.
unsafe fn find_method(module: &Owned, name: &CStr) -> Option<Method> {
    // SAFETY: the module cache accepts only module objects with a live definition.
    let Some(Boxed::Module(def)) = (unsafe { module.as_py().boxed() }) else {
        unreachable!()
    };
    // SAFETY: nonempty method tables terminate with a null name, per Python.h.
    let mut entry = unsafe { (**def).methods };
    while let Some(method) = unsafe {
        // SAFETY: C promises the table entries and their names remain live.
        entry.as_ref()
    } {
        if method.name.is_null() {
            break;
        }
        // SAFETY: names in a valid PyMethodDef are NUL-terminated.
        if unsafe { CStr::from_ptr(method.name) } == name {
            if method.flags != 1 || method.function.is_none() {
                return extensions_error(
                    Kind::SystemError,
                    "extension method must use METH_VARARGS",
                    None,
                );
            }
            return method.function;
        }
        // SAFETY: the current name was non-null and the array has a sentinel.
        entry = unsafe { entry.add(1) };
    }
    extensions_error(
        Kind::AttributeError,
        format!("extension has no method '{}'", name.to_string_lossy()),
        None,
    )
}

/// Borrows exact native arguments, invokes C, and transfers an owned native
/// result. The caller checks pending before interpreting the returned word.
#[unsafe(no_mangle)]
unsafe extern "C" fn imre_extension_call(
    init: Init,
    name: *const c_char,
    kinds: *const u8,
    words: *const usize,
    argc: usize,
    returns: u32,
) -> usize {
    // SAFETY: compiler passes a linked PyInit and NUL-terminated method name.
    let Some(module) = (unsafe { module(init) }) else {
        return 0;
    };
    // SAFETY: generated import stubs keep the method name alive.
    let Some(method) = (unsafe { find_method(&module, CStr::from_ptr(name)) }) else {
        return 0;
    };
    let mut args = SmallVec::with_capacity(argc);
    for index in 0..argc {
        // SAFETY: parallel arrays contain exactly argc entries, with borrowed
        // native handles of the compiler-verified type at each position.
        let Some(value) = (unsafe { Owned::from_native(*kinds.add(index), *words.add(index)) })
        else {
            return 0;
        };
        args.push(value);
    }
    let args = argument_tuple(args);
    let tuple = Py(Rc::as_ptr(&args).cast_mut().cast());
    // SAFETY: C receives two live borrowed objects and can retain either.
    // Both are owned by this stack frame, even across reentrant callbacks.
    let result = unsafe { checked_result(method(module.as_py(), tuple)) };
    release_arguments(args);
    result
        .and_then(|result| result.native(returns))
        .unwrap_or(0)
}
