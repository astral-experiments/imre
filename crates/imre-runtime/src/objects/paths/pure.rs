//! Pure paths retain their input segments: nesting a path of the same flavor
//! splices its raw segments, while a foreign path contributes its POSIX spelling.

use crate::StrOwner;
use std::cell::OnceCell;
use std::hash::{Hash, Hasher};

use super::{Error, Kind, Object, Result, Storage, StorageCell, Str, object};
use crate::objects::exceptions::string_repr;
use crate::objects::text::{lowercase, lowercase_chars};

pub(in crate::objects) struct PurePath {
    pub(super) windows: bool,
    raw: Vec<StrOwner>,
    pub(super) drive: Str,
    pub(super) root: Str,
    tail: Vec<Str>,
    pub(super) text: StrOwner,
    folded: OnceCell<Str>,
    name: OnceCell<StrOwner>,
}

/// Split a path with normalized backslashes into the drive, root and remainder.
/// A UNC drive includes the server and share; extended UNC uses two more fields.
fn win_split(s: &str) -> (&str, &str, &str) {
    if s.starts_with(r"\\") {
        let start = if s
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("\\\\?\\UNC\\"))
        {
            8
        } else {
            2
        };
        if let Some(server) = s[start..].find('\\') {
            let share = start + server + 1;
            if let Some(end) = s[share..].find('\\') {
                let end = share + end;
                return (&s[..end], "\\", &s[end + 1..]);
            }
        }
        return (s, "", "");
    }
    if let Some(tail) = s.strip_prefix('\\') {
        return ("", "\\", tail);
    }
    let start = s.chars().next().map_or(0, char::len_utf8);
    if s[start..].starts_with(':') {
        let (drive, tail) = s.split_at(start + 1);
        return if let Some(tail) = tail.strip_prefix('\\') {
            (drive, "\\", tail)
        } else {
            (drive, "", tail)
        };
    }
    ("", "", s)
}

fn replace_separator(source: &str, from: char, to: char) -> Result<Str> {
    // Both separators are single-byte ASCII, so the result has exactly the input length.
    let mut text = Str::try_with_capacity(source.len()).map_err(|_| Error::allocation())?;
    for piece in source.split_inclusive(from) {
        if let Some(piece) = piece.strip_suffix(from) {
            text.push_str(piece);
            text.push(to);
        } else {
            text.push_str(piece);
        }
    }
    Ok(text)
}

fn append_segment(text: &mut Str, separator: &str, part: &str) -> Result<()> {
    text.try_reserve(separator.len().saturating_add(part.len()))
        .map_err(|_| Error::allocation())?;
    text.push_str(separator);
    text.push_str(part);
    Ok(())
}

fn copy_components<'a>(parts: impl Iterator<Item = &'a str>) -> Result<Vec<Str>> {
    let mut tail = Vec::new();
    for part in parts {
        tail.try_reserve(1).map_err(|_| Error::allocation())?;
        tail.push(Str::try_new(part).map_err(|_| Error::allocation())?);
    }
    Ok(tail)
}

fn win_join(raw: &[StrOwner]) -> Result<Str> {
    let (mut drive, mut root, mut tail) = ("", "", Str::default());
    for part in raw {
        let input = replace_separator(part, '/', '\\')?;
        let (d, r, t) = win_split(&input);
        // ntpath.join compares the original drive spellings, including separators.
        let d = &part[..d.len()];
        if !r.is_empty()
            || (!d.is_empty()
                && d != drive
                && lowercase(d).map_err(|_| Error::allocation())?
                    != lowercase(drive).map_err(|_| Error::allocation())?)
        {
            if !d.is_empty() || drive.is_empty() {
                drive = d;
            }
            root = if r.is_empty() { "" } else { "\\" };
            tail.clear();
            append_segment(&mut tail, "", t)?;
        } else {
            if !d.is_empty() {
                drive = d;
            }
            let sep = if !tail.is_empty() && !tail.ends_with('\\') {
                "\\"
            } else {
                ""
            };
            append_segment(&mut tail, sep, t)?;
        }
    }
    let mut path = replace_separator(drive, '/', '\\')?;
    let sep = if !tail.is_empty()
        && root.is_empty()
        && !path.is_empty()
        && !path.ends_with([':', '\\'])
    {
        "\\"
    } else {
        ""
    };
    append_segment(&mut path, sep, root)?;
    append_segment(&mut path, "", &tail)?;
    Ok(path)
}

