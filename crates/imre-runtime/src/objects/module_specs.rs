//! Native path operations used by importlib.machinery.ModuleSpec properties.
use super::Str;

/// Return the prefix before the last ASCII dot, or Python's empty string.
///
/// # Safety
/// The exact Unicode input is rooted for this call; one string owner is returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_module_parent_prefix(name: *const Str) -> *const Str {
    // SAFETY: the compiler roots the name during the property read.
    let parent = unsafe { &*name }.rsplit_once('.').map_or("", |(p, _)| p);
    crate::StrOwner::into_raw(super::literal_string(parent))
}

/// Resolve bytecode paths and derive source caches using the Python command optimization.
///
/// # Safety
/// Zero denotes None. Other origin words and the optimization argument borrow rooted strings. The result
/// owns a new reference when nonzero, including when it preserves the origin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_module_cache_from_origin(
    origin: usize,
    optimization: *const Str,
) -> usize {
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
        super::finish(
            // SAFETY: the caller roots the optimization string during the property read.
            super::cache_paths::module_cache(path, unsafe { &*optimization })
                .map(|v| v.map_or(0, |s| crate::StrOwner::into_raw(s) as usize)),
            0,
        )
    } else {
        0
    }
}
