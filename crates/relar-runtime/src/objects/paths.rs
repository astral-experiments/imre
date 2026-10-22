//! Sealed POSIX paths with lexical normalization and snapshot directory iterators.
//! Path objects are immutable; directory state owns names, never open descriptors.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::exceptions::{Error, Kind, Result, finish};
use super::{Int, Object, Storage, StorageCell, Str, fail, files, object};

impl Storage {
    pub(super) fn path(&self) -> &str {
        match self {
            Self::Path(path) => path,
            _ => fail("SystemError", "expected a path object"),
        }
    }
}

/// POSIX normalization removes empty and dot components while retaining `..`
/// and exactly two leading slashes, which may have implementation-defined meaning.
fn normalize(path: &str) -> Str {
    let root = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let mut result = Str::from(root);
    for part in path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(part);
    }
    if result.is_empty() {
        result.push('.');
    }
    result
}

fn join(path: &str, child: &str) -> Str {
    if child.starts_with('/') {
        return normalize(child);
    }
    if path == "." {
        return normalize(child);
    }
    let mut joined = Str::with_capacity(path.len() + child.len() + 1);
    joined.push_str(path);
    if !path.ends_with('/') {
        joined.push('/');
    }
    joined.push_str(child);
    normalize(&joined)
}

fn new_path(path: Str) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Path(path)))))
}

fn name(path: &str) -> &str {
    if matches!(path, "." | "/" | "//") {
        ""
    } else {
        path.rsplit('/').next().unwrap()
    }
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        _ if matches!(path, "." | "/" | "//") => path,
        Some(0) => "/",
        Some(1) if path.starts_with("//") => "//",
        Some(index) => &path[..index],
        None => ".",
    }
}

enum ResolvePart {
    Component(Str),
    ResolvedLink(Str),
}

/// Resolve links component by component, retaining missing suffixes when not
/// strict. An explicit stack and link cache handle long acyclic link chains
/// without recursion or the kernel's per-lookup symlink traversal limit.
fn resolve(path: &str, strict: bool) -> Result<Str> {
    let mut resolved = if path.starts_with('/') {
        Str::from("/")
    } else {
        let directory = std::env::current_dir().map_err(files::os_error)?;
        path_utf8(directory.into_os_string())?
    };
    let mut remaining = path.split('/').count();
    let mut parts: Vec<_> = path
        .split('/')
        .rev()
        .map(|part| ResolvePart::Component(Str::from(part)))
        .collect();
    let mut links: HashMap<Str, Option<Str>> = HashMap::new();
    while let Some(part) = parts.pop() {
        let part = match part {
            ResolvePart::Component(part) => part,
            ResolvePart::ResolvedLink(link) => {
                links.insert(link, Some(resolved.clone()));
                continue;
            }
        };
        remaining -= 1;
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            resolved = Str::from(parent(&resolved));
            continue;
        }
        let mut next = resolved.clone();
        if next != "/" {
            next.push('/');
        }
        next.push_str(&part);
        // Validate at the actual lookup, so an earlier strict filesystem error
        // still precedes a later component containing a NUL.
        check_nul(&next)?;
        let metadata = match fs::symlink_metadata(next.as_str()) {
            Ok(metadata) => metadata,
            Err(error) if strict => return Err(files::os_error(error)),
            Err(_) => {
                resolved = next;
                continue;
            }
        };
        if !metadata.file_type().is_symlink() {
            if strict && remaining != 0 && !metadata.is_dir() {
                return Err(files::os_error(io::Error::from_raw_os_error(libc::ENOTDIR)));
            }
            resolved = next;
            continue;
        }
        match links.get(&next) {
            Some(Some(target)) => {
                resolved = target.clone();
                continue;
            }
            Some(None) => {
                if strict {
                    return Err(files::os_error(io::Error::from_raw_os_error(libc::ELOOP)));
                }
                resolved = next;
                continue;
            }
            None => {}
        }
        let target = match fs::read_link(next.as_str()) {
            Ok(target) => path_utf8(target.into_os_string())?,
            Err(error) if strict => return Err(files::os_error(error)),
            Err(_) => {
                resolved = next;
                continue;
            }
        };
        if target.starts_with('/') {
            resolved = Str::from("/");
        }
        links.insert(next.clone(), None);
        parts.push(ResolvePart::ResolvedLink(next));
        remaining += target.split('/').count();
        parts.extend(
            target
                .split('/')
                .rev()
                .map(|part| ResolvePart::Component(Str::from(part))),
        );
    }
    Ok(resolved)
}

