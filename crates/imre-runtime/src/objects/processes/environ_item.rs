//! Typed subscripts preserve the key object exposed by a missing environment entry.
use super::*;
use std::borrow::Cow;

fn encoded_key(key: &Str) -> Result<Vec<u8>> {
    super::super::Codec::encode_str(key, &crate::sys::filesystem_encoding()?, "surrogateescape")
}

/// # Safety
/// The caller roots an exact bytes or str key with the indicated representation.
unsafe fn missing(key: usize, bytes: bool, encoded: Cow<'_, [u8]>) {
    let kind = WordKind::new(if bytes { 8 } else { 3 });
    // SAFETY: the caller roots the original key of the exact declared kind.
    let original = unsafe { kind.acquire(key) };
    let encoded = if bytes {
        original.clone()
    } else {
        let copy = crate::BytesOwner::into_raw(crate::BytesOwner::new(encoded.into_owned()));
        // SAFETY: the new bytes allocation transfers its one owned reference.
        unsafe { OwnedWord::from_owned(copy as usize, WordKind::new(8)) }
    };
    super::super::exceptions::raise_key_from_none(original, encoded);
}

/// Look up a text key using the codec selected at interpreter startup.
///
/// # Safety
/// The caller roots the key through this call and owns a non-null returned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_index(key: *const Str) -> *const Str {
    // SAFETY: the caller roots an exact string key.
    let encoded = match encoded_key(unsafe { &*key }) {
        Ok(encoded) => encoded,
        Err(error) => return error.returning(std::ptr::null()),
    };
    let value = with_environment_value(&encoded, |value| {
        let codec = super::super::Codec::lookup(&crate::sys::filesystem_encoding()?)?;
        codec
            .decode(value, Some("surrogateescape"))
            .map(|text| StrOwner::into_raw(StrOwner::new(Str::from(text))))
    });
    match value {
        Some(value) => finish(value, std::ptr::null()),
        None => {
            // SAFETY: the exact key remains rooted through the missing-key translation.
            unsafe { missing(key as usize, false, Cow::Owned(encoded)) };
            std::ptr::null()
        }
    }
}

/// Remove one native environment entry; unsetenv itself accepts absent names.
///
/// # Safety
/// The caller roots one exact bytes (nonzero flag) or str (zero flag) until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_environ_delete(key: *const std::ffi::c_void, bytes: u32) {
    let remove = || {
        // SAFETY: the ABI flag matches the caller's rooted key representation.
        let encoded = unsafe {
            if bytes != 0 {
                Cow::Borrowed((&*key.cast::<Bytes>()).as_slice())
            } else {
                Cow::Owned(encoded_key(&*key.cast::<Str>())?)
            }
        };
        let name = cstring(&encoded)?;
        let guard = ENVIRONMENT.lock().unwrap();
        // SAFETY: the guard keeps the environment stable until the search completes.
        let present = unsafe { environment_entries(native_environ()) }.any(|(k, _)| k == &*encoded);
        change_environment_locked(&name, None, &guard)?;
        drop(guard);
        if !present {
            // SAFETY: the key remains rooted even after releasing the environment guard.
            unsafe { missing(key as usize, bytes != 0, encoded) };
        }
        Ok(())
    };
    finish(remove(), ());
}
