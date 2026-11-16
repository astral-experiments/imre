//! Stable owners for the Unicode and filesystem configuration of a native process.
use super::{Codec, canonical, encoding};
use crate::objects::exceptions::{Error, Kind};
use crate::{Str, StrOwner};

pub(crate) const DEFAULT_ENCODING: &str = "utf-8";
// The native filesystem uses POSIX path and descriptor APIs.
pub(crate) const FILESYSTEM_ERRORS: &str = "surrogateescape";

struct Names {
    unicode: StrOwner,
    filesystem: Option<StrOwner>,
    errors: StrOwner,
}

thread_local! {
    static NAMES: Names = {
        let settings = encoding::settings(false, None);
        Names {
            unicode: canonical(DEFAULT_ENCODING),
            filesystem: settings.filesystem_codec().map(|codec| canonical(codec.name())),
            errors: canonical(FILESYSTEM_ERRORS),
        }
    };
}

/// Install Python's retained configuration strings before any user literal or intern call.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_init_encoding_names() {
    NAMES.with(|_| ());
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_default_encoding() -> *const Str {
    NAMES.with(|names| StrOwner::into_raw(names.unicode.clone()))
}

fn unknown_filesystem_codec() -> Error {
    Error::new(
        Kind::LookupError,
        format!(
            "unknown encoding: {}",
            encoding::settings(false, None).filesystem_codeset()
        ),
    )
}

/// Native path operations use the codec chosen at startup without acquiring a Python string.
pub(crate) fn filesystem_codec() -> Result<Codec, Error> {
    encoding::settings(false, None)
        .filesystem_codec()
        .ok_or_else(unknown_filesystem_codec)
}

/// Transfer a reference to the canonical codec name. A locale outside the native
/// registry raises LookupError even when an explicit stdio codec allowed startup.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_filesystem_encoding() -> *const Str {
    match NAMES.with(|names| names.filesystem.clone()) {
        Some(name) => StrOwner::into_raw(name),
        None => unknown_filesystem_codec().returning(std::ptr::null()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_filesystem_errors() -> *const Str {
    NAMES.with(|names| StrOwner::into_raw(names.errors.clone()))
}
