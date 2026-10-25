//! SimpleNamespace owns a real insertion-ordered Python dictionary. Exposing
//! __dict__ returns that same object, which can outlive or mutate its namespace.
use super::{Error, Kind, Object, Storage, StorageCell, Str, mappings, object};

unsafe fn dictionary(namespace: *const Object) -> crate::ObjectOwner {
    // SAFETY: the ABI requires a rooted namespace; clone its owner before leaving storage.
    unsafe {
        object(namespace).0.with(|state| match state {
            Storage::SimpleNamespace(dict) => crate::ObjectOwner::clone(dict),
            _ => super::fail("SystemError", "expected a SimpleNamespace"),
        })
    }
}

fn absent(key: &Str) -> Error {
    Error::new(
        Kind::AttributeError,
        format!(
            "'types.SimpleNamespace' object has no attribute '{}'",
            key.as_str()
        ),
    )
}

/// # Safety
/// The caller roots a newly created string-keyed dictionary of its declared layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_namespace_wrap(dict: *const Object) -> *const Object {
    // SAFETY: the wrapper acquires a separate owner, as required by the ABI.
    let dict = unsafe { crate::ObjectOwner::acquire_word(dict as usize) };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::SimpleNamespace(dict),
    ))))
}

/// # Safety
/// The caller roots a namespace; the result transfers an independent dictionary owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_namespace_dict(namespace: *const Object) -> *const Object {
    // SAFETY: the caller keeps namespace live throughout the projection.
    crate::ObjectOwner::into_raw(unsafe { dictionary(namespace) })
}

/// # Safety
/// Both inputs have their exact live types; a successful read transfers the value owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_namespace_get(namespace: *const Object, key: *const Str) -> usize {
    // SAFETY: the dictionary clone and caller's string root the complete lookup.
    let value = unsafe {
        mappings::lookup(
            crate::ObjectOwner::as_ptr(&dictionary(namespace)),
            key as usize,
        )
    };
    match value {
        Some(value) => value.into_word(),
        // SAFETY: the caller keeps the original attribute name live.
        None => unsafe { absent(&*key) }.returning(0),
    }
}

/// # Safety
/// The namespace and immutable string key are rooted throughout this nonreentrant operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_namespace_delete(namespace: *const Object, key: *const Str) {
    // SAFETY: deletion from a string-keyed native dictionary cannot call user code.
    unsafe {
        let dict = dictionary(namespace);
        let ptr = crate::ObjectOwner::as_ptr(&dict);
        if mappings::lookup(ptr, key as usize).is_some() {
            mappings::imre_dict_delete(ptr, key as usize);
        } else {
            absent(&*key).raise();
        }
    }
}
