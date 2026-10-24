//! SHA-256 handles share a mutable hash state; digest reads finalize a snapshot.

use std::rc::Rc;

use sha2::digest::Output;
use sha2::{Digest, Sha256};

use super::{Bytes, Object, Storage, StorageCell, Str, object};

fn handle(state: Sha256) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Sha256(
        Box::new(state),
    )))))
}

#[inline]
fn hex(digest: Output<Sha256>) -> *const Str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = Str::with_capacity(digest.len() * 2);
    for byte in digest {
        result.push(char::from(HEX[(byte >> 4) as usize]));
        result.push(char::from(HEX[(byte & 15) as usize]));
    }
    Rc::into_raw(Rc::new(result))
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
    Rc::into_raw(Rc::new(digest.to_vec()))
}

/// # Safety
/// The caller roots the hash. The lowercase hexadecimal result owns its storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_hexdigest(hash: *const Object) -> *const Str {
    // SAFETY: finalize consumes only a snapshot of the rooted hash.
    hex(unsafe { snapshot(hash) }.finalize())
}