fn path_utf8(path: OsString) -> Result<Str> {
    path.into_string()
        .map(Str::from)
        .map_err(|_| Error::new(Kind::UnicodeDecodeError, "resolved path is not valid UTF-8"))
}

fn suffix_start(name: &str) -> Option<usize> {
    let index = name.rfind('.')?;
    name[..index]
        .chars()
        .any(|character| character != '.')
        .then_some(index)
}

/// # Safety
/// `text` is a rooted exact UTF-8 string. The result transfers one Path owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_new(text: *const Str) -> *const Object {
    // SAFETY: the caller roots the immutable source through normalization.
    new_path(normalize(unsafe { &*text }))
}

/// # Safety
/// Root the exact Path and string throughout the call. Absolute children replace
/// the left path; otherwise both operands are joined without resolving symlinks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_join(path: *const Object, child: *const Str) -> *const Object {
    // SAFETY: lexical work has no callbacks and only an owned string escapes.
    let joined = unsafe { object(path).0.with(|storage| join(storage.path(), &*child)) };
    new_path(joined)
}

/// # Safety
/// Both arguments are rooted exact Paths; comparison borrows immutable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_equal(left: *const Object, right: *const Object) -> u8 {
    if left == right {
        return 1;
    }
    // SAFETY: no mutation or callback overlaps these immutable storage borrows.
    unsafe {
        object(left).0.with(|left| {
            object(right)
                .0
                .with(|right| u8::from(left.path() == right.path()))
        })
    }
}

/// # Safety
/// Root an exact Path. The result transfers one owned string with its spelling.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_text(path: *const Object) -> *const Str {
    // SAFETY: only an independent immutable string escapes the storage access.
    let text = unsafe { object(path).0.with(|storage| Str::from(storage.path())) };
    Rc::into_raw(Rc::new(text))
}

/// # Safety
/// Root an exact Path. The result transfers its final component as an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_name(path: *const Object) -> *const Str {
    // SAFETY: the path is immutable and the copied component owns its storage.
    let text = unsafe {
        object(path)
            .0
            .with(|storage| Str::from(name(storage.path())))
    };
    Rc::into_raw(Rc::new(text))
}

/// # Safety
/// Root an exact Path. Python 3.14 recognizes a trailing dot as a suffix, while
/// names consisting entirely of dots and leading-dot names have no such suffix.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_stem(path: *const Object) -> *const Str {
    // SAFETY: only an owned substring escapes this immutable storage access.
    let text = unsafe {
        object(path).0.with(|storage| {
            let name = name(storage.path());
            Str::from(&name[..suffix_start(name).unwrap_or(name.len())])
        })
    };
    Rc::into_raw(Rc::new(text))
}

/// # Safety
/// Root an exact Path. The result includes the final dot or is an empty string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_suffix(path: *const Object) -> *const Str {
    // SAFETY: only an owned substring escapes this immutable storage access.
    let text = unsafe {
        object(path).0.with(|storage| {
            let name = name(storage.path());
            Str::from(&name[suffix_start(name).unwrap_or(name.len())..])
        })
    };
    Rc::into_raw(Rc::new(text))
}

/// # Safety
/// Root an exact Path. This lexical parent never follows symlinks or collapses
/// `..`; roots and the current-directory path are their own parents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_parent(path: *const Object) -> *const Object {
    // SAFETY: the new spelling is owned before the original storage borrow ends.
    let parent = unsafe {
        object(path)
            .0
            .with(|storage| Str::from(parent(storage.path())))
    };
    new_path(parent)
}

/// # Safety
/// Root an exact Path; this lexical predicate cannot fail or invoke callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_is_absolute(path: *const Object) -> u8 {
    // SAFETY: the immutable rooted path is borrowed only for the predicate.
    unsafe {
        object(path)
            .0
            .with(|storage| u8::from(storage.path().starts_with('/')))
    }
}

/// # Safety
/// Root an exact Path with clear pending error state. Success transfers a new
/// absolute Path owner; strict lookup failures preserve their OS error category.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_resolve(path: *const Object, strict: u8) -> *const Object {
    // SAFETY: filesystem calls cannot reenter; no borrowed storage escapes.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| resolve(storage.path(), strict != 0))
    };
    finish(result.map(new_path), std::ptr::null())
}

