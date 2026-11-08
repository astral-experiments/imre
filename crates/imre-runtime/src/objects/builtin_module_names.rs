//! Names owned by this executable's built-in importer.
use std::cell::OnceCell;

use super::{Error, Kind, OwnedWord, records::tuples::tuple_from_words, words::WordKind};
use crate::{Str, StrOwner};

pub(crate) use imre_ast::providers::is_native_package;

/// Apply the finder's ownership rules before optional providers open a host library.
pub(crate) fn owns(
    name: &str,
    is_frozen: impl FnOnce() -> bool,
    has_source: impl FnOnce() -> bool,
) -> bool {
    imre_ast::providers::builtin_finder_provider(name, is_frozen, has_source)
        .is_some_and(crate::dynamic::provider_available)
}

pub(crate) fn names(mut included: impl FnMut(&str) -> bool) -> Result<Vec<&'static str>, Error> {
    let mut names = Vec::new();
    for name in imre_ast::providers::builtin_module_candidates() {
        if included(name) {
            names.try_reserve(1).map_err(|_| Error::allocation())?;
            names.push(name);
        }
    }
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

thread_local! {
    static TYPED_NAMES: OnceCell<OwnedWord> = const { OnceCell::new() };
}

fn typed_names(sources: &str, frozen_modules: bool) -> Result<OwnedWord, Error> {
    let names = names(|name| {
        owns(
            name,
            || imre_ast::providers::is_frozen(name, || frozen_modules),
            || sources.split('\0').any(|source| source == name),
        )
    })?;
    let mut words = Vec::new();
    words
        .try_reserve_exact(names.len())
        .map_err(|_| Error::allocation())?;
    for name in names {
        let text = Str::try_new(name).map_err(|_| Error::allocation())?;
        let text = StrOwner::try_new(text).map_err(|_| Error::allocation())?;
        // SAFETY: the field takes the single string reference just allocated.
        words.push(unsafe {
            OwnedWord::from_owned(StrOwner::into_raw(text) as usize, WordKind::new(3))
        });
    }
    tuple_from_words(words.into_iter())
}

/// Transfer a reference to the typed executable's tuple of genuine built-in names.
///
/// # Safety
/// The compiler supplies `length` bytes of NUL-separated UTF-8 module names from
/// the same immutable executable manifest on every call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_builtin_module_names(
    manifest: *const u8,
    length: usize,
    frozen_modules: bool,
) -> *const usize {
    let result: Result<OwnedWord, Error> = TYPED_NAMES.with(|cache| {
        if let Some(names) = cache.get() {
            return Ok(names.clone());
        }
        let bytes = if length == 0 {
            &[]
        } else {
            // SAFETY: the executable owns all manifest bytes for this call.
            unsafe { std::slice::from_raw_parts(manifest, length) }
        };
        let sources = std::str::from_utf8(bytes).map_err(|_| {
            Error::new(Kind::SystemError, "invalid built-in module source manifest")
        })?;
        let names = typed_names(sources, frozen_modules)?;
        Ok(cache.get_or_init(|| names).clone())
    });
    match result {
        Ok(names) => names.into_word() as *const usize,
        Err(error) => error.returning(std::ptr::null()),
    }
}