impl PurePath {
    pub(super) fn new(windows: bool, raw: Vec<StrOwner>) -> Result<Self> {
        let input = if windows {
            win_join(&raw)?
        } else {
            let mut input = Str::default();
            for part in &raw {
                if part.starts_with('/') {
                    input.clear();
                }
                let sep = if !input.is_empty() && !input.ends_with('/') {
                    "/"
                } else {
                    ""
                };
                append_segment(&mut input, sep, part)?;
            }
            input
        };
        let (drive, mut root, rest) = if windows {
            win_split(&input)
        } else {
            let n = input.len() - input.trim_start_matches('/').len();
            (
                "",
                match n {
                    0 => "",
                    2 => "//",
                    _ => "/",
                },
                &input[n..],
            )
        };
        let sep = if windows { '\\' } else { '/' };
        if windows && root.is_empty() && drive.starts_with(sep) && !drive.ends_with(sep) {
            let mut fields = drive.split(sep);
            let host = fields.nth(2).unwrap_or_default();
            match fields.count() {
                1 if !matches!(host, "" | "." | "?" | "?.") => root = "\\",
                3 => root = "\\",
                _ => {}
            }
        }
        let tail = copy_components(rest.split(sep).filter(|p| !p.is_empty() && *p != "."))?;
        let drive = Str::try_new(drive).map_err(|_| Error::allocation())?;
        Self::parsed(windows, raw, drive, root.into(), tail)
    }

    fn parsed(
        windows: bool,
        raw: Vec<StrOwner>,
        drive: Str,
        root: Str,
        tail: Vec<Str>,
    ) -> Result<Self> {
        let mut text = Str::try_new(&drive).map_err(|_| Error::allocation())?;
        append_segment(&mut text, "", &root)?;
        let sep = if windows { "\\" } else { "/" };
        if windows && text.is_empty() && tail.first().is_some_and(|p| !win_split(p).0.is_empty()) {
            append_segment(&mut text, "", ".\\")?;
        }
        for (index, part) in tail.iter().enumerate() {
            append_segment(&mut text, if index == 0 { "" } else { sep }, part)?;
        }
        if text.is_empty() {
            text.push('.');
        }
        Ok(Self {
            windows,
            raw,
            drive,
            root,
            tail,
            text: StrOwner::try_new(text).map_err(|_| Error::allocation())?,
            folded: OnceCell::new(),
            name: OnceCell::new(),
        })
    }

    pub(super) fn join(&self, extra: impl ExactSizeIterator<Item = StrOwner>) -> Result<Self> {
        let mut raw = Vec::new();
        raw.try_reserve_exact(self.raw.len().saturating_add(extra.len()))
            .map_err(|_| Error::allocation())?;
        raw.extend(self.raw.iter().cloned());
        raw.extend(extra);
        Self::new(self.windows, raw)
    }

    fn with_tail(&self, tail: Vec<Str>) -> Result<Self> {
        let mut p = Self::parsed(
            self.windows,
            vec![],
            Str::try_new(&self.drive).map_err(|_| Error::allocation())?,
            self.root.clone(),
            tail,
        )?;
        p.raw
            .try_reserve_exact(1)
            .map_err(|_| Error::allocation())?;
        p.raw.push(StrOwner::clone(&p.text));
        Ok(p)
    }

    pub(super) fn parent(&self) -> Result<Self> {
        self.with_tail(copy_components(
            self.tail[..self.tail.len().saturating_sub(1)]
                .iter()
                .map(Str::as_str),
        )?)
    }

    /// Return the same normalized spelling used for Windows path equality.
    fn windows_key(&self) -> Result<&Str> {
        if let Some(key) = self.folded.get() {
            return Ok(key);
        }
        let key = lowercase(&self.text).map_err(|_| Error::allocation())?;
        Ok(self.folded.get_or_init(|| key))
    }