/// # Safety
/// Root an exact Path. Like Python 3.14, failed metadata queries (including NUL
/// paths) return false and do not install or modify a pending language error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_exists(path: *const Object) -> u8 {
    // SAFETY: filesystem queries have no callbacks; only a boolean escapes.
    unsafe {
        object(path)
            .0
            .with(|storage| u8::from(fs::metadata(storage.path()).is_ok()))
    }
}

/// # Safety
/// Root an exact Path. This follows symlinks and suppresses metadata errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_is_file(path: *const Object) -> u8 {
    // SAFETY: filesystem queries have no callbacks; only a boolean escapes.
    unsafe {
        object(path).0.with(|storage| {
            u8::from(fs::metadata(storage.path()).is_ok_and(|metadata| metadata.is_file()))
        })
    }
}

/// # Safety
/// Root an exact Path. This follows symlinks and suppresses metadata errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_is_dir(path: *const Object) -> u8 {
    // SAFETY: filesystem queries have no callbacks; only a boolean escapes.
    unsafe {
        object(path).0.with(|storage| {
            u8::from(fs::metadata(storage.path()).is_ok_and(|metadata| metadata.is_dir()))
        })
    }
}

/// # Safety
/// Root an exact Path with clear pending error state. Strict UTF-8 reads perform
/// universal-newline conversion and close the descriptor before returning.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_read_text(path: *const Object) -> *const Str {
    // SAFETY: I/O cannot reenter; the result owns its string or error.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| files::read_path_text(storage.path()))
    };
    finish(
        result.map(|text| Rc::into_raw(Rc::new(text))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root an exact Path and string with clear pending error state. Success returns
/// the canonical integer character count after flushing and closing the file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_write_text(path: *const Object, text: *const Str) -> usize {
    // SAFETY: both immutable roots live through the complete synchronous write.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| files::write_path_text(storage.path(), &*text))
    };
    finish(result, 0)
}

fn check_nul(path: &str) -> Result<()> {
    if path.contains('\0') {
        Err(Error::new(Kind::ValueError, "embedded null byte"))
    } else {
        Ok(())
    }
}

/// Match pathlib's retry order: create the leaf first, make missing parents with
/// their default mode, then retry each child without creating its parents again.
fn mkdir(path: &str, mode: u32, parents: bool, exist_ok: bool) -> io::Result<()> {
    let mut pending = Vec::new();
    let (mut current, mut current_mode, mut current_exists) = (path, mode, exist_ok);
    loop {
        match fs::DirBuilder::new().mode(current_mode).create(current) {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let next = parent(current);
                if !parents || next == current {
                    return Err(error);
                }
                pending.push((current, current_mode, current_exists));
                (current, current_mode, current_exists) = (next, 0o777, true);
            }
            Err(_)
                if current_exists
                    && fs::metadata(current).is_ok_and(|metadata| metadata.is_dir()) =>
            {
                break;
            }
            Err(error) => return Err(error),
        }
    }
    for (path, mode, exist_ok) in pending.into_iter().rev() {
        mkdir(path, mode, false, exist_ok)?;
    }
    Ok(())
}

/// # Safety
/// Root an exact Path and canonical Int mode with clear pending error state.
/// This synchronous operation returns no value and never mutates a Path object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_mkdir(
    path: *const Object,
    mode: usize,
    parents: u8,
    exist_ok: u8,
) {
    // SAFETY: both arguments stay rooted through validation and filesystem I/O.
    let result = unsafe {
        object(path).0.with(|storage| {
            let path = storage.path();
            check_nul(path)?;
            let mode = Int::borrow(mode).big().to_i32().ok_or_else(|| {
                Error::new(
                    Kind::OverflowError,
                    "Python int too large to convert to C int",
                )
            })?;
            mkdir(path, mode as u32, parents != 0, exist_ok != 0).map_err(files::os_error)
        })
    };
    finish(result, ());
}

/// # Safety
/// Root an exact Path with clear pending error state. Missing-ok suppresses only
/// a missing entry; directories and non-directory ancestors retain their errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_unlink(path: *const Object, missing_ok: u8) {
    // SAFETY: unlink has no language callbacks and the immutable path stays live.
    let result = unsafe {
        object(path).0.with(|storage| {
            check_nul(storage.path())?;
            match fs::remove_file(storage.path()) {
                Err(error) if missing_ok != 0 && error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result.map_err(files::os_error),
            }
        })
    };
    finish(result, ());
}

