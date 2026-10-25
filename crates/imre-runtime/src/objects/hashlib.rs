//! SHA-256 handles share a mutable hash state; digest reads finalize a snapshot.

use crate::StrOwner;
use std::cell::RefCell;

use sha2::digest::Output;
use sha2::{Digest, Sha256};

use super::{Bytes, Object, Storage, StorageCell, Str, object};

/// Return a hash for a runtime name; aliases select the same SHA-256 provider.
///
/// # Safety
/// The caller roots the exact str and bytes until the constructor returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_new(name: *const Str, data: *const Bytes) -> *const Object {
    use super::exceptions::{Error, Kind, finish};
    // SAFETY: both operands remain owned for this operation.
    let (name, data) = unsafe { (&*name, &*data) };
    let known = ["sha256", "sha-256", "sha2-256", "2.16.840.1.101.3.4.2.1"]
        .iter()
        .any(|alias| name.eq_ignore_ascii_case(alias));
    let result = if known {
        Ok(handle(Sha256::new_with_prefix(data)))
    } else {
        Err(Error::new(
            Kind::ValueError,
            format!("unsupported hash type {name}"),
        ))
    };
    finish(result, std::ptr::null())
}

thread_local! { static AVAILABLE: RefCell<Option<crate::ObjectOwner>> = const { RefCell::new(None) }; }

/// The public algorithms set has a stable identity; changing it does not enable a provider.
#[unsafe(no_mangle)]
pub extern "C" fn imre_hash_algorithms_available() -> *const Object {
    use super::mappings::{imre_set_add, imre_set_new};
    AVAILABLE.with_borrow_mut(|slot| {
        let set = slot.get_or_insert_with(|| {
            // SAFETY: the fresh string-keyed set transfers one reference to this slot.
            let set = unsafe { crate::ObjectOwner::from_raw(imre_set_new(3)) };
            let name = StrOwner::new(Str::from("sha256"));
            // SAFETY: both exact inputs are rooted; the set acquires its own key.
            unsafe {
                imre_set_add(
                    crate::ObjectOwner::as_ptr(&set),
                    StrOwner::as_ptr(&name) as usize,
                )
            };
            set
        });
        crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(set))
    })
}

/// Drop the process's module root so another program starts with a fresh set.
pub(super) fn clear_algorithms() {
    AVAILABLE.with(RefCell::take);
}

fn handle(state: Sha256) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Sha256(Box::new(state)),
    ))))
}

#[inline]
fn hex(digest: Output<Sha256>) -> *const Str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = Str::with_capacity(digest.len() * 2);
    for byte in digest {
        result.push(char::from(HEX[(byte >> 4) as usize]));
        result.push(char::from(HEX[(byte & 15) as usize]));
    }
    StrOwner::into_raw(StrOwner::new(result))
}

/// # Safety
/// The caller roots an exact bytes handle while the one-shot digest reads its contents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_hex(value: *const Bytes) -> *const Str {
    // SAFETY: the compiler roots the immutable bytes until hashing returns.
    hex(Sha256::digest(unsafe { &*value }))
}

/// # Safety
/// The caller roots the initial bytes. The new hash owns its state independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_new(value: *const Bytes) -> *const Object {
    // SAFETY: SHA-256 consumes the borrowed bytes before returning.
    handle(Sha256::new_with_prefix(unsafe { &*value }))
}

/// # Safety
/// Both typed handles are rooted. The state update does not invoke Python code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_update(hash: *const Object, value: *const Bytes) {
    // SAFETY: the exclusive state access cannot reenter, and no input reference is stored.
    unsafe {
        object(hash).0.with_mut(|storage| {
            let Storage::Sha256(state) = storage else {
                unreachable!()
            };
            state.update(&*value);
        });
    }
}

/// # Safety
/// The caller roots an exact SHA-256 object; only the copied state escapes the borrow.
unsafe fn snapshot(hash: *const Object) -> Sha256 {
    // SAFETY: cloning SHA-256's fixed buffer cannot reenter or modify the object.
    unsafe {
        object(hash).0.with(|storage| {
            let Storage::Sha256(state) = storage else {
                unreachable!()
            };
            (**state).clone()
        })
    }
}

/// # Safety
/// The caller roots the hash. Its independent copy can be updated separately.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_copy(hash: *const Object) -> *const Object {
    // SAFETY: the rooted hash supplies an exact snapshot.
    handle(unsafe { snapshot(hash) })
}

/// # Safety
/// The caller roots the hash. The returned 32 bytes own their storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_digest(hash: *const Object) -> *const Bytes {
    // SAFETY: only the copied state is consumed by finalize.
    let digest = unsafe { snapshot(hash) }.finalize();
    crate::BytesOwner::into_raw(crate::BytesOwner::new(digest.to_vec()))
}

/// # Safety
/// The caller roots the hash. The lowercase hexadecimal result owns its storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_hexdigest(hash: *const Object) -> *const Str {
    // SAFETY: finalize consumes only a snapshot of the rooted hash.
    hex(unsafe { snapshot(hash) }.finalize())
}