    #[inline]
    fn windows_equal(&self, other: &Self) -> bool {
        if let (Some(left), Some(right)) = (self.folded.get(), other.folded.get()) {
            return left == right;
        }
        self.windows_equal_uncached(other)
    }

    #[cold]
    fn windows_equal_uncached(&self, other: &Self) -> bool {
        if let (Ok(left), Ok(right)) = (self.windows_key(), other.windows_key()) {
            return left == right;
        }
        if self.text.is_ascii() && other.text.is_ascii() {
            return self.text.eq_ignore_ascii_case(&other.text);
        }
        lowercase_chars(&self.text).eq(lowercase_chars(&other.text))
    }

    #[inline]
    fn hash_windows<H: Hasher>(&self, state: &mut H) {
        // Hash must receive the same sequence of writes with and without a cached key.
        const CHUNK: usize = 256;
        if let Some(key) = self.folded.get().or_else(|| self.windows_key().ok()) {
            for bytes in key.as_bytes().chunks(CHUNK) {
                state.write(bytes);
            }
        } else {
            self.hash_windows_uncached::<H, CHUNK>(state);
        }
        state.write_u8(0xff);
    }

    #[cold]
    fn hash_windows_uncached<H: Hasher, const CHUNK: usize>(&self, state: &mut H) {
        let mut bytes = [0; CHUNK];
        let mut len = 0;
        let mut encoded = [0; 4];
        for ch in lowercase_chars(&self.text) {
            for &byte in ch.encode_utf8(&mut encoded).as_bytes() {
                bytes[len] = byte;
                len += 1;
                if len == CHUNK {
                    state.write(&bytes);
                    len = 0;
                }
            }
        }
        if len != 0 {
            state.write(&bytes[..len]);
        }
    }

