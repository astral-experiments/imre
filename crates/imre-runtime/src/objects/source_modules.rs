//! A closed import graph has one live module per import name. Holding the registry
//! reference also keeps a module alive after the last local alias is discarded.

use super::{Object, Rc, Storage, StorageCell, Str, acquire_rc, object};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static MODULES: RefCell<HashMap<Rc<Str>, Rc<Object>>> = RefCell::default();
}

/// Return an owned handle for this already imported module.
///
/// # Safety
/// Both arguments are rooted native strings for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_source_module(
    name: *const Str,
    filename: *const Str,
) -> *const Object {
    MODULES.with_borrow_mut(|modules| {
        // SAFETY: the compiler supplies the exact import name and verified source path.
        if let Some(module) = modules.get(unsafe { &*name }) {
            return Rc::into_raw(Rc::clone(module));
        }
        // SAFETY: retain independent references to the two rooted strings.
        let (name, filename) = unsafe {
            (
                acquire_rc::<Str>(name as usize),
                acquire_rc(filename as usize),
            )
        };
        let module = Rc::new(Object(StorageCell::new(Storage::SourceModule {
            name: Rc::clone(&name),
            filename,
        })));
        modules.insert(name, Rc::clone(&module));
        Rc::into_raw(module)
    })
}

/// Borrow the module and return an owning reference to its name or filename.
///
/// # Safety
/// `module` is a live native source module owned by the caller.
unsafe fn attribute(module: *const Object, file: bool) -> *const Str {
    // SAFETY: the ABI caller roots the module during this read.
    unsafe { object(module) }.0.with(|state| match state {
        Storage::SourceModule { name, filename } => {
            Rc::into_raw(Rc::clone(if file { filename } else { name }))
        }
        _ => super::fail("SystemError", "expected a native source module"),
    })
}

/// Read the import name from a live module.
///
/// # Safety
/// `module` is a rooted native source module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_source_module_name(module: *const Object) -> *const Str {
    // SAFETY: this entry point has the same rooted-module contract as attribute.
    unsafe { attribute(module, false) }
}

/// Read the original source filename from a live module.
///
/// # Safety
/// `module` is a rooted native source module.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_source_module_filename(module: *const Object) -> *const Str {
    // SAFETY: this entry point has the same rooted-module contract as attribute.
    unsafe { attribute(module, true) }
}
