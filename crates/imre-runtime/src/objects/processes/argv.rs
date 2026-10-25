//! Convert rooted Python command objects into independent, NUL-checked exec arguments.

use super::*;
use crate::objects::records::descriptor;

/// Borrow argv[0] and its descriptor while the original command stays rooted.
pub(super) fn first(cmd: &OwnedWord) -> Option<(usize, WordKind)> {
    // SAFETY: the compiler admits only scalars, lists, and tuples of path arguments.
    unsafe {
        match cmd.kind.raw() {
            5 => object(cmd.word as *const Object).0.with(|s| match s {
                Storage::Sequence(list) => list.values.first().map(|&word| (word, list.kind)),
                _ => Some((cmd.word, cmd.kind)),
            }),
            6 => {
                let tuple = cmd.word as *const usize;
                let layout = descriptor(tuple);
                (layout.length != 0).then(|| {
                    let kind = WordKind::new(*layout.kinds);
                    (kind.read_record_field(tuple), kind)
                })
            }
            _ => Some((cmd.word, cmd.kind)),
        }
    }
}

/// Each Python string, bytes value, or Path becomes exactly one Unix argument.
pub(super) fn copy(cmd: &OwnedWord) -> Result<Vec<CString>> {
    // SAFETY: cmd owns the collection and every element while copying. Tuple
    // descriptors give each field's actual representation, including mixed tuples.
    unsafe {
        match cmd.kind.raw() {
            5 => object(cmd.word as *const Object).0.with(|s| match s {
                Storage::Path { .. } | Storage::PurePath(_) => {
                    Ok(vec![cstring(s.path().as_bytes())?])
                }
                Storage::Sequence(list) => copy_argv(&list.values, |word| element(word, list.kind)),
                _ => fail("SystemError", "unexpected process command object"),
            }),
            6 => {
                let tuple = cmd.word as *const usize;
                let layout = descriptor(tuple);
                let mut result = Vec::new();
                result
                    .try_reserve_exact(layout.length)
                    .map_err(|_| Error::allocation())?;
                for i in 0..layout.length {
                    let kind = WordKind::new(*layout.kinds.add(i));
                    result.push(element(kind.read_record_field(tuple.add(i)), kind)?);
                }
                Ok(result)
            }
            _ => Ok(vec![element(cmd.word, cmd.kind)?]),
        }
    }
}

/// # Safety
/// A non-null value must be a rooted str, bytes or Path with this exact kind.
pub(super) unsafe fn optional_path(value: *const (), kind: u32) -> Result<Option<CString>> {
    // SAFETY: the compiler restricts the optional to native path representations.
    (!value.is_null())
        .then(|| unsafe { element(value as usize, WordKind::new(kind)) })
        .transpose()
}

/// # Safety
/// The caller roots the element with the given storage kind.
unsafe fn element(word: usize, kind: WordKind) -> Result<CString> {
    // SAFETY: the caller roots the exact command argument.
    unsafe { with_bytes(word, kind, cstring) }
}

/// Inspect the original path bytes, including NUL, before a launch API validates them.
pub(super) unsafe fn with_bytes<T>(word: usize, kind: WordKind, f: impl FnOnce(&[u8]) -> T) -> T {
    let (word, kind) = if kind.raw() == 9 {
        crate::objects::unions::member(word)
    } else {
        (word, kind)
    };
    // SAFETY: all permitted union members are owned str, bytes or Path values.
    unsafe {
        match kind.raw() {
            3 => f((&*(word as *const Str)).as_bytes()),
            8 => f(&*(word as *const Bytes)),
            5 => object(word as *const Object)
                .0
                .with(|s| f(s.path().as_bytes())),
            _ => fail("SystemError", "unexpected process argument element"),
        }
    }
}

/// Subprocess exception messages interpolate str(cmd), which differs from repr
/// for a standalone string or Path. Lists and tuples keep their normal repr.
pub(super) fn display(cmd: &OwnedWord) -> String {
    // SAFETY: formatting cannot mutate the closed, acyclic command object.
    unsafe {
        if cmd.kind.raw() == 3 {
            return (&*(cmd.word as *const Str)).as_str().to_owned();
        }
        if cmd.kind.raw() == 5
            && let Some(path) = object(cmd.word as *const Object).0.with(|s| {
                matches!(s, Storage::Path { .. } | Storage::PurePath(_))
                    .then(|| s.path().to_owned())
            })
        {
            return path;
        }
        let mut text = String::new();
        crate::objects::values::write_repr(&mut text, cmd.word, cmd.kind);
        text
    }
}
