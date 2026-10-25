//! Public decoder instances own their visible memo; each scan owns its key namespace.

use super::readonly;
use crate::finish;
use crate::objects::mappings::imre_dict_new;
use crate::objects::{Object, Storage, StorageCell, Str, object};

/// Construct a default decoder with a fresh Python-visible memo dictionary.
#[unsafe(no_mangle)]
pub extern "C" fn imre_json_decoder_new() -> *const Object {
    // SAFETY: a fresh string/string dictionary transfers its sole owner to this instance.
    let memo = unsafe { crate::ObjectOwner::from_raw(imre_dict_new(3, 3)) };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::JsonDecoder(memo),
    ))))
}

/// # Safety
/// The caller roots the exact decoder; the returned dictionary has its own owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_decoder_memo(decoder: *const Object) -> *const Object {
    // SAFETY: the storage borrow stays inside this closure.
    unsafe {
        object(decoder).0.with(|storage| {
            let Storage::JsonDecoder(memo) = storage else {
                unreachable!("expected JSONDecoder")
            };
            crate::ObjectOwner::into_raw(memo.clone())
        })
    }
}

/// # Safety
/// The caller roots the decoder and Unicode input while parsing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_json_decoder_decode(
    _decoder: *const Object,
    text: *const Str,
) -> *const Object {
    // The default decoder interns keys within each scan. A caller can retain
    // or mutate its public memo without changing the parser's key owners.
    // JSONDecoder.decode treats U+FEFF as an ordinary invalid token. Only the
    // module-level loads entry checks for it before scanning a Unicode input.
    // SAFETY: the caller roots the text; only an exception needs another owner.
    let result = unsafe {
        readonly::loads(&*text, || {
            Some(crate::StrOwner::acquire_word(text as usize))
        })
    };
    finish(result, std::ptr::null())
}