/// # Safety
/// Root an exact Path with clear pending error state. Only an empty directory is
/// removed; symlinks are never followed by the final directory-removal operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_rmdir(path: *const Object) {
    // SAFETY: the rooted spelling is borrowed only for synchronous I/O.
    let result = unsafe {
        object(path).0.with(|storage| {
            check_nul(storage.path())?;
            fs::remove_dir(storage.path()).map_err(files::os_error)
        })
    };
    finish(result, ());
}

/// # Safety
/// Root an exact Path and target string with clear pending error state. A
/// successful rename transfers one new Path owner; failure returns null. Relative
/// targets use the process cwd, and target spelling is normalized only afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_replace(
    path: *const Object,
    target: *const Str,
) -> *const Object {
    // SAFETY: the roots remain live throughout rename and target normalization.
    let result = unsafe {
        object(path).0.with(|storage| {
            check_nul(storage.path())?;
            check_nul(&*target)?;
            fs::rename(storage.path(), (&*target).as_str()).map_err(files::os_error)?;
            Ok(new_path(normalize(&*target)))
        })
    };
    finish(result, std::ptr::null())
}

pub(super) struct PathIterator {
    root: Str,
    state: IterationState,
}

enum IterationState {
    PendingGlob(Pattern),
    Snapshot {
        names: std::vec::IntoIter<OsString>,
        pattern: Option<Pattern>,
    },
    Exhausted,
}

/// Takes the complete name snapshot and closes the directory handle before any
/// Path can escape, matching CPython's scandir-based iterator acquisition.
fn snapshot(root: &str, suppress_errors: bool) -> Result<Vec<OsString>> {
    if root.contains('\0') {
        return Err(Error::new(Kind::ValueError, "embedded null byte"));
    }
    let entries = fs::read_dir(root)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect()
        })
        .map_err(files::os_error);
    if suppress_errors {
        Ok(entries.unwrap_or_default())
    } else {
        entries
    }
}

impl PathIterator {
    fn new(root: Str, pattern: Option<&str>) -> Result<Self> {
        let state = if let Some(pattern) = pattern {
            IterationState::PendingGlob(Pattern::new(pattern)?)
        } else {
            IterationState::Snapshot {
                names: snapshot(&root, false)?.into_iter(),
                pattern: None,
            }
        };
        Ok(Self { root, state })
    }

    fn next(&mut self) -> Result<*const Object> {
        if matches!(self.state, IterationState::PendingGlob(_)) {
            let IterationState::PendingGlob(pattern) =
                std::mem::replace(&mut self.state, IterationState::Exhausted)
            else {
                unreachable!()
            };
            if pattern.literal {
                let path = join(&self.root, &pattern.text);
                // Literal globbing uses lexists, so dangling symlinks are included
                // without scanning or inspecting unrelated directory names.
                return Ok(if fs::symlink_metadata(path.as_str()).is_ok() {
                    new_path(path)
                } else {
                    std::ptr::null()
                });
            }
            let names = snapshot(&self.root, true)?;
            self.state = IterationState::Snapshot {
                names: names.into_iter(),
                pattern: Some(pattern),
            };
        }
        let IterationState::Snapshot { names, pattern } = &mut self.state else {
            return Ok(std::ptr::null());
        };
        for name in names {
            let name = name.into_string().map_err(|_| {
                Error::new(
                    Kind::UnicodeDecodeError,
                    "directory entry is not valid UTF-8",
                )
            })?;
            if pattern
                .as_ref()
                .is_none_or(|pattern| pattern.matches(&name))
            {
                return Ok(new_path(join(&self.root, &name)));
            }
        }
        self.state = IterationState::Exhausted;
        Ok(std::ptr::null())
    }
}

/// # Safety
/// Root an exact Path and either a string pattern or null. Null selects iterdir:
/// it snapshots names now and propagates directory errors. A glob validates now,
/// scans on its first next call, and suppresses OS scan errors. Returns one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_iter(
    path: *const Object,
    pattern: *const Str,
) -> *const Object {
    // SAFETY: only the copied root and pattern survive this immutable borrow.
    let result = unsafe {
        object(path).0.with(|storage| {
            PathIterator::new(
                Str::from(storage.path()),
                pattern.as_ref().map(|text| text.as_str()),
            )
        })
    };
    finish(
        result.map(|iterator| {
            Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::PathIterator(
                Box::new(iterator),
            )))))
        }),
        std::ptr::null(),
    )
}