    /// Byte ends of successively shorter nonempty tails in the normalized spelling.
    fn windows_tail_ends<'a>(&self, key: &'a str) -> impl Iterator<Item = usize> + use<'a> {
        std::iter::once(key.len())
            .chain(key.rmatch_indices('\\').map(|(i, _)| i))
            .take(self.tail.len())
    }

    /// Return the comparison key of the path left when every tail component is removed.
    fn windows_anchor_key(&self) -> Result<Str> {
        let mut key = lowercase(&self.drive).map_err(|_| Error::allocation())?;
        append_segment(&mut key, "", &self.root)?;
        if key.is_empty() {
            key.push('.');
        }
        Ok(key)
    }

    /// Find the closest parent of `other` that compares equal to this path or a
    /// parent. A lowercase expansion can make differently parsed drive prefixes equal.
    fn shared_windows_parent(&self, other: &Self) -> Result<Option<usize>> {
        let left = self.windows_key()?.as_str();
        let right = other.windows_key()?.as_str();
        if left == right {
            return Ok(Some(other.tail.len()));
        }
        let common = left
            .bytes()
            .zip(right.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let anchor = self.windows_anchor_key()?;
        let mut ends = self.windows_tail_ends(left).peekable();
        // Both paths' parent boundaries are ordered from longest to shortest.
        for (step, end) in other.windows_tail_ends(right).enumerate() {
            while ends.next_if(|len| *len > end).is_some() {}
            if (end <= common && ends.peek() == Some(&end)) || &right[..end] == anchor.as_str() {
                return Ok(Some(other.tail.len() - step));
            }
        }
        let base = other.windows_anchor_key()?;
        Ok((anchor == base
            || left.starts_with(base.as_str())
                && self.windows_tail_ends(left).any(|i| i == base.len()))
        .then_some(0))
    }

    fn shared_tail_prefix(&self, other: &Self) -> Result<Option<usize>> {
        if self.windows != other.windows {
            return Ok(None);
        }
        if self.windows {
            return self.shared_windows_parent(other);
        }
        Ok((self.root == other.root).then(|| {
            self.tail
                .iter()
                .zip(&other.tail)
                .take_while(|(a, b)| a == b)
                .count()
        }))
    }

    pub(super) fn is_relative_to(&self, other: &Self) -> Result<bool> {
        Ok(self.shared_tail_prefix(other)? == Some(other.tail.len()))
    }

    pub(super) fn relative_to(&self, other: &Self, walk_up: bool) -> Result<Self> {
        let shared = self.shared_tail_prefix(other)?;
        if shared != Some(other.tail.len()) {
            if !walk_up {
                return Err(Error::new(
                    Kind::ValueError,
                    format!(
                        "{} is not in the subpath of {}",
                        string_repr(&self.text),
                        string_repr(&other.text)
                    ),
                ));
            }
            if other.tail[shared.unwrap_or(0)..]
                .iter()
                .any(|s| s.as_str() == "..")
            {
                return Err(Error::new(
                    Kind::ValueError,
                    format!(
                        "'..' segment in {} cannot be walked",
                        string_repr(&other.text)
                    ),
                ));
            }
        }
        let common = shared.ok_or_else(|| {
            Error::new(
                Kind::ValueError,
                format!(
                    "{} and {} have different anchors",
                    string_repr(&self.text),
                    string_repr(&other.text)
                ),
            )
        })?;
        let tail = copy_components(
            std::iter::repeat_n("..", other.tail.len() - common)
                .chain(self.tail.iter().skip(common).map(Str::as_str)),
        )?;
        let mut path = Self::parsed(self.windows, vec![], Str::default(), Str::default(), tail)?;
        path.raw
            .try_reserve_exact(1)
            .map_err(|_| Error::allocation())?;
        path.raw.push(StrOwner::clone(&path.text));
        Ok(path)
    }

    pub(super) fn name(&self) -> Result<&StrOwner> {
        super::cached_name(&self.name, || super::try_owned_string(self.basename()))
    }

    pub(super) fn basename(&self) -> &str {
        self.tail.last().map_or("", |s| s.as_str())
    }

    pub(super) fn count_part(&self, part: &str) -> usize {
        let anchor =
            part.strip_prefix(self.drive.as_str()) == Some(self.root.as_str()) && !part.is_empty();
        usize::from(anchor) + self.tail.iter().filter(|s| s.as_str() == part).count()
    }

    pub(super) fn is_absolute(&self) -> bool {
        if self.windows {
            self.text.starts_with(r"\\") || (!self.root.is_empty() && !self.drive.is_empty())
        } else {
            !self.root.is_empty()
        }
    }

    pub(super) fn with_name(&self, name: &Str) -> Result<*const Object> {
        if name.is_empty()
            || name.as_str() == "."
            || name.contains('/')
            || self.windows && name.contains('\\')
        {
            return Err(Error::new(
                Kind::ValueError,
                format!("Invalid name {}", string_repr(name)),
            ));
        }
        let mut tail = copy_components(self.tail.iter().map(Str::as_str))?;
        let Some(last) = tail.last_mut() else {
            return Err(Error::new(
                Kind::ValueError,
                format!(
                    "{}({}) has an empty name",
                    self.class(),
                    string_repr(self.as_posix()?.as_str())
                ),
            ));
        };
        *last = Str::try_new(name).map_err(|_| Error::allocation())?;
        self.with_tail(tail)?.into_object()
    }

    pub(super) fn as_posix(&self) -> Result<StrOwner> {
        if self.windows && self.text.contains('\\') {
            Ok(StrOwner::new(replace_separator(&self.text, '\\', '/')?))
        } else {
            Ok(StrOwner::clone(&self.text))
        }
    }
    pub(in crate::objects) fn write_posix_repr(
        &self,
        output: &mut crate::objects::values::ReprWriter<'_>,
    ) -> std::fmt::Result {
        if self.windows && self.text.contains('\\') {
            crate::objects::exceptions::write_windows_path_repr(&self.text, output)
        } else {
            crate::objects::exceptions::write_quoted_repr(&self.text, output)
        }
    }

    pub(in crate::objects) fn class(&self) -> &'static str {
        if self.windows {
            "PureWindowsPath"
        } else {
            "PurePosixPath"
        }
    }
    pub(super) fn into_object(self) -> Result<*const Object> {
        let path = allocator_api2::boxed::Box::try_new(self).map_err(|_| Error::allocation())?;
        crate::ObjectOwner::try_new(Object(StorageCell::new(Storage::PurePath(path))))
            .map(crate::ObjectOwner::into_raw)
            .map_err(|_| Error::allocation())
    }
}

