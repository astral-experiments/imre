//! Standard stream settings captured once before user code can mutate the environment.
use crate::objects::Codec;
use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::sync::OnceLock;

pub(super) struct Settings {
    pub(super) codec: Codec,
    pub(super) errors: Cow<'static, str>,
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
        let (name, errors) = value.split_once(':').unwrap_or((&value, ""));
        if !name.is_empty() {
            return Settings {
                codec: lookup(name),
                errors: if errors.is_empty() {
                    "strict".into()
                } else {
                    errors.to_owned().into()
                },
            };
        }
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
        let (codec, default_errors) = default_encoding(isolated, utf8);
        Settings {
            codec,
            errors: if errors.is_empty() {
                default_errors.into()
            } else {
                errors.to_owned().into()
            },
        }
    })
}

fn lookup(name: &str) -> Codec {
    Codec::lookup(name).unwrap_or_else(|_| fatal(&format!("LookupError: unknown encoding: {name}")))
}

fn fatal(message: &str) -> ! {
    let _ = writeln!(io::stderr().lock(), "{message}");
    std::process::exit(1);
}

fn default_encoding(isolated: bool, utf8: Option<bool>) -> (Codec, &'static str) {
    let utf8_codec = Codec::Utf8 { signature: false };
    if utf8 == Some(true) {
        return (utf8_codec, "surrogateescape");
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "C".to_owned());
    let (locale, codeset) = match locale_codeset(&locale) {
        Some(codeset) => (locale, codeset),
        None => ("C".to_owned(), "ascii".to_owned()),
    };
    if matches!(locale.as_str(), "C" | "POSIX") {
        if utf8 != Some(false) {
            return (utf8_codec, "surrogateescape");
        }
        if std::env::var_os("LC_ALL").is_none_or(|s| s.is_empty())
            && (isolated || std::env::var("PYTHONCOERCECLOCALE").as_deref() != Ok("0"))
            && ["C.UTF-8", "C.utf8", "UTF-8"]
                .into_iter()
                .any(|name| locale_codeset(name).is_some())
        {
            return (utf8_codec, "surrogateescape");
        }
        return (lookup(&codeset), "surrogateescape");
    }
    let errors = if matches!(locale.as_str(), "C.UTF-8" | "C.utf8" | "UTF-8") {
        "surrogateescape"
    } else {
        "strict"
    };
    (lookup(&codeset), errors)
}

/// Read the selected character set without changing the process locale.
fn locale_codeset(name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    // SAFETY: the locale handle is owned here. Only this thread selects it, the
    // codeset is copied while it is live, and the previous locale is restored.
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
        let ptr = libc::nl_langinfo(libc::CODESET);
        let result = (!ptr.is_null())
            .then(|| CStr::from_ptr(ptr).to_str().ok().map(str::to_owned))
            .flatten();
        libc::uselocale(previous);
        libc::freelocale(local);
        result
    }
}