/// # Safety
/// Root an exact directory iterator with clear pending error state. A nonnull
/// result owns one Path; null denotes exhaustion unless an exception is pending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_path_next(iterator: *const Object) -> *const Object {
    // SAFETY: no callback or storage reference escapes the exclusive iterator step.
    let result = unsafe {
        object(iterator).0.with_mut(|storage| match storage {
            Storage::PathIterator(iterator) => iterator.next(),
            _ => fail("SystemError", "expected a path iterator"),
        })
    };
    finish(result, std::ptr::null())
}

struct Pattern {
    text: Str,
    tokens: Vec<Token>,
    literal: bool,
}

enum Token {
    Literal(char),
    Any,
    Star,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl Pattern {
    fn new(pattern: &str) -> Result<Self> {
        if pattern.is_empty() || pattern == "." {
            return Err(Error::new(
                Kind::ValueError,
                "unacceptable empty glob pattern",
            ));
        }
        if pattern.contains('/') || pattern.contains("**") {
            return Err(Error::new(
                Kind::ValueError,
                "glob requires a nonrecursive single-component pattern",
            ));
        }
        let chars: Vec<_> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let character = chars[index];
            index += 1;
            match character {
                '*' => tokens.push(Token::Star),
                '?' => tokens.push(Token::Any),
                '[' => {
                    let start = index;
                    let negated = chars.get(index) == Some(&'!');
                    let content = index + usize::from(negated);
                    let search = content + usize::from(chars.get(content) == Some(&']'));
                    let Some(end) = (search..chars.len()).find(|&at| chars[at] == ']') else {
                        tokens.push(Token::Literal('['));
                        continue;
                    };
                    debug_assert!(end > start);
                    tokens.push(character_class(&chars[start..end]));
                    index = end + 1;
                }
                character => tokens.push(Token::Literal(character)),
            }
        }
        Ok(Self {
            text: Str::from(pattern),
            tokens,
            literal: !pattern.contains(['*', '?', '[']),
        })
    }

    /// Greedy star matching remembers the most recent star and advances its
    /// Unicode-scalar boundary on mismatch, avoiding recursive backtracking.
    fn matches(&self, name: &str) -> bool {
        let (mut token, mut at) = (0, 0);
        let mut star = None;
        while at < name.len() {
            if matches!(self.tokens.get(token), Some(Token::Star)) {
                star = Some((token, at));
                token += 1;
                continue;
            }
            let character = name[at..].chars().next().unwrap();
            if self
                .tokens
                .get(token)
                .is_some_and(|token| token.matches(character))
            {
                token += 1;
                at += character.len_utf8();
                continue;
            }
            if let Some((previous, consumed)) = star {
                let next = consumed + name[consumed..].chars().next().unwrap().len_utf8();
                star = Some((previous, next));
                token = previous + 1;
                at = next;
            } else {
                return false;
            }
        }
        while matches!(self.tokens.get(token), Some(Token::Star)) {
            token += 1;
        }
        token == self.tokens.len()
    }
}

/// Python's fnmatch removes descending ranges before interpreting negation.
/// Preserve literal hyphens separately from range separators: `[b-a!]`, for
/// example, becomes a negated empty class and therefore matches any character.
fn character_class(chars: &[char]) -> Token {
    let mut chunks = Vec::<Vec<char>>::new();
    let mut start = 0;
    let mut search = 1 + usize::from(chars.first() == Some(&'!'));
    while let Some(index) = (search..chars.len()).find(|&index| chars[index] == '-') {
        chunks.push(chars[start..index].to_vec());
        start = index + 1;
        search = index + 3;
    }
    if start < chars.len() {
        chunks.push(chars[start..].to_vec());
    } else if let Some(last) = chunks.last_mut() {
        last.push('-');
    }
    for index in (1..chunks.len()).rev() {
        if chunks[index - 1].last().unwrap() > chunks[index].first().unwrap() {
            let removed = chunks.remove(index);
            chunks[index - 1].pop();
            chunks[index - 1].extend(removed.into_iter().skip(1));
        }
    }
    let mut characters = Vec::new();
    for (index, chunk) in chunks.into_iter().enumerate() {
        if index != 0 {
            characters.push(('-', true));
        }
        characters.extend(chunk.into_iter().map(|character| (character, false)));
    }
    let negated = characters
        .first()
        .is_some_and(|&(character, _)| character == '!');
    let mut index = usize::from(negated);
    let mut ranges = Vec::new();
    while index < characters.len() {
        if index + 2 < characters.len() && characters[index + 1].1 {
            ranges.push((characters[index].0, characters[index + 2].0));
            index += 3;
        } else {
            ranges.push((characters[index].0, characters[index].0));
            index += 1;
        }
    }
    Token::Class { negated, ranges }
}

