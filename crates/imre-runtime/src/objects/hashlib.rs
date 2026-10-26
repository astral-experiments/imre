//! Hash objects share mutable state. Digest reads finalize an independent snapshot.

use std::cell::RefCell;
use std::ptr;

use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::exceptions::{Error, Kind, Result, finish};
use super::{Bytes, Object, Storage, StorageCell, Str, object};
use crate::{BytesOwner, Int, ObjectOwner};

#[derive(Clone)]
pub(crate) enum State {
    Sha1(Sha1),
    Sha256(Sha256),
}

impl State {
    pub(crate) fn sha1(data: &[u8]) -> Self {
        Self::Sha1(Sha1::new_with_prefix(data))
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Sha1(_) => "sha1",
            Self::Sha256(_) => "sha256",
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha1(h) => h.update(data),
            Self::Sha256(h) => h.update(data),
        }
    }

    pub(crate) fn digest_size(&self) -> usize {
        match self {
            Self::Sha1(_) => Sha1::output_size(),
            Self::Sha256(_) => Sha256::output_size(),
        }
    }

    pub(crate) fn digest<T>(&self, result: impl FnOnce(&[u8]) -> T) -> T {
        match self {
            Self::Sha1(h) => result(&h.clone().finalize()),
            Self::Sha256(h) => result(&h.clone().finalize()),
        }
    }

    pub(crate) fn hexdigest(&self) -> Result<Str> {
        self.digest(hex_text)
    }
}

fn handle(state: State) -> Result<*const Object> {
    let state = allocator_api2::boxed::Box::try_new(state).map_err(|_| Error::allocation())?;
    ObjectOwner::try_new(Object(StorageCell::new(Storage::Hash(state))))
        .map(ObjectOwner::into_raw)
        .map_err(|_| Error::allocation())
}

/// # Safety
/// The caller roots the exact str and bytes until the constructor returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_new(name: *const Str, data: *const Bytes) -> *const Object {
    // SAFETY: both operands remain owned for this operation.
    let (name, data) = unsafe { (&*name, &*data) };
    let matches = |aliases: &[&str]| aliases.iter().any(|alias| name.eq_ignore_ascii_case(alias));
    let result = if matches(&["sha1", "sha-1", "ssl3-sha1", "1.3.14.3.2.26"]) {
        handle(State::sha1(data))
    } else if matches(&["sha256", "sha-256", "sha2-256", "2.16.840.1.101.3.4.2.1"]) {
        handle(State::Sha256(Sha256::new_with_prefix(data)))
    } else {
        Err(Error::new(
            Kind::ValueError,
            format!("unsupported hash type {name}"),
        ))
    };
    finish(result, ptr::null())
}

thread_local! { static AVAILABLE: RefCell<Option<ObjectOwner>> = const { RefCell::new(None) }; }

/// The public algorithms set has a stable identity. Changing it does not enable a provider.
#[unsafe(no_mangle)]
pub extern "C" fn imre_hash_algorithms_available() -> *const Object {
    let result = AVAILABLE.with_borrow_mut(|slot| {
        let set = match slot {
            Some(set) => set,
            None => slot.insert(super::mappings::Set::names(
                ["sha1", "sha256"].into_iter(),
                false,
            )?),
        };
        Ok(ObjectOwner::into_raw(ObjectOwner::clone(set)))
    });
    finish(result, ptr::null())
}

pub(super) fn clear_algorithms() {
    AVAILABLE.with(RefCell::take);
}

fn hex_text(digest: &[u8]) -> Result<Str> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = Str::try_with_capacity(digest.len() * 2).map_err(|_| Error::allocation())?;
    for &byte in digest {
        result.push(char::from(HEX[(byte >> 4) as usize]));
        result.push(char::from(HEX[(byte & 15) as usize]));
    }
    Ok(result)
}

fn hex(digest: &[u8]) -> Result<*const Str> {
    crate::checked_owned_str(hex_text(digest)?)
}

/// # Safety
/// The caller roots the immutable bytes until the one-shot digest completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha1_hex(value: *const Bytes) -> *const Str {
    // SAFETY: the compiler roots the exact bytes until hashing returns.
    finish(hex(&Sha1::digest(unsafe { &*value })), ptr::null())
}

/// # Safety
/// The caller roots the immutable bytes until the one-shot digest completes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_hex(value: *const Bytes) -> *const Str {
    // SAFETY: the compiler roots the exact bytes until hashing returns.
    finish(hex(&Sha256::digest(unsafe { &*value })), ptr::null())
}

/// # Safety
/// The caller roots the initial bytes. The new hash owns its state independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha1_new(value: *const Bytes) -> *const Object {
    // SAFETY: the provider consumes the borrowed bytes before returning.
    finish(handle(State::sha1(unsafe { &*value })), ptr::null())
}

/// # Safety
/// The caller roots the initial bytes. The new hash owns its state independently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sha256_new(value: *const Bytes) -> *const Object {
    // SAFETY: the provider consumes the borrowed bytes before returning.
    finish(
        handle(State::Sha256(Sha256::new_with_prefix(unsafe { &*value }))),
        ptr::null(),
    )
}

/// # Safety
/// Both typed handles are rooted. The state update does not invoke Python code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_update(hash: *const Object, value: *const Bytes) {
    // SAFETY: the exclusive state access cannot reenter, and no input reference is stored.
    unsafe {
        object(hash).0.with_mut(|storage| {
            let Storage::Hash(state) = storage else {
                unreachable!()
            };
            state.update(&*value);
        });
    }
}

/// # Safety
/// The caller roots an exact hash object and the operation cannot invoke Python code.
unsafe fn with_state<T>(hash: *const Object, f: impl FnOnce(&State) -> T) -> T {
    // SAFETY: the rooted hash remains immutably borrowed for the callback.
    unsafe {
        object(hash).0.with(|storage| {
            let Storage::Hash(state) = storage else {
                unreachable!()
            };
            f(state)
        })
    }
}

/// # Safety
/// The compiler roots the hash until its provider name has been copied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_name(hash: *const Object) -> *const Str {
    // SAFETY: provider names are static strings independent of the hash.
    let name = unsafe { with_state(hash, State::name) };
    finish(crate::checked_owned_str(Str::from(name)), ptr::null())
}

/// # Safety
/// The compiler roots the exact hash during the metadata lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_digest_size(hash: *const Object) -> usize {
    // SAFETY: the lookup only reads the hash's algorithm tag.
    let size = unsafe { with_state(hash, State::digest_size) };
    Int::from_usize(size).into_raw()
}

/// # Safety
/// The caller roots the hash. Its independent copy can be updated separately.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_copy(hash: *const Object) -> *const Object {
    // SAFETY: clone copies the provider's fixed-size buffers.
    finish(
        handle(unsafe { with_state(hash, State::clone) }),
        ptr::null(),
    )
}

/// # Safety
/// The caller roots the hash. The returned digest owns its bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_digest(hash: *const Object) -> *const Bytes {
    // SAFETY: finalize reads a snapshot of the rooted hash.
    let digest = unsafe { with_state(hash, |s| s.digest(super::bytes::owned_input)) };
    finish(digest.map(BytesOwner::into_raw), ptr::null())
}

/// # Safety
/// The caller roots the hash. The lowercase hexadecimal result owns its storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_hash_hexdigest(hash: *const Object) -> *const Str {
    // SAFETY: finalize reads a snapshot of the rooted hash.
    let result = unsafe { with_state(hash, State::hexdigest) };
    finish(result.and_then(crate::checked_owned_str), ptr::null())
}
