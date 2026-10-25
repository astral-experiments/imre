//! Native path operations used by importlib.machinery.ModuleSpec properties.
use super::Str;
use std::rc::Rc;

/// Return the prefix before the last ASCII dot, or Python's empty string.
///
/// # Safety
/// The exact Unicode input is rooted for this call; one string owner is returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_module_parent_prefix(name: *const Str) -> *const Str {
    // SAFETY: the compiler roots the name during the property read.
    let parent = unsafe { &*name }.rsplit_once('.').map_or("", |(p, _)| p);
    Rc::into_raw(super::literal_string(parent))
}

/// Resolve the directly usable bytecode path. A native interpreter which has
/// not exposed cache tags must not invent an ABI tag for source bytecode.
///
/// # Safety
/// Zero denotes None. Other words borrow a rooted string. The returned word
/// owns a new reference when nonzero, including when it preserves the origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_module_cache_from_origin(origin: usize) -> usize {
    if origin == 0 {
        return 0;
    }
    // SAFETY: a non-null optional str stores its rooted pointer as a word.
    let path = unsafe { &*(origin as *const Str) };
    if path.ends_with(".pyc") {
        // SAFETY: the result transfers a new owner of the same immutable origin.
        unsafe { crate::imre_str_retain(origin as *const Str) };
        origin
    } else if path.ends_with(".py") || cfg!(windows) && path.ends_with(".pyw") {
        super::exceptions::Error::new(
            super::exceptions::Kind::NotImplementedError,
            "source cache paths require a bytecode cache tag",
        )
        .returning(0)
    } else {
        0
    }
}