impl Token {
    fn matches(&self, character: char) -> bool {
        match self {
            Self::Literal(expected) => *expected == character,
            Self::Any => true,
            Self::Star => false,
            Self::Class { negated, ranges } => {
                ranges
                    .iter()
                    .any(|&(start, end)| start <= character && character <= end)
                    != *negated
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::Int;
    use crate::objects::exceptions::{
        relar_error_pending, relar_error_take, relar_exception_matches,
    };
    use crate::objects::{relar_object_release, values};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "relar-runtime-paths-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> Rc<Object> {
            path(self.0.to_str().unwrap())
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn path(text: &str) -> Rc<Object> {
        // SAFETY: this new allocation transfers one owner into the test's Rc.
        unsafe { Rc::from_raw(new_path(normalize(text))) }
    }
    fn take_error(kind: Kind) {
        assert_eq!(relar_error_pending(), 1);
        let error = relar_error_take();
        // SAFETY: take transferred the live exception, which is inspected then released.
        unsafe {
            assert_eq!(relar_exception_matches(error, kind as u32), 1);
            relar_object_release(error);
        }
    }
    fn names(iterator: &mut PathIterator) -> Vec<String> {
        let mut result = Vec::new();
        loop {
            let next = iterator
                .next()
                .unwrap_or_else(|_| panic!("unexpected iteration error"));
            if next.is_null() {
                break;
            }
            // SAFETY: each successful next transfers one exact Path owner.
            let next = unsafe { Rc::from_raw(next) };
            // SAFETY: the immutable Path remains rooted during this copied-name read.
            result.push(unsafe { next.0.with(|storage| name(storage.path()).to_owned()) });
        }
        result.sort();
        result
    }

    #[test]
    fn lexical_paths_preserve_roots_parent_components_and_python_314_suffixes() {
        for (raw, normalized, expected_parent, expected_name, stem, suffix) in [
            ("", ".", ".", "", "", ""),
            ("///a/./b//../", "/a/b/..", "/a/b", "..", "..", ""),
            ("//a//b/", "//a/b", "//a", "b", "b", ""),
            ("//a", "//a", "//", "a", "a", ""),
            ("//", "//", "//", "", "", ""),
            (".profile", ".profile", ".", ".profile", ".profile", ""),
            ("...", "...", ".", "...", "...", ""),
            ("..a.", "..a.", ".", "..a.", "..a", "."),
            ("x.tar.gz", "x.tar.gz", ".", "x.tar.gz", "x.tar", ".gz"),
            ("α/β.txt", "α/β.txt", "α", "β.txt", "β", ".txt"),
        ] {
            let value = path(raw);
            // SAFETY: the immutable local Path remains rooted during the read.
            let actual = unsafe { value.0.with(|storage| storage.path().to_owned()) };
            assert_eq!(actual, normalized);
            // SAFETY: the Rc roots each Path; every ABI result transfers a fresh owner.
            unsafe {
                let pointer = Rc::as_ptr(&value);
                assert_eq!(
                    Rc::from_raw(relar_path_name(pointer)).as_str(),
                    expected_name
                );
                assert_eq!(Rc::from_raw(relar_path_stem(pointer)).as_str(), stem);
                assert_eq!(Rc::from_raw(relar_path_suffix(pointer)).as_str(), suffix);
                let parent = Rc::from_raw(relar_path_parent(pointer));
                assert_eq!(
                    parent.0.with(|storage| storage.path().to_owned()),
                    expected_parent
                );
            }
        }
        assert_eq!(join("//", "x"), "//x");
        assert_eq!(join("a/b", "/x"), "/x");
        assert_eq!(join("a", "../b"), "a/../b");
    }

    #[test]
    fn paths_are_values_in_record_equality_and_representation() {
        let left = path("a/./b");
        let right = path("a/b");
        // SAFETY: both roots have exact Object representation 5 for this traversal.
        unsafe {
            assert_eq!(relar_path_equal(Rc::as_ptr(&left), Rc::as_ptr(&right)), 1);
            assert!(values::equal(
                Rc::as_ptr(&left) as usize,
                Rc::as_ptr(&right) as usize,
                super::super::words::WordKind::new(5)
            ));
            assert_eq!(
                Rc::from_raw(values::relar_value_repr(Rc::as_ptr(&left) as usize, 5)).as_str(),
                "PosixPath('a/b')"
            );
        }
    }

    #[test]
    fn path_reads_writes_predicates_and_errors_share_text_file_semantics() {
        let directory = Directory::new();
        let filename = directory.0.join("text");
        let value = path(filename.to_str().unwrap());
        let root = directory.path();
        // SAFETY: all exact roots outlive their ABI calls, which transfer owned results.
        unsafe {
            let pointer = Rc::as_ptr(&value);
            assert_eq!(relar_path_exists(pointer), 0);
            assert_eq!(relar_path_is_dir(Rc::as_ptr(&root)), 1);
            let count = Int(relar_path_write_text(pointer, &Str::from("é\r\n🦀\r")));
            assert_eq!(count.as_small(), Some(5));
            assert_eq!(relar_path_is_file(pointer), 1);
            assert_eq!(
                Rc::from_raw(relar_path_read_text(pointer)).as_str(),
                "é\n🦀\n"
            );
            fs::write(&filename, [0xff]).unwrap();
            assert!(relar_path_read_text(pointer).is_null());
            take_error(Kind::UnicodeDecodeError);
            fs::remove_file(&filename).unwrap();
            assert!(relar_path_read_text(pointer).is_null());
            take_error(Kind::FileNotFoundError);
            assert!(relar_path_read_text(Rc::as_ptr(&root)).is_null());
            take_error(Kind::IsADirectoryError);
            let nul = path("a\0b");
            assert_eq!(relar_path_exists(Rc::as_ptr(&nul)), 0);
            assert_eq!(relar_path_is_file(Rc::as_ptr(&nul)), 0);
            assert_eq!(relar_path_is_dir(Rc::as_ptr(&nul)), 0);
            assert!(relar_path_read_text(Rc::as_ptr(&nul)).is_null());
            take_error(Kind::ValueError);
        }
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn mkdir_uses_leaf_mode_and_distinguishes_existing_directories_from_files() {
        use std::os::unix::fs::PermissionsExt;

        let directory = Directory::new();
        let leaf = path(directory.0.join("parent/leaf").to_str().unwrap());
        let mode = Int::small(0o700).unwrap();
        // SAFETY: exact rooted Path/Int values remain live, and every failure is taken.
        unsafe {
            relar_path_mkdir(Rc::as_ptr(&leaf), mode.0, 0, 0);
            take_error(Kind::FileNotFoundError);
            relar_path_mkdir(Rc::as_ptr(&leaf), mode.0, 1, 0);
            assert_eq!(relar_error_pending(), 0);
            assert_eq!(
                fs::metadata(directory.0.join("parent/leaf"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
            assert_eq!(
                fs::metadata(directory.0.join("parent"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777
            );
            relar_path_mkdir(Rc::as_ptr(&leaf), mode.0, 1, 0);
            take_error(Kind::FileExistsError);
            relar_path_mkdir(Rc::as_ptr(&leaf), mode.0, 1, 1);
            assert_eq!(relar_error_pending(), 0);
            symlink("parent/leaf", directory.0.join("link")).unwrap();
            let link = path(directory.0.join("link").to_str().unwrap());
            relar_path_mkdir(Rc::as_ptr(&link), mode.0, 0, 1);
            assert_eq!(relar_error_pending(), 0);
            fs::write(directory.0.join("file"), "kept").unwrap();
            let file = path(directory.0.join("file").to_str().unwrap());
            relar_path_mkdir(Rc::as_ptr(&file), mode.0, 1, 1);
            take_error(Kind::FileExistsError);
            let child = path(directory.0.join("file/child").to_str().unwrap());
            relar_path_mkdir(Rc::as_ptr(&child), mode.0, 1, 1);
            take_error(Kind::NotADirectoryError);
            let huge = Int::new(num_bigint::BigInt::from(1_u8) << 100);
            relar_path_mkdir(Rc::as_ptr(&leaf), huge.0, 1, 1);
            take_error(Kind::OverflowError);
            let nul = path("invalid\0path");
            relar_path_mkdir(Rc::as_ptr(&nul), huge.0, 1, 1);
            take_error(Kind::ValueError);
        }
    }

    #[test]
    fn replacements_overwrite_entries_and_unlink_never_follows_a_symlink() {
        let directory = Directory::new();
        let source = directory.0.join("source");
        let outside = directory.0.join("preserved");
        let target = directory.0.join("target");
        fs::write(&source, "replacement").unwrap();
        fs::write(&outside, "original").unwrap();
        symlink(&outside, &target).unwrap();
        let source_path = path(source.to_str().unwrap());
        let target_text = Str::from(target.to_str().unwrap());
        // SAFETY: exact paths/strings are rooted, and successful results transfer owners.
        unsafe {
            let replaced = relar_path_replace(Rc::as_ptr(&source_path), &target_text);
            assert!(!replaced.is_null());
            let replaced = Rc::from_raw(replaced);
            assert_eq!(relar_error_pending(), 0);
            assert_eq!(
                replaced.0.with(|storage| storage.path().to_owned()),
                target_text.as_str()
            );
            assert!(!source.exists());
            assert_eq!(fs::read_to_string(&target).unwrap(), "replacement");
            assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
            assert!(relar_path_replace(Rc::as_ptr(&source_path), &target_text).is_null());
            take_error(Kind::FileNotFoundError);
            relar_path_unlink(Rc::as_ptr(&replaced), 0);
            relar_path_unlink(Rc::as_ptr(&replaced), 1);
            assert_eq!(relar_error_pending(), 0);
            symlink(&outside, &target).unwrap();
            relar_path_rmdir(Rc::as_ptr(&replaced));
            take_error(Kind::NotADirectoryError);
            relar_path_unlink(Rc::as_ptr(&replaced), 0);
            assert_eq!(fs::read_to_string(outside).unwrap(), "original");
            let non_directory = path(directory.0.join("preserved/child").to_str().unwrap());
            relar_path_unlink(Rc::as_ptr(&non_directory), 1);
            take_error(Kind::NotADirectoryError);
            let root = directory.path();
            relar_path_unlink(Rc::as_ptr(&root), 1);
            take_error(Kind::IsADirectoryError);
        }
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn directory_snapshots_keep_acquisition_timing_and_include_broken_symlinks() {
        let directory = Directory::new();
        fs::write(directory.0.join("old.json"), "").unwrap();
        let root = Str::from(directory.0.to_str().unwrap());
        let mut eager = PathIterator::new(root.clone(), None).unwrap_or_else(|_| panic!());
        let mut lazy = PathIterator::new(root.clone(), Some("*.json")).unwrap_or_else(|_| panic!());
        fs::write(directory.0.join("new.json"), "").unwrap();
        assert_eq!(names(&mut eager), ["old.json"]);
        let first = lazy.next().unwrap_or_else(|_| panic!());
        assert!(!first.is_null());
        // SAFETY: next transfers one Path owner, discarded to test later iteration.
        unsafe {
            relar_object_release(first);
        }
        fs::write(directory.0.join("later.json"), "").unwrap();
        let rest = names(&mut lazy);
        assert_eq!(rest.len(), 1);
        assert_ne!(rest[0], "later.json");
        symlink("missing", directory.0.join("broken")).unwrap();
        let mut literal = PathIterator::new(root, Some("broken")).unwrap_or_else(|_| panic!());
        assert_eq!(names(&mut literal), ["broken"]);
        let absent = Str::from(directory.0.join("absent").to_str().unwrap());
        assert!(PathIterator::new(absent.clone(), None).is_err());
        let mut empty = PathIterator::new(absent, Some("*")).unwrap_or_else(|_| panic!());
        assert!(names(&mut empty).is_empty());
    }

    #[test]
    fn wildcard_patterns_match_unicode_hidden_names_and_character_classes() {
        for (pattern, yes, no) in [
            ("*.json", ".hidden.json", "plain.txt"),
            ("?.json", "🦀.json", "aa.json"),
            ("[a-c]?", "bβ", "dβ"),
            ("[!a-c]", "β", "b"),
            ("[]]", "]", "["),
            ("[!]", "[!]", "!"),
            ("[!b-a]", "x", ""),
            ("[b-a!]", "x", ""),
            ("[a--!a]", "b", "a"),
            ("a*b?c", "aλbbxc", "aλbc"),
            ("[abc", "[abc", "a"),
            ("[-a]", "-", "b"),
            ("[a-]", "-", "b"),
        ] {
            let pattern = Pattern::new(pattern).unwrap_or_else(|_| panic!());
            assert!(pattern.matches(yes), "{} {yes}", pattern.text);
            assert!(!pattern.matches(no), "{} {no}", pattern.text);
        }
    }
}
