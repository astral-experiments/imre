//! A closed import graph has one live module per import name. Holding the registry
//! reference also keeps a module alive after the last local alias is discarded.

use super::{Object, Storage, StorageCell, Str, object};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static MODULES: RefCell<HashMap<crate::StrOwner, crate::ObjectOwner>> = RefCell::default();
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
            return crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(module));
        }
        // SAFETY: retain independent references to the two rooted strings.
        let (name, filename) = unsafe {
            (
                crate::StrOwner::acquire_word(name as usize),
                crate::StrOwner::acquire_word(filename as usize),
            )
        };
        let module = crate::ObjectOwner::new(Object(StorageCell::new(Storage::SourceModule {
            name: crate::StrOwner::clone(&name),
            filename,
        })));
        modules.insert(name, crate::ObjectOwner::clone(&module));
        crate::ObjectOwner::into_raw(module)
    })
}

/// Borrow the module and return an owning reference to its name or filename.
///
/// # Safety
/// `module` is a live native source module owned by the caller.
unsafe fn attribute(module: *const Object, file: bool) -> *const Str {
    // SAFETY: the caller roots the module; cloning an immutable string cannot
    // reenter the module or mutate its storage during this borrow.
    unsafe {
        object(module).0.with(|state| match state {
            Storage::SourceModule { name, filename } => {
                crate::StrOwner::into_raw(crate::StrOwner::clone(if file {
                    filename
                } else {
                    name
                }))
            }
            _ => super::fail("SystemError", "expected a native source module"),
        })
    }
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
