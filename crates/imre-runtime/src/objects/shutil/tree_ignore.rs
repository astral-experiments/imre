use super::*;
use crate::StrOwner;
use crate::objects::{mappings, values};
use std::borrow::Cow;

pub(super) type Function =
    unsafe extern "C" fn(*const (), *const (), *const Object) -> *const Object;

#[derive(Clone, Copy)]
pub(super) struct Ignore {
    pub(super) function: Function,
    pub(super) context: *const (),
    pub(super) set: bool,
}

pub(super) struct Ignored {
    names: crate::ObjectOwner,
    set: bool,
}

impl Ignore {
    /// CPython supplies one list of entry names, before creating the destination.
    pub(super) fn apply(self, src: &Name, entries: &[fs::DirEntry]) -> TreeResult<Ignored> {
        let mut names = Words::new(if matches!(src, Name::Bytes(_)) { 8 } else { 3 });
        names
            .values
            .try_reserve(entries.len())
            .map_err(|_| Error::allocation())?;
        for entry in entries {
            let name = src.joined(Path::new(&entry.file_name()))?;
            let value = match name {
                Name::Text(s) | Name::Path(s) => StrOwner::into_raw(s) as usize,
                Name::Bytes(b) => crate::BytesOwner::into_raw(b) as usize,
            };
            names.values.push(value);
        }
        let names = names.into_object();
        // SAFETY: the compiler verifies both parameter types and whether the return is a list or set.
        let raw = unsafe {
            (self.function)(
                self.context,
                callback::handle(src),
                crate::ObjectOwner::as_ptr(&names),
            )
        };
        if let Some(error) = take_pending() {
            return Err(Failure::Raised(error));
        }
        // SAFETY: a successful callback returns one independently owned container.
        let names = unsafe { crate::ObjectOwner::from_raw(raw) };
        Ok(Ignored {
            names,
            set: self.set,
        })
    }
}

impl Ignored {
    /// Retain the callback's container, so a custom copier can mutate it between entries.
    pub(super) fn contains(&self, name: &Name) -> Result<bool> {
        let key = callback::handle(name) as usize;
        // SAFETY: key and all entries have the exact path kind checked at the call site.
        unsafe {
            if self.set {
                Ok(mappings::imre_set_contains(crate::ObjectOwner::as_ptr(&self.names), key) != 0)
            } else {
                self.names.0.with(|s| {
                    let list = s.sequence()?;
                    Ok(list
                        .values
                        .iter()
                        .any(|&word| values::equal(key, word, list.kind)))
                })
            }
        }
    }
}

/// Match filenames using fnmatch's text rules (Latin-1 for bytes) and return a
/// native set containing the original name objects.
///
/// # Safety
/// The callable context is a one-field record owning its pattern list; names
/// is a rooted list with the same exact string or bytes kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_ignore_patterns_call(
    context: *const usize,
    _path: *const (),
    names: *const Object,
) -> *const Object {
    unsafe fn spelling<'a>(word: usize, kind: u32) -> Cow<'a, str> {
        // SAFETY: the containing list roots this exact immutable value.
        unsafe {
            if kind == 3 {
                Cow::Borrowed((&*(word as *const Str)).as_str())
            } else {
                Cow::Owned(
                    (&*(word as *const Bytes))
                        .iter()
                        .map(|&b| char::from(b))
                        .collect(),
                )
            }
        }
    }
    // SAFETY: the borrowed lists are not mutated, and the output set is fresh.
    let result = unsafe {
        let patterns = &*(*context as *const Object);
        patterns.0.with(|s| {
            let source = s.sequence()?;
            let mut patterns = Vec::new();
            patterns
                .try_reserve(source.values.len())
                .map_err(|_| Error::allocation())?;
            for &word in &source.values {
                patterns.push(paths::Pattern::component(&spelling(
                    word,
                    source.kind.raw(),
                )));
            }
            (*names).0.with(|s| {
                let names = s.sequence()?;
                let output = crate::ObjectOwner::from_raw(mappings::imre_set_new(names.kind.raw()));
                for &name in &names.values {
                    let text = spelling(name, names.kind.raw());
                    if patterns.iter().any(|pattern| pattern.matches(&text)) {
                        mappings::set_add(crate::ObjectOwner::as_ptr(&output), name)?;
                    }
                }
                Ok(crate::ObjectOwner::into_raw(output))
            })
        })
    };
    finish(result, std::ptr::null())
}
