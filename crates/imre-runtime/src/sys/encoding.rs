//! Filesystem and standard stream settings captured before user code runs.
use crate::objects::Codec;
use std::borrow::Cow;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::sync::{Mutex, Once, OnceLock};

struct Locale {
    coerced_ctype: Option<OsString>,
    codeset: String,
    utf8_mode: bool,
}

impl Locale {
    fn default_text_encoding(&self) -> &str {
        if self.utf8_mode {
            "utf-8"
        } else {
            &self.codeset
        }
    }
}

pub(super) struct Settings {
    pub(super) codec: Codec,
    pub(super) errors: Cow<'static, str>,
    locale: Locale,
}

impl Settings {
    pub(super) fn filesystem_codeset(&self) -> &str {
        // Apple and Android always encode filesystem paths as UTF-8.
        if cfg!(any(target_vendor = "apple", target_os = "android")) {
            "utf-8"
        } else {
            self.locale.default_text_encoding()
        }
    }
}

/// Explicit `locale` asks for the locale even when UTF-8 mode overrides the file default.
pub(crate) fn file_encoding(name: Option<&str>) -> Cow<'_, str> {
    match name {
        Some("locale") => with_process_locale(current_codeset).into(),
        Some(name) => name.into(),
        None if settings(false, None).locale.utf8_mode => "utf-8".into(),
        None => with_process_locale(current_codeset).into(),
    }
}

/// Return the locale codeset even when UTF-8 mode overrides the default file encoding.
#[unsafe(no_mangle)]
pub extern "C" fn imre_locale_encoding() -> *const crate::Str {
    crate::StrOwner::into_raw(crate::StrOwner::new(crate::Str::new(&file_encoding(Some(
        "locale",
    )))))
}

/// Report the captured startup mode used by Python's text-file encoding default.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_utf8_mode() -> bool {
    settings(false, None).locale.utf8_mode
}

pub(super) fn settings(isolated: bool, utf8: Option<bool>) -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let value = if isolated {
            String::new()
        } else {
            match std::env::var("PYTHONIOENCODING") {
                Ok(setting) => setting,
                Err(std::env::VarError::NotPresent) => String::new(),
                Err(std::env::VarError::NotUnicode(_)) => {
                    fatal("PYTHONIOENCODING is not valid UTF-8")
                }
            }
        };
        let utf8 = utf8.or_else(|| {
            if isolated {
                return None;
            }
            match std::env::var("PYTHONUTF8").as_deref() {
                Ok("1") => Some(true),
                Ok("0") => Some(false),
                Ok("") | Err(_) => None,
                Ok(_) => fatal("PYTHONUTF8 must be 0 or 1"),
            }
        });
        let (locale, locale_errors) = default_encoding(isolated, utf8);
        let (name, errors) = value.split_once(':').unwrap_or((&value, ""));
        let (codec, default_errors) = if name.is_empty() {
            (lookup(locale.default_text_encoding()), locale_errors)
        } else {
            (lookup(name), "strict")
        };
        Settings {
            codec,
            locale,
            errors: if errors.is_empty() {
                default_errors.into()
            } else {
                errors.to_owned().into()
            },
        }
    })
}

/// Publish locale coercion only in native main, before the program starts workers.
pub(super) fn initialize(isolated: bool, utf8: Option<bool>) {
    static START: Once = Once::new();
    let settings = settings(isolated, utf8);
    START.call_once(|| {
        if let Some(name) = &settings.locale.coerced_ctype {
            // SAFETY: both interpreter entry points initialize before running user code
            // or starting threads. Do not mutate the environment from lazy settings reads.
            unsafe { std::env::set_var("LC_CTYPE", name) };
            // CPython reapplies the environment for all categories after coercion.
            with_process_locale(|| unsafe { libc::setlocale(libc::LC_ALL, c"".as_ptr()) });
        }
    });
}

fn lookup(name: &str) -> Codec {
    Codec::lookup(name).unwrap_or_else(|_| fatal(&format!("LookupError: unknown encoding: {name}")))
}

fn fatal(message: &str) -> ! {
    let _ = writeln!(io::stderr().lock(), "{message}");
    std::process::exit(1);
}

fn default_encoding(isolated: bool, utf8: Option<bool>) -> (Locale, &'static str) {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var_os(key).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "C".into());
    let charset = locale_codeset(&locale);
    // A rejected locale leaves a C program in its initial C locale.
    let c_locale = locale == "C" || charset.is_none();
    let legacy = c_locale || locale == "POSIX";
    let utf8_mode = utf8.unwrap_or(legacy);
    let coerce = (c_locale || (cfg!(target_os = "linux") && locale == "POSIX"))
        && std::env::var_os("LC_ALL").is_none_or(|s| s.is_empty())
        && (isolated || std::env::var("PYTHONCOERCECLOCALE").as_deref() != Ok("0"));
    let coerced = coerce.then(|| {
        ["C.UTF-8", "C.utf8", "UTF-8"]
            .into_iter()
            .find_map(|name| locale_codeset(name).map(|codeset| (OsString::from(name), codeset)))
    });
    let coerced = coerced.flatten();
    let coerced_ctype = coerced.as_ref().map(|(name, _)| name.clone());
    let (ctype_name, codeset) = coerced
        .or_else(|| charset.map(|codeset| (locale.clone(), codeset)))
        .unwrap_or_else(|| {
            (
                "C".into(),
                locale_codeset("C").unwrap_or_else(|| "utf-8".to_owned()),
            )
        });
    // Python starts in the environment's character locale. Other categories
    // retain the C library defaults until the program changes them.
    if let Ok(name) = CString::new(ctype_name.as_bytes()) {
        locked_locale(|| {
            // SAFETY: the name was accepted by newlocale; locale changes hold the lock.
            unsafe { libc::setlocale(libc::LC_CTYPE, name.as_ptr()) };
        });
    }
    let errors =
        if utf8_mode || legacy || matches!(locale.to_str(), Some("C.UTF-8" | "C.utf8" | "UTF-8")) {
            "surrogateescape"
        } else {
            "strict"
        };
    (
        Locale {
            coerced_ctype,
            codeset,
            utf8_mode,
        },
        errors,
    )
}

