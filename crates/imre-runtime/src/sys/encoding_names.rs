//! Stable owners for the Unicode and filesystem configuration of a native process.
use super::{Codec, canonical, encoding};
use crate::objects::exceptions::{Error, Kind};
use crate::{Str, StrOwner};

struct Names {
    unicode: StrOwner,
    filesystem: Option<StrOwner>,
    errors: StrOwner,
}

thread_local! {
    static NAMES: Names = {
        let settings = encoding::settings(false, None);
        Names {
            unicode: canonical("utf-8"),
            filesystem: Codec::lookup(settings.filesystem_codeset()).ok().map(|codec| canonical(codec.name())),
            // Imre's native filesystem runtime uses POSIX path and descriptor APIs.
            errors: canonical("surrogateescape"),
        }
    };
}

/// Install Python's retained configuration strings before any user literal or intern call.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_init_encoding_names() {
    NAMES.with(|_| ());
}

pub(crate) fn default_encoding() -> StrOwner {
    NAMES.with(|names| names.unicode.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_default_encoding() -> *const Str {
    StrOwner::into_raw(default_encoding())
}

/// Transfer a reference to the canonical codec name. A locale outside the native
/// registry raises LookupError even when an explicit stdio codec allowed startup.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_filesystem_encoding() -> *const Str {
    NAMES.with(|names| match &names.filesystem {
        Some(name) => StrOwner::into_raw(name.clone()),
        None => Error::new(
            Kind::LookupError,
            format!(
                "unknown encoding: {}",
                encoding::settings(false, None).filesystem_codeset()
            ),
        )
        .returning(std::ptr::null()),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_filesystem_errors() -> *const Str {
    NAMES.with(|names| StrOwner::into_raw(names.errors.clone()))
}
