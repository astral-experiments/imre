//! Pure paths retain their input segments: nesting a path of the same flavor
//! splices its raw segments, while a foreign path contributes its POSIX spelling.

use std::cell::OnceCell;
use std::rc::Rc;

use super::{Error, Kind, Object, Result, Storage, StorageCell, Str, object};
use crate::objects::exceptions::string_repr;
use crate::objects::text::lowercase;

pub(in crate::objects) struct PurePath {
    pub(super) windows: bool,
    raw: Vec<Rc<Str>>,
    pub(super) drive: Str,
    pub(super) root: Str,
    tail: Vec<Str>,
    pub(super) text: Rc<Str>,
    folded: OnceCell<Str>,
    name: OnceCell<Rc<Str>>,
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

/// Normalize a Windows path comparison key with the same Unicode data as `str.lower()`.
fn windows_lower(source: &str) -> Str {
    lowercase(source).expect("failed to allocate lowercase Windows path")
}

fn win_join(raw: &[Rc<Str>]) -> String {
    let (mut drive, mut root, mut tail) = ("", "", String::new());
    for part in raw {
        let input = part.replace('/', "\\");
        let (d, r, t) = win_split(&input);
        // ntpath.join compares the original drive spellings, including separators.
        let d = &part[..d.len()];
        if !r.is_empty()
            || (!d.is_empty() && d != drive && windows_lower(d) != windows_lower(drive))
        {
            if !d.is_empty() || drive.is_empty() {
                drive = d;
            }
            root = if r.is_empty() { "" } else { "\\" };
            tail = t.to_owned();
        } else {
            if !d.is_empty() {
                drive = d;
            }
            if !tail.is_empty() && !tail.ends_with('\\') {
                tail.push('\\');
            }
            tail.push_str(t);
        }
    }
    let mut path = drive.replace('/', "\\");
    if !tail.is_empty() && root.is_empty() && !path.is_empty() && !path.ends_with([':', '\\']) {
        path.push('\\');
    }
    path.push_str(root);
    path.push_str(&tail);
    path
}

impl PurePath {
    pub(super) fn new(windows: bool, raw: Vec<Rc<Str>>) -> Self {
        let input = if windows {
            win_join(&raw)
        } else {
            let mut input = String::new();
            for part in &raw {
                if part.starts_with('/') {
                    input.clear();
                } else if !input.is_empty() && !input.ends_with('/') {
                    input.push('/');
                }
                input.push_str(part);
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
        let tail = rest
            .split(sep)
            .filter(|p| !p.is_empty() && *p != ".")
            .map(Str::from)
            .collect();
        Self::parsed(windows, raw, drive.into(), root.into(), tail)
    }

    fn parsed(windows: bool, raw: Vec<Rc<Str>>, drive: Str, root: Str, tail: Vec<Str>) -> Self {
        let mut text = drive.clone();
        text.push_str(&root);
        let sep = if windows { '\\' } else { '/' };
        if windows && text.is_empty() && tail.first().is_some_and(|p| !win_split(p).0.is_empty()) {
            text.push_str(".\\");
        }
        for (index, part) in tail.iter().enumerate() {
            if index != 0 {
                text.push(sep);
            }
            text.push_str(part);
        }
        if text.is_empty() {
            text.push('.');
        }
        Self {
            windows,
            raw,
            drive,
            root,
            tail,
            text: Rc::new(text),
            folded: OnceCell::new(),
            name: OnceCell::new(),
        }
    }

    pub(super) fn join(&self, extra: impl IntoIterator<Item = Rc<Str>>) -> Self {
        let mut raw = self.raw.clone();
        raw.extend(extra);
        Self::new(self.windows, raw)
    }

    fn with_tail(&self, tail: Vec<Str>) -> Self {
        let mut p = Self::parsed(
            self.windows,
            vec![],
            self.drive.clone(),
            self.root.clone(),
            tail,
        );
        p.raw.push(Rc::clone(&p.text));
        p
    }

    pub(super) fn parent(&self) -> Self {
        self.with_tail(self.tail[..self.tail.len().saturating_sub(1)].to_vec())
    }

    pub(super) fn name(&self) -> &Rc<Str> {
        self.name
            .get_or_init(|| Rc::new(self.tail.last().cloned().unwrap_or_default()))
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
        let mut tail = self.tail.clone();
        let Some(last) = tail.last_mut() else {
            return Err(Error::new(
                Kind::ValueError,
                format!(
                    "{}({}) has an empty name",
                    self.class(),
                    string_repr(&self.as_posix())
                ),
            ));
        };
        *last = name.clone();
        Ok(self.with_tail(tail).into_object())
    }

    pub(super) fn as_posix(&self) -> Str {
        if self.windows {
            self.text.replace('\\', "/").into()
        } else {
            (*self.text).clone()
        }
    }
    pub(in crate::objects) fn class(&self) -> &'static str {
        if self.windows {
            "PureWindowsPath"
        } else {
            "PurePosixPath"
        }
    }
    pub(super) fn into_object(self) -> *const Object {
        Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::PurePath(
            Box::new(self),
        )))))
    }
}

impl Storage {
    pub(in crate::objects) fn path_key(&self) -> &str {
        match self {
            Self::PurePath(p) if p.windows => p.folded.get_or_init(|| windows_lower(&p.text)),
            _ => self.path(),
        }
    }
    pub(in crate::objects) fn path_equal(&self, other: &Self) -> bool {
        self.windows_path() == other.windows_path() && self.path_key() == other.path_key()
    }
    pub(super) fn windows_path(&self) -> bool {
        matches!(self, Self::PurePath(p) if p.windows)
    }
    pub(in crate::objects) fn posix_spelling(&self) -> Rc<Str> {
        match self {
            Self::PurePath(p) if p.windows => Rc::new(p.as_posix()),
            _ => Rc::clone(self.path_text()),
        }
    }
    pub(super) fn drive_root(&self) -> (&str, &str) {
        match self {
            Self::PurePath(p) => (&p.drive, &p.root),
            _ => {
                let p = self.path();
                ("", &p[..p.len() - p.trim_start_matches('/').len()])
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_pure_path_new(windows: u32) -> *const Object {
    PurePath::new(windows != 0, vec![]).into_object()
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
                (Storage::PurePath(a), Storage::PurePath(b)) if a.windows == b.windows => {
                    Ok(a.join(b.raw.iter().cloned()).into_object())
                }
                _ => Err(b.posix_spelling()),
            })
        })
    };
    result.unwrap_or_else(|child| {
        // SAFETY: the local Rc roots the spelling while the original path stays live.
        unsafe { super::imre_path_join(left, Rc::as_ptr(&child)) }
    })
}

/// # Safety
/// The caller roots an exact concrete or pure path until its spelling is acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_as_posix(path: *const Object) -> *const Str {
    // SAFETY: only an owned immutable spelling escapes.
    unsafe { object(path).0.with(|p| Rc::into_raw(p.posix_spelling())) }
}

/// # Safety
/// The caller roots the immutable path through this read of its drive (0),
/// root (1), or combined anchor (2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_prefix(path: *const Object, part: u32) -> *const Str {
    // SAFETY: the prefix slices stay inside the borrow until an owned result exists.
    unsafe {
        object(path).0.with(|p| {
            let (drive, root) = p.drive_root();
            let text = match part {
                0 => Str::from(drive),
                1 => Str::from(root),
                2 => {
                    let mut s = Str::from(drive);
                    s.push_str(root);
                    s
                }
                _ => unreachable!(),
            };
            Rc::into_raw(Rc::new(text))
        })
    }
}
