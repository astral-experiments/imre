//! Lexical component operations borrow parsed path flavors and never access the filesystem.

use super::*;
use crate::objects::exceptions::string_repr;
use crate::objects::words::Words;

impl Storage {
    fn parsed<T>(&self, f: impl FnOnce(&PurePath) -> T) -> T {
        match self {
            Self::PurePath(path) => f(path),
            _ => f(&PurePath::new(false, vec![Rc::clone(self.path_text())])),
        }
    }

    fn path_repr(&self) -> String {
        match self {
            Self::PurePath(p) => format!("{}({})", p.class(), string_repr(&p.as_posix())),
            _ => format!("PosixPath({})", string_repr(self.path())),
        }
    }

    pub(super) fn replace_name(&self, replacement: &Str) -> Result<*const Object> {
        if let Storage::PurePath(p) = self {
            return p.with_name(replacement);
        }
        if replacement.is_empty() || replacement.as_str() == "." || replacement.contains('/') {
            return Err(Error::new(
                Kind::ValueError,
                format!("Invalid name {}", string_repr(replacement)),
            ));
        }
        if self.basename().is_empty() {
            return Err(Error::new(
                Kind::ValueError,
                format!("{} has an empty name", self.path_repr()),
            ));
        }
        Ok(new_path(join(parent(self.path()), replacement)))
    }
}

/// # Safety
/// Both rooted objects are immutable paths; they may identify the same object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_is_relative_to(
    path: *const Object,
    other: *const Object,
) -> u32 {
    // SAFETY: no callback, mutation or borrowed component escapes the comparison.
    u32::from(unsafe {
        object(path)
            .0
            .with(|s| s.parsed(|p| object(other).0.with(|o| o.parsed(|o| p.is_relative_to(o)))))
    })
}

/// # Safety
/// Both path owners remain rooted until the newly owned relative path is returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_relative_to(
    path: *const Object,
    other: *const Object,
    walk_up: u32,
) -> *const Object {
    // SAFETY: parsed components coexist only under immutable storage borrows.
    let result = unsafe {
        object(path).0.with(|s| {
            let relative = s.parsed(|p| {
                object(other)
                    .0
                    .with(|o| o.parsed(|o| p.relative_to(o, walk_up != 0)))
            })?;
            Ok(if matches!(s, Storage::PurePath(_)) {
                relative.into_object()
            } else {
                new_path(relative.text)
            })
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The path and new component are live throughout validation; the result owns its path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_change_extension(
    path: *const Object,
    value: *const Str,
    stem: u32,
) -> *const Object {
    // SAFETY: no callback invalidates the immutable path or replacement string.
    let result = unsafe {
        object(path).0.with(|s| {
            let value = &*value;
            let name = s.basename();
            let (old_stem, old_suffix) = name.split_at(suffix_start(name).unwrap_or(name.len()));
            if stem != 0 {
                if old_suffix.is_empty() {
                    return s.replace_name(value);
                }
                if value.is_empty() {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("{} has a non-empty suffix", s.path_repr()),
                    ));
                }
                let mut name = value.clone();
                name.push_str(old_suffix);
                s.replace_name(&name)
            } else {
                if old_stem.is_empty() {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("{} has an empty name", s.path_repr()),
                    ));
                }
                if !value.is_empty() && !value.starts_with('.') {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("Invalid suffix {}", string_repr(value)),
                    ));
                }
                let mut name = Str::from(old_stem);
                name.push_str(value);
                s.replace_name(&name)
            }
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// The input path stays rooted until each suffix is installed in its own result list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_suffixes(path: *const Object) -> *const Object {
    // SAFETY: immutable names are copied into independent string owners.
    let result = unsafe {
        object(path).0.with(|s| {
            let name = s.basename().trim_start_matches('.');
            let mut suffixes = Words::new(3);
            suffixes
                .values
                .try_reserve(name.bytes().filter(|b| *b == b'.').count())
                .map_err(|_| Error::allocation())?;
            for part in name.split('.').skip(1) {
                let mut suffix = Str::from(".");
                suffix.push_str(part);
                suffixes.values.push(Rc::into_raw(Rc::new(suffix)) as usize);
            }
            Ok(Rc::into_raw(suffixes.into_object()))
        })
    };
    finish(result, std::ptr::null())
}
