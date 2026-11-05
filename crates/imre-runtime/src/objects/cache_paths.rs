//! POSIX cache naming. Native code has no bytecode tag; callers may select a target interpreter.
use super::{
    Str,
    exceptions::{Error, Kind, string_repr},
    files, finish,
    text::python_alnum,
};
use std::cell::RefCell;
#[derive(Default)]
struct Config {
    tag: Option<crate::StrOwner>,
    prefix: Option<crate::StrOwner>,
}
thread_local! { static CONFIG: RefCell<Config> = RefCell::default(); }
#[unsafe(no_mangle)]
pub extern "C" fn imre_cache_paths_init(read_env: u32) {
    if read_env == 0 {
        return;
    }
    let p = std::env::var("PYTHONPYCACHEPREFIX")
        .ok()
        .filter(|p| !p.is_empty());
    CONFIG.with_borrow_mut(|c| c.prefix = p.map(|p| crate::StrOwner::new(Str::from(p))));
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_cache_config_get(prefix: u32) -> usize {
    CONFIG.with_borrow(|c| {
        (if prefix != 0 { &c.prefix } else { &c.tag })
            .as_ref()
            .map_or(0, |v| crate::StrOwner::into_raw(v.clone()) as usize)
    })
}
/// # Safety
/// A nonzero optional word is a live, Rc-owned native string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_config_set(prefix: u32, value: usize) {
    let value = if value == 0 {
        None
    } else {
        // SAFETY: retain the exact caller object in the public attribute.
        Some(unsafe { crate::StrOwner::acquire_word(value) })
    };
    CONFIG.with_borrow_mut(|c| {
        if prefix != 0 {
            c.prefix = value
        } else {
            c.tag = value
        }
    });
}
fn tag() -> Result<crate::StrOwner, Error> {
    CONFIG.with_borrow(|c| c.tag.clone()).ok_or_else(|| {
        Error::new(
            Kind::NotImplementedError,
            "sys.implementation.cache_tag is None",
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_cache_path_tag() -> *const Str {
    finish(tag().map(crate::StrOwner::into_raw), std::ptr::null())
}
// Bootstrap splits root-only heads differently from posixpath.split.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}
fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.trim_end_matches('/'))
        .collect::<Vec<_>>()
        .join("/")
}
pub(crate) const PYCACHE: &str = "__pycache__";

pub(crate) fn cache_relative_head(head: &str) -> Result<String, Error> {
    if head.starts_with('/') {
        return Ok(head.trim_start_matches('/').to_owned());
    }
    let cwd = std::env::current_dir().map_err(files::errno_error)?;
    let cwd = super::paths::path_utf8(cwd.into_os_string())?;
    let mut head = join(&[&cwd, head.strip_prefix("./").unwrap_or(head)]);
    let root_len = head.len() - head.trim_start_matches('/').len();
    head.drain(..root_len);
    Ok(head)
}
pub(crate) fn cache_stem<'a>(
    path: &'a str,
    tag: &str,
    optimization: &str,
) -> Result<(&'a str, String), Error> {
    let (head, tail) = split(path);
    let (base, sep, rest) = tail
        .rsplit_once('.')
        .map_or(("", "", tail), |(b, r)| (b, ".", r));
    let mut name = format!("{}{sep}{tag}", if base.is_empty() { rest } else { base });
    if !optimization.is_empty() {
        if !python_alnum(optimization) {
            return Err(Error::new(
                Kind::ValueError,
                format!("{} is not alphanumeric", string_repr(optimization)),
            ));
        }
        name.push_str(".opt-");
        name.push_str(optimization);
    }
    Ok((head, name))
}

pub(crate) fn cache_location(head: &str, name: &str, prefix: Option<&str>) -> Result<Str, Error> {
    match prefix {
        None => Ok(Str::from(join(&[head, PYCACHE, name]))),
        Some(p) => Ok(Str::from(join(&[p, &cache_relative_head(head)?, name]))),
    }
}
fn cache_name(
    path: &str,
    tag: &str,
    optimization: &str,
    prefix: Option<&str>,
) -> Result<Str, Error> {
    let (head, mut name) = cache_stem(path, tag, optimization)?;
    name.push_str(".pyc");
    cache_location(head, &name, prefix)
}

pub(crate) fn source_parts<'a>(
    path: &'a str,
    prefix: Option<&str>,
) -> Result<(&'a str, &'a str), Error> {
    let (head, name) = split(path);
    let inside = prefix.and_then(|p| {
        head.strip_prefix(p.trim_end_matches('/'))
            .filter(|s| s.starts_with('/'))
    });
    let dir = match inside {
        Some(s) => s,
        None => {
            let (d, c) = split(head);
            if c != "__pycache__" {
                return Err(Error::new(
                    Kind::ValueError,
                    format!(
                        "__pycache__ not bottom-level directory in {}",
                        string_repr(path)
                    ),
                ));
            }
            d
        }
    };
    match name.bytes().filter(|b| *b == b'.').count() {
        2 => {}
        3 => {
            let opt = name.rsplit('.').nth(1).expect("three separators");
            let level = opt.strip_prefix("opt-").ok_or_else(|| {
                Error::new(
                    Kind::ValueError,
                    "optimization portion of filename does not start with 'opt-'",
                )
            })?;
            if !python_alnum(level) {
                return Err(Error::new(
                    Kind::ValueError,
                    format!(
                        "optimization level {} is not an alphanumeric value",
                        string_repr(opt)
                    ),
                ));
            }
        }
        _ => {
            return Err(Error::new(
                Kind::ValueError,
                format!("expected only 2 or 3 dots in {}", string_repr(name)),
            ));
        }
    }
    Ok((dir, name.split('.').next().unwrap_or("")))
}