impl Storage {
    #[inline]
    pub(in crate::objects) fn hash_path<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::PurePath(path) if path.windows => path.hash_windows(state),
            _ => self.path().hash(state),
        }
    }
    #[inline]
    pub(in crate::objects) fn path_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::PurePath(left), Self::PurePath(right)) if left.windows && right.windows => {
                left.windows_equal(right)
            }
            _ => self.windows_path() == other.windows_path() && self.path() == other.path(),
        }
    }
    pub(super) fn windows_path(&self) -> bool {
        matches!(self, Self::PurePath(p) if p.windows)
    }
    pub(in crate::objects) fn posix_spelling(&self) -> Result<StrOwner> {
        match self {
            Self::PurePath(p) => p.as_posix(),
            _ => Ok(StrOwner::clone(self.path_text())),
        }
    }
    pub(super) fn drive_root(&self) -> (&str, &str) {
        match self {
            Self::PurePath(p) => (&p.drive, &p.root),
            _ => {
                let p = self.component_text();
                ("", &p[..p.len() - p.trim_start_matches('/').len()])
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_pure_path_new(windows: u32) -> *const Object {
    super::finish(
        PurePath::new(windows != 0, vec![]).and_then(PurePath::into_object),
        std::ptr::null(),
    )
}

/// # Safety
/// Both immutable paths are rooted. A same-flavor pure path contributes its raw
/// segments; crossing flavors uses the portable spelling of the right operand.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_join_object(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    // SAFETY: immutable inputs stay live even when both operands are the same object.
    let result = unsafe {
        object(left).0.with(|a| {
            object(right).0.with(|b| match (a, b) {
                (Storage::PurePath(a), Storage::PurePath(b)) if a.windows == b.windows => a
                    .join(b.raw.iter().cloned())
                    .and_then(PurePath::into_object),
                _ => {
                    let child = b.posix_spelling()?;
                    // SAFETY: the local Rc roots the spelling while the original path stays live.
                    Ok(super::imre_path_join(left, StrOwner::as_ptr(&child)))
                }
            })
        })
    };
    super::finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots an exact concrete or pure path until its spelling is acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_as_posix(path: *const Object) -> *const Str {
    // SAFETY: only an owned immutable spelling escapes.
    let spelling = unsafe { object(path).0.with(Storage::posix_spelling) };
    super::finish(spelling.map(StrOwner::into_raw), std::ptr::null())
}

/// # Safety
/// The caller roots the immutable path through this read of its drive (0),
/// root (1), or combined anchor (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_prefix(path: *const Object, part: u32) -> *const Str {
    // SAFETY: the prefix slices stay inside the borrow until an owned result exists.
    let result = unsafe {
        object(path).0.with(|p| {
            let (drive, root) = p.drive_root();
            let (first, second) = match part {
                0 => (drive, ""),
                1 => (root, ""),
                2 => (drive, root),
                _ => unreachable!(),
            };
            let mut text = Str::try_with_capacity(first.len().saturating_add(second.len()))
                .map_err(|_| Error::allocation())?;
            text.push_str(first);
            text.push_str(second);
            StrOwner::try_new(text).map_err(|_| Error::allocation())
        })
    };
    super::finish(result.map(StrOwner::into_raw), std::ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Writes(Vec<Vec<u8>>);

    impl Hasher for Writes {
        fn finish(&self) -> u64 {
            0
        }

        fn write(&mut self, value: &[u8]) {
            self.0.push(value.to_vec());
        }
    }

    #[test]
    fn cached_and_streamed_windows_hashes_use_the_same_byte_boundaries() {
        for prefix in [254, 255, 256, 257] {
            let text = format!("{}İ𐐀\0AΣ", "Z".repeat(prefix));
            let Ok(path) = PurePath::new(true, vec![StrOwner::new(text.into())]) else {
                panic!("test path allocation failed")
            };
            let mut cached = Writes::default();
            path.hash_windows(&mut cached);
            let mut streamed = Writes::default();
            path.hash_windows_uncached::<_, 256>(&mut streamed);
            streamed.write_u8(0xff);
            assert_eq!(cached.0, streamed.0);
        }
    }
}
