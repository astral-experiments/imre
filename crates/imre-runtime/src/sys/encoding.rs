//! Filesystem and standard stream settings captured before user code runs.
use crate::objects::Codec;
use std::borrow::Cow;
use std::ffi::{CStr, CString, OsStr};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

struct Locale {
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
pub(crate) fn file_encoding(name: Option<&str>) -> &str {
    match name {
        Some("locale") => &settings(false, None).locale.codeset,
        None => settings(false, None).locale.default_text_encoding(),
        Some(name) => name,
    }
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
            .find_map(locale_codeset)
    });
    let codeset = coerced
        .flatten()
        .or(charset)
        .or_else(|| locale_codeset("C"))
        .unwrap_or_else(|| "utf-8".to_owned());
    let errors =
        if utf8_mode || legacy || matches!(locale.to_str(), Some("C.UTF-8" | "C.utf8" | "UTF-8")) {
            "surrogateescape"
        } else {
            "strict"
        };
    (Locale { codeset, utf8_mode }, errors)
}

/// Read the selected character set without changing the process locale.
fn locale_codeset(name: impl AsRef<OsStr>) -> Option<String> {
    let name = CString::new(name.as_ref().as_bytes()).ok()?;
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
            .flatten()
            .filter(|name| !name.is_empty());
        libc::uselocale(previous);
        libc::freelocale(local);
        result
    }
}