pub(crate) fn source_location(dir: &str, name: &str) -> Str {
    Str::from(join(&[dir, name]))
}

fn source_name(path: &str, prefix: Option<&str>) -> Result<Str, Error> {
    let (dir, stem) = source_parts(path, prefix)?;
    Ok(source_location(dir, &format!("{stem}.py")))
}

/// # Safety
/// The generated caller roots the path, previously read tag, and optimization string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_from_source(
    path: *const Str,
    tag: *const Str,
    optimization: *const Str,
) -> *const Str {
    // SAFETY: all three strings remain live for the call.
    let result = CONFIG.with_borrow(|c| unsafe {
        cache_name(
            &*path,
            &*tag,
            &*optimization,
            c.prefix.as_ref().map(|p| p.as_str()),
        )
    });
    finish(
        result.map(|s| crate::StrOwner::into_raw(crate::StrOwner::new(s))),
        std::ptr::null(),
    )
}
/// # Safety
/// The checked tag and filesystem path are rooted strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_source_from_cache(_tag: *const Str, path: *const Str) -> *const Str {
    // SAFETY: the path remains live for the call.
    finish(
        CONFIG
            .with_borrow(|c| unsafe { source_name(&*path, c.prefix.as_ref().map(|p| p.as_str())) })
            .map(|s| crate::StrOwner::into_raw(crate::StrOwner::new(s))),
        std::ptr::null(),
    )
}
pub(super) fn module_cache(
    path: &str,
    optimization: &str,
) -> Result<Option<crate::StrOwner>, Error> {
    CONFIG.with_borrow(|c| {
        c.tag
            .as_ref()
            .map(|tag| {
                cache_name(
                    path,
                    tag,
                    optimization,
                    c.prefix.as_ref().map(|p| p.as_str()),
                )
                .map(crate::StrOwner::new)
            })
            .transpose()
    })
}

/// Warn before checking conflicting arguments or invoking the truth protocol.
#[unsafe(no_mangle)]
pub extern "C" fn imre_cache_debug_validate(no_debug: u32, no_opt: u32) {
    if no_debug != 0 {
        return;
    }
    finish(
        warn_debug_override().and_then(|()| {
            if no_opt == 0 {
                Err(Error::new(
                    Kind::TypeError,
                    "debug_override or optimization must be set to None",
                ))
            } else {
                Ok(())
            }
        }),
        (),
    );
}

pub(crate) const DEBUG_OVERRIDE_WARNING: &str =
    "the debug_override parameter is deprecated; use 'optimization' instead";

pub(crate) fn warn_debug_override() -> Result<(), Error> {
    super::warnings::deprecation(
        "importlib._bootstrap_external",
        "<frozen importlib._bootstrap_external>",
        258,
        DEBUG_OVERRIDE_WARNING,
    )
}