/// Read the selected character set without changing the process locale.
fn locale_codeset(name: impl AsRef<OsStr>) -> Option<String> {
    with_ctype_locale(name, || {
        // SAFETY: nl_langinfo returns the active locale's NUL-terminated codeset name.
        let ptr = unsafe { libc::nl_langinfo(libc::CODESET) };
        (!ptr.is_null())
            .then(|| {
                // SAFETY: the non-null codeset name is valid for this active locale.
                unsafe { CStr::from_ptr(ptr) }
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            })
            .flatten()
            .filter(|name| !name.is_empty())
    })
    .flatten()
}

fn locked_locale<T>(operation: impl FnOnce() -> T) -> T {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    operation()
}

/// Serialize process-locale queries and consumers with locale changes.
pub(crate) fn with_process_locale<T>(operation: impl FnOnce() -> T) -> T {
    settings(false, None);
    locked_locale(operation)
}

fn current_codeset() -> String {
    // SAFETY: the caller holds the process-locale lock while copying libc's storage.
    let name = unsafe { libc::nl_langinfo(libc::CODESET) };
    if name.is_null() {
        return "UTF-8".to_owned();
    }
    // SAFETY: the non-null NUL-terminated encoding name remains valid under the lock.
    let name = unsafe { CStr::from_ptr(name) };
    if name.to_bytes().is_empty() {
        "UTF-8".to_owned()
    } else {
        name.to_string_lossy().into_owned()
    }
}

/// Decode a C string using the active character locale, while holding the locale lock.
fn decode_locale(value: &CStr) -> crate::Result<String> {
    unsafe extern "C" {
        fn imre_decode_locale_name(
            name: *const libc::c_char,
            length: usize,
            points: *mut u32,
            count: *mut usize,
        ) -> usize;
    }
    let bytes = value.to_bytes();
    let mut points = vec![0; bytes.len()];
    let mut count = 0;
    // SAFETY: name is NUL-terminated; each decoded scalar uses at least one input
    // byte. The C helper owns its mbstate_t and validates scalars under the locale lock.
    let start = unsafe {
        imre_decode_locale_name(value.as_ptr(), bytes.len(), points.as_mut_ptr(), &mut count)
    };
    if start == bytes.len() {
        return Ok(points[..count]
            .iter()
            .map(|&point| {
                char::from_u32(point).expect("the locale decoder emits Unicode scalar values")
            })
            .collect());
    }
    let byte = bytes[start];
    Err(crate::Error::unicode_decode(
        "locale",
        crate::BytesOwner::new(bytes.to_vec()),
        start..start + 1,
        "decoding error",
        format!(
            "'locale' codec can't decode byte 0x{byte:02x} in position {start}: decoding error"
        ),
    ))
}

/// Copy and decode the selected category before another change can invalidate its name.
pub(crate) fn set_locale(
    category: libc::c_int,
    name: Option<&CStr>,
) -> Option<crate::Result<String>> {
    with_process_locale(|| {
        // SAFETY: a null name queries the category, otherwise the CString is live.
        let value =
            unsafe { libc::setlocale(category, name.map_or(std::ptr::null(), CStr::as_ptr)) };
        if value.is_null() {
            return None;
        }
        // SAFETY: a successful setlocale returns a NUL-terminated name.
        let value = unsafe { CStr::from_ptr(value) };
        Some(decode_locale(value))
    })
}

/// Use the selected character locale on this thread and restore the previous locale.
fn with_ctype_locale<T>(name: impl AsRef<OsStr>, op: impl FnOnce() -> T) -> Option<T> {
    let name = CString::new(name.as_ref().as_bytes()).ok()?;
    // SAFETY: the locale handle is owned here. Only this thread selects it, the
    // callback runs while it is live, and the previous locale is restored.
    unsafe {
        let local = libc::newlocale(libc::LC_CTYPE_MASK, name.as_ptr(), std::ptr::null_mut());
        if local.is_null() {
            return None;
        }
        let previous = libc::uselocale(local);
        if previous.is_null() {
            libc::freelocale(local);
            return None;
        }
        let result = op();
        libc::uselocale(previous);
        libc::freelocale(local);
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_locale, with_ctype_locale};

    #[test]
    fn locale_decoder_reports_the_first_invalid_character() {
        let check = || {
            for (name, start) in [
                (c"abc\xf4\x90\x80\x80abc", 3),
                (c"a\xf4\x90\x80\x80\xff", 1),
                (c"xx\xe2\x82", 2),
            ] {
                let error = decode_locale(name).unwrap_err();
                let args = error.decode_args().expect("UnicodeDecodeError");
                assert_eq!(args.encoding, "locale");
                assert_eq!(args.object, name.to_bytes());
                assert_eq!((args.start, args.end), (start, start + 1));
            }
        };
        // C is required by POSIX. Where installed, also exercise a multibyte locale.
        with_ctype_locale("C", check).expect("C character locale");
        let _ = ["C.UTF-8", "C.utf8", "UTF-8"]
            .into_iter()
            .find_map(|name| with_ctype_locale(name, check));
    }
}
