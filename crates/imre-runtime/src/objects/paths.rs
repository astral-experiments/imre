//! Sealed POSIX paths with lexical normalization and snapshot directory iterators.
//! Path objects are immutable; directory state owns names, never open descriptors.

use crate::{StrOwner, WeakStr};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::exceptions::{Error, Filename, Kind, Result, finish};
use super::{Bytes, Int, Object, Storage, StorageCell, Str, fail, files, object};

mod components;
mod matching;
mod metadata;
mod pure;
mod traversal;
mod walking;
pub(super) use metadata::Info;
pub(super) use pure::PurePath;
pub(super) use walking::Walker;

#[derive(Clone)]
pub(super) enum PathText {
    Flat(StrOwner),
    /// Like pathlib's _from_parsed_string: str and filesystem calls keep the
    /// supplied spelling, while name, parent and joins use parsed components.
    Parsed {
        spelling: StrOwner,
        components: StrOwner,
    },
    WithParent {
        text: StrOwner,
        parent: WeakStr,
    },
    /// Keep readdir's type alongside the spelling until someone asks for info.
    Scanned {
        spelling: StrOwner,
        components: Option<StrOwner>,
        parent: WeakStr,
        kind: rustix::fs::FileType,
    },
    /// Only paths whose info was requested pay for the persistent view.
    WithInfo {
        lexical: Box<PathText>,
        view: crate::ObjectOwner,
    },
    Joined {
        parts: JoinedParts,
        spelling: std::cell::OnceCell<StrOwner>,
    },
}

#[derive(Clone)]
pub(super) struct JoinedParts {
    parent: Prefix,
    child: StrOwner,
    absolute: bool,
}

#[derive(Clone)]
enum Prefix {
    Empty,
    Flat(StrOwner),
    Joined(Rc<JoinedParts>),
}

impl PathText {
    fn from_entry(spelling: Str, kind: rustix::fs::FileType) -> Self {
        let (spelling, components) = match Self::from_parsed_string(spelling) {
            Self::Flat(s) => (s, None),
            Self::Parsed {
                spelling,
                components,
            } => (spelling, Some(components)),
            _ => unreachable!(),
        };
        Self::Scanned {
            spelling,
            components,
            parent: WeakStr::default(),
            kind,
        }
    }

    fn info(&mut self) -> crate::ObjectOwner {
        if let Self::WithInfo { view, .. } = self {
            return crate::ObjectOwner::clone(view);
        }
        let entry = if let Self::Scanned { kind, .. } = self {
            Some(*kind)
        } else {
            None
        };
        let spelling = StrOwner::clone(self.spelling());
        let view = Info::object(StrOwner::clone(&spelling), entry);
        let lexical = std::mem::replace(self, Self::Flat(spelling));
        *self = Self::WithInfo {
            lexical: Box::new(lexical),
            view: crate::ObjectOwner::clone(&view),
        };
        view
    }

    fn from_parsed_string(spelling: Str) -> Self {
        if spelling.is_empty() {
            Self::Flat(StrOwner::new(Str::from(".")))
        } else if is_normalized(&spelling) {
            Self::Flat(StrOwner::new(spelling))
        } else {
            Self::Parsed {
                components: StrOwner::new(normalize(&spelling)),
                spelling: StrOwner::new(spelling),
            }
        }
    }

    fn components(&self) -> &StrOwner {
        match self {
            Self::Parsed { components, .. } => components,
            Self::Scanned {
                spelling,
                components,
                ..
            } => components.as_ref().unwrap_or(spelling),
            Self::WithInfo { lexical, .. } => lexical.components(),
            _ => self.spelling(),
        }
    }

    fn parent(&mut self) -> StrOwner {
        if let Self::WithInfo { lexical, .. } = self {
            return lexical.parent();
        }
        if let Self::Scanned {
            spelling,
            components,
            parent: cached,
            ..
        } = self
        {
            if let Some(p) = cached.upgrade() {
                return p;
            }
            let p = StrOwner::new(Str::from(parent(components.as_ref().unwrap_or(spelling))));
            if p.len() > size_of::<Str>() {
                *cached = StrOwner::downgrade(&p);
            }
            return p;
        }
        if let Self::Parsed { components, .. } = self {
            return StrOwner::new(Str::from(parent(components)));
        }
        if let Self::WithParent { parent, .. } = self
            && let Some(shared) = parent.upgrade()
        {
            return shared;
        }
        let text = self.spelling();
        let shared = StrOwner::new(Str::from(parent(text)));
        if shared.len() > size_of::<Str>() {
            // Derived paths own the spelling. Discarding the last derived
            // owner frees the string even while the source path stays rooted.
            *self = Self::WithParent {
                text: StrOwner::clone(text),
                parent: StrOwner::downgrade(&shared),
            };
        }
        shared
    }

    fn cached(&self) -> Option<&StrOwner> {
        match self {
            Self::Flat(text) | Self::WithParent { text, .. } => Some(text),
            Self::Parsed { components, .. } => Some(components),
            Self::Scanned { .. } => Some(self.components()),
            Self::WithInfo { lexical, .. } => lexical.cached(),
            Self::Joined { spelling, .. } => spelling.get(),
        }
    }

    fn prefix(&self) -> Prefix {
        match self {
            Self::Flat(text) | Self::WithParent { text, .. } => Prefix::Flat(StrOwner::clone(text)),
            Self::Parsed { components, .. } => Prefix::Flat(StrOwner::clone(components)),
            Self::Scanned { .. } => Prefix::Flat(StrOwner::clone(self.components())),
            Self::WithInfo { lexical, .. } => lexical.prefix(),
            Self::Joined { parts, spelling } => match spelling.get() {
                Some(text) => Prefix::Flat(StrOwner::clone(text)),
                None => Prefix::Joined(Rc::new(parts.clone())),
            },
        }
    }

    fn is_absolute(&self) -> bool {
        match self {
            Self::Flat(text) | Self::WithParent { text, .. } => text.starts_with('/'),
            Self::Parsed { components, .. } => components.starts_with('/'),
            Self::Scanned { .. } => self.components().starts_with('/'),
            Self::WithInfo { lexical, .. } => lexical.is_absolute(),
            Self::Joined { parts, .. } => parts.absolute,
        }
    }

    fn spelling(&self) -> &StrOwner {
        match self {
            Self::Flat(text) | Self::WithParent { text, .. } => text,
            Self::Parsed { spelling, .. } => spelling,
            Self::Scanned { spelling, .. } => spelling,
            Self::WithInfo { lexical, .. } => lexical.spelling(),
            Self::Joined { parts, spelling } => spelling.get_or_init(|| parts.render()),
        }
    }
}

impl JoinedParts {
    fn render(&self) -> StrOwner {
        match &self.parent {
            Prefix::Empty => {
                return if is_normalized(&self.child) {
                    StrOwner::clone(&self.child)
                } else {
                    StrOwner::new(normalize(&self.child))
                };
            }
            Prefix::Flat(base) => return StrOwner::new(join(base, &self.child)),
            Prefix::Joined(_) => {}
        }

        // Collect unrendered components without materializing ancestor spellings.
        let mut children = vec![self.child.as_str()];
        let mut prefix = &self.parent;
        while let Prefix::Joined(parts) = prefix {
            children.push(parts.child.as_str());
            prefix = &parts.parent;
        }
        let mut result = Str::default();
        if let Prefix::Flat(base) = prefix
            && base.as_str() != "."
        {
            result.push_str(base);
        }
        for child in children.into_iter().rev() {
            append_components(&mut result, child);
        }
        if result.is_empty() {
            result.push('.');
        }
        StrOwner::new(result)
    }
}

impl Drop for JoinedParts {
    fn drop(&mut self) {
        // Release uniquely owned ancestry iteratively, including unrendered paths.
        let mut parent = std::mem::replace(&mut self.parent, Prefix::Empty);
        while let Prefix::Joined(node) = parent {
            let Some(mut parts) = Rc::into_inner(node) else {
                break;
            };
            parent = std::mem::replace(&mut parts.parent, Prefix::Empty);
        }
    }
}

impl Storage {
    pub(super) fn path(&self) -> &str {
        self.path_text()
    }

    fn basename(&self) -> &str {
        match self {
            Self::PurePath(p) => p.basename(),
            _ => name(self.path_value().components()),
        }
    }

    fn path_name(&self) -> &StrOwner {
        if let Self::PurePath(p) = self {
            return p.name();
        }
        let Self::Path { name: cached, .. } = self else {
            fail("SystemError", "expected a path object");
        };
        cached.get_or_init(|| {
            let text = self.path_value().components();
            let component = name(text);
            if component.len() == text.len() {
                StrOwner::clone(text)
            } else {
                StrOwner::new(Str::from(component))
            }
        })
    }

    pub(super) fn path_text(&self) -> &StrOwner {
        match self {
            Self::PurePath(p) => &p.text,
            _ => self.path_value().spelling(),
        }
    }

    fn component_text(&self) -> &StrOwner {
        match self {
            Self::PurePath(p) => &p.text,
            _ => self.path_value().components(),
        }
    }

    fn path_value(&self) -> &PathText {
        match self {
            Self::Path { text, .. } => text,
            _ => fail("SystemError", "expected a path object"),
        }
    }
}

/// A canonical spelling can share the input string's immutable owner. Only
/// the one- and two-slash roots and the current-directory marker stand alone.
fn is_normalized(path: &str) -> bool {
    if matches!(path, "." | "/" | "//") {
        return true;
    }
    let tail = path
        .strip_prefix("//")
        .or_else(|| path.strip_prefix('/'))
        .unwrap_or(path);
    tail.split('/').all(|part| !part.is_empty() && part != ".")
}

/// POSIX normalization removes empty and dot components while retaining `..`
/// and exactly two leading slashes, which may have implementation-defined meaning.
pub(super) fn normalize(path: &str) -> Str {
    let mut result = Str::default();
    append_components(&mut result, path);
    if result.is_empty() {
        result.push('.');
    }
    result
}

fn append_components(result: &mut Str, path: &str) {
    if path.starts_with('/') {
        result.clear();
        result.push('/');
        if path.starts_with("//") && !path.starts_with("///") {
            result.push('/');
        }
    }
    for part in path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(part);
    }
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
    // The parent is already normalized. A non-dot canonical relative child
    // introduces no component that needs rewriting across the boundary.
    if child != "." && is_normalized(child) {
        joined
    } else {
        normalize(&joined)
    }
}

fn new_path(path: impl Into<StrOwner>) -> *const Object {
    path_with_text(PathText::Flat(path.into()))
}

fn path_with_text(text: PathText) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Path {
            text,
            name: std::cell::OnceCell::new(),
        },
    ))))
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
        let directory = std::env::current_dir().map_err(files::errno_error)?;
        path_utf8(directory.into_os_string())?
    };
    let mut source = path.split('/');
    let mut remaining = source.clone().count();
    // Only expanded symlink targets and their completion markers need a stack.
    let mut parts = Vec::new();
    let mut links: HashMap<Str, Option<Str>> = HashMap::new();
    while let Some(part) = parts.pop().or_else(|| {
        source
            .next()
            .map(|part| ResolvePart::Component(Str::from(part)))
    }) {
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
        check_nul(&next, "lstat: embedded null character in path")?;
        let component_error = |error| files::errno_with_filename(error, Some(&next));
        let metadata = match fs::symlink_metadata(next.as_str()) {
            Ok(metadata) => metadata,
            Err(error) if strict => return Err(component_error(error)),
            Err(_) => {
                resolved = next;
                continue;
            }
        };
        if !metadata.file_type().is_symlink() {
            if strict && remaining != 0 && !metadata.is_dir() {
                return Err(component_error(io::Error::from_raw_os_error(libc::ENOTDIR)));
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
                    return Err(component_error(io::Error::from_raw_os_error(libc::ELOOP)));
                }
                resolved = next;
                continue;
            }
            None => {}
        }
        let target = match fs::read_link(next.as_str()) {
            Ok(target) => path_utf8(target.into_os_string())?,
            Err(error) if strict => return Err(component_error(error)),
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

pub(super) fn path_utf8(path: OsString) -> Result<Str> {
    path.into_string()
        .map(Str::from)
        .map_err(|_| Error::new(Kind::UnicodeDecodeError, "resolved path is not valid UTF-8"))
}

/// Returns the current POSIX working directory as a new owned string.
#[unsafe(no_mangle)]
pub extern "C" fn imre_getcwd() -> *const Str {
    let result = std::env::current_dir()
        .map_err(files::errno_error)
        .and_then(|cwd| path_utf8(cwd.into_os_string()))
        .map(|text| StrOwner::into_raw(StrOwner::new(text)));
    finish(result, std::ptr::null())
}

fn suffix_start(name: &str) -> Option<usize> {
    let index = name.rfind('.')?;
    name[..index]
        .chars()
        .any(|character| character != '.')
        .then_some(index)
}

/// # Safety
/// `text` is a live `StrOwner` handle from the string ABI. The result transfers
/// one Path owner and may retain the input's canonical spelling.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_new(text: *const Str) -> *const Object {
    // SAFETY: the caller roots the immutable source through normalization.
    let spelling = unsafe { &*text };
    if is_normalized(spelling) {
        // SAFETY: the rooted input remains alive while its shared owner is acquired.
        new_path(unsafe { crate::StrOwner::acquire_word(text as usize) })
    } else {
        new_path(normalize(spelling))
    }
}

/// # Safety
/// The word is a rooted str | Path value. Both branches return an owned string
/// without resolving links or modifying the path's lexical spelling.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_union_path_text(value: usize) -> *const Str {
    // SAFETY: clearing tag six restores the exact pathlib object address.
    unsafe {
        if value & 7 == 6 {
            imre_path_text((value & !7) as *const Object)
        } else {
            StrOwner::into_raw(crate::StrOwner::acquire_word(value))
        }
    }
}

/// # Safety
/// The unnormalized path spelling is a live string through the filesystem call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_text_exists(path: *const Str) -> u32 {
    // SAFETY: The path string is borrowed through the stat call.
    u32::from(fs::metadata(unsafe { &*path }.as_str()).is_ok())
}

/// # Safety
/// Root the exact Path and string throughout the call. Absolute children replace
/// the left path; otherwise both operands are joined without resolving symlinks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_join(path: *const Object, child: *const Str) -> *const Object {
    // SAFETY: the caller roots both operands until the result owns its child
    // and, for a relative component, its immutable parent spelling.
    unsafe {
        object(path).0.with(|storage| {
            if let Storage::PurePath(p) = storage {
                return finish(
                    p.join(std::iter::once(crate::StrOwner::acquire_word(
                        child as usize,
                    )))
                    .map(PurePath::into_object),
                    std::ptr::null(),
                );
            }
            let parent = storage.path_value();
            let child_text = &*child;
            if matches!(child_text.as_str(), "" | ".") {
                if let PathText::Parsed { components, .. } = parent {
                    return new_path(StrOwner::clone(components));
                }
                return path_with_text(parent.clone());
            }
            if let Some(base) = parent.cached()
                && base.len().saturating_add(child_text.len()) < size_of::<Str>()
            {
                return new_path(join(base, child_text));
            }
            let child_absolute = child_text.starts_with('/');
            StrOwner::increment_strong_count(child);
            path_with_text(PathText::Joined {
                parts: JoinedParts {
                    parent: if child_absolute {
                        Prefix::Empty
                    } else {
                        parent.prefix()
                    },
                    child: StrOwner::from_raw(child),
                    absolute: child_absolute || parent.is_absolute(),
                },
                spelling: std::cell::OnceCell::new(),
            })
        })
    }
}

/// # Safety
/// Root the path and any non-null replacement component. Null represents Python None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_with_name(
    path: *const Object,
    replacement: *const Str,
) -> *const Object {
    // SAFETY: both inputs are rooted and lexical operations cannot invoke callbacks.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| storage.replace_name(replacement.as_ref()))
    };
    finish(result, std::ptr::null())
}

/// Count exact (case-sensitive) components, including a nonempty anchor.
///
/// # Safety
/// The caller roots the native path and comparison string until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_count_part(path: *const Object, part: *const Str) -> usize {
    // SAFETY: both inputs remain rooted, and component comparison cannot call Python.
    let count = unsafe {
        object(path).0.with(|storage| {
            let part = (&*part).as_str();
            if let Storage::PurePath(p) = storage {
                return p.count_part(part);
            }
            let path = storage.component_text();
            let tail = path.trim_start_matches('/');
            let anchor = &path[..path.len() - tail.len()];
            usize::from(!anchor.is_empty() && part == anchor)
                + tail
                    .split('/')
                    .filter(|s| !s.is_empty() && *s != "." && *s == part)
                    .count()
        })
    };
    Int::from_usize(count).into_raw()
}

/// # Safety
/// Both arguments are rooted exact Paths; comparison borrows immutable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_equal(left: *const Object, right: *const Object) -> u32 {
    if left == right {
        return 1;
    }
    // SAFETY: no mutation or callback overlaps these immutable storage borrows.
    unsafe {
        object(left).0.with(|left| {
            object(right)
                .0
                .with(|right| u32::from(left.path_equal(right)))
        })
    }
}

/// # Safety
/// Root an exact Path. The result transfers one owned string with its spelling.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_text(path: *const Object) -> *const Str {
    // SAFETY: the path roots its immutable spelling until the result acquires it.
    let text = unsafe {
        object(path)
            .0
            .with(|storage| StrOwner::clone(storage.path_text()))
    };
    StrOwner::into_raw(text)
}

/// # Safety
/// Root an exact Path. The result transfers its final component as an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_name(path: *const Object) -> *const Str {
    // SAFETY: spelling is fixed. The one per-path cached component and the
    // returned handle each own their reference across subsequent operations.
    let text = unsafe {
        object(path)
            .0
            .with(|storage| StrOwner::clone(storage.path_name()))
    };
    StrOwner::into_raw(text)
}

/// # Safety
/// Root an exact Path. Python 3.14 recognizes a trailing dot as a suffix, while
/// names consisting entirely of dots and leading-dot names have no such suffix.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_stem(path: *const Object) -> *const Str {
    // SAFETY: the rooted path owns its spelling and cached basename.
    let stem = unsafe {
        object(path).0.with(|storage| {
            let component = storage.basename();
            match suffix_start(component) {
                Some(end) => StrOwner::new(Str::from(&component[..end])),
                None => StrOwner::clone(storage.path_name()),
            }
        })
    };
    StrOwner::into_raw(stem)
}

/// # Safety
/// Root an exact Path. The result includes the final dot or is an empty string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_suffix(path: *const Object) -> *const Str {
    // SAFETY: only an owned substring escapes this immutable storage access.
    let text = unsafe {
        object(path).0.with(|storage| {
            let name = storage.basename();
            Str::from(&name[suffix_start(name).unwrap_or(name.len())..])
        })
    };
    StrOwner::into_raw(StrOwner::new(text))
}

/// # Safety
/// Root an exact Path with no overlapping storage borrow. This lexical parent
/// never follows symlinks or collapses
/// `..`; roots and the current-directory path are their own parents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_parent(path: *const Object) -> *const Object {
    // SAFETY: the rooted exact Path is accessed exclusively. Computing and
    // caching its lexical parent never invokes user code or borrows another Path.
    unsafe {
        object(path).0.with_mut(|storage| {
            if storage.basename().is_empty() {
                crate::ObjectOwner::increment_strong_count(path);
                return path;
            }
            if let Storage::PurePath(p) = storage {
                return finish(p.parent().map(PurePath::into_object), std::ptr::null());
            }
            let Storage::Path { text, .. } = storage else {
                fail("SystemError", "expected a path object");
            };
            new_path(text.parent())
        })
    }
}

/// # Safety
/// Root an exact Path; this lexical predicate cannot fail or invoke callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_is_absolute(path: *const Object) -> u32 {
    // SAFETY: the immutable rooted path is borrowed only for the predicate.
    unsafe {
        object(path).0.with(|storage| {
            u32::from(match storage {
                Storage::PurePath(p) => p.is_absolute(),
                _ => storage.path_value().is_absolute(),
            })
        })
    }
}

/// Anchors a relative path to the working directory, preserving symlinks and `..`.
///
/// # Safety
/// The input remains rooted for the call. Success returns an owned path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_absolute(path: *const Object) -> *const Object {
    // SAFETY: the input stays live while its immutable spelling is borrowed.
    let result = unsafe {
        object(path).0.with(|storage| {
            if storage.path().starts_with('/') {
                crate::ObjectOwner::increment_strong_count(path);
                return Ok(path);
            }
            let cwd = std::env::current_dir().map_err(files::errno_error)?;
            Ok(new_path(join(
                &path_utf8(cwd.into_os_string())?,
                storage.path(),
            )))
        })
    };
    finish(result, std::ptr::null())
}

/// # Safety
/// Root an exact Path with clear pending error state. Success transfers a new
/// absolute Path owner; strict lookup failures preserve their OS error category.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_resolve(path: *const Object, strict: u32) -> *const Object {
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
pub unsafe extern "C" fn imre_path_exists(path: *const Object) -> u32 {
    // SAFETY: filesystem queries have no callbacks; only a boolean escapes.
    unsafe {
        object(path)
            .0
            .with(|storage| u32::from(fs::metadata(storage.path()).is_ok()))
    }
}

/// Test the target of a filesystem spelling, following symlinks and suppressing stat errors.
pub(crate) fn is_file(path: &std::path::Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// # Safety
/// Root an exact Path. This follows symlinks and suppresses metadata errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_is_file(path: *const Object) -> u32 {
    // SAFETY: filesystem queries have no callbacks; only a boolean escapes.
    unsafe {
        object(path)
            .0
            .with(|storage| u32::from(is_file(storage.path().as_ref())))
    }
}

/// # Safety
/// Root an exact Path. This follows symlinks and suppresses metadata errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_is_dir(path: *const Object) -> u32 {
    // SAFETY: filesystem queries have no callbacks; only a boolean escapes.
    unsafe {
        object(path).0.with(|storage| {
            u32::from(fs::metadata(storage.path()).is_ok_and(|metadata| metadata.is_dir()))
        })
    }
}

/// # Safety
/// Root an exact Path with clear pending error state. Strict UTF-8 reads perform
/// universal-newline conversion and close the descriptor before returning.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_read_text(path: *const Object) -> *const Str {
    // SAFETY: I/O cannot reenter; the result owns its string or error.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| files::read_path_text(storage.path()))
    };
    finish(
        result.map(|text| StrOwner::into_raw(StrOwner::new(text))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root an exact Path and string with clear pending error state. Success returns
/// the canonical integer character count after flushing and closing the file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_text(path: *const Object, text: *const Str) -> usize {
    // SAFETY: both immutable roots live through the complete synchronous write.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| files::write_path_text(storage.path(), &*text))
    };
    finish(result, 0)
}

/// # Safety
/// Root an exact Path until the file closes and its owned bytes or error are returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_read_bytes(path: *const Object) -> *const Bytes {
    // SAFETY: the synchronous read cannot call back or invalidate the pathname.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| files::read_path_bytes(storage.path()))
    };
    finish(
        result.map(|bytes| crate::BytesOwner::into_raw(crate::BytesOwner::new(bytes))),
        std::ptr::null(),
    )
}

/// # Safety
/// Root the Path and bytes through writing and closing. Success returns the byte count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_write_bytes(path: *const Object, data: *const Bytes) -> usize {
    // SAFETY: synchronous output only borrows these immutable values.
    let result = unsafe {
        object(path)
            .0
            .with(|storage| files::write_path_bytes(storage.path(), &*data))
    };
    finish(result, 0)
}

fn check_nul(path: &str, message: &'static str) -> Result<()> {
    if path.contains('\0') {
        Err(Error::new(Kind::ValueError, message))
    } else {
        Ok(())
    }
}

/// # Safety
/// The caller roots an exact Path. Its spelling is retained if readlink fails;
/// a successful call transfers a fresh Path containing the stored link target.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_readlink(path: *const Object) -> *const Object {
    // SAFETY: the caller keeps the original Path rooted through conversion.
    let result = unsafe { super::os::links::readlink(path) }.and_then(|target| {
        let target = target.to_str().ok_or_else(|| {
            Error::new(
                Kind::UnicodeDecodeError,
                "symlink target is not valid UTF-8",
            )
        })?;
        Ok(new_path(normalize(target)))
    });
    finish(result, std::ptr::null())
}

/// # Safety
/// The caller roots the destination Path and exact str or bytes target matching
/// the tag. Both original spellings are retained by any resulting OS exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_link(path: *const Object, word: usize, tag: u32, kind: u32) {
    // SAFETY: the caller roots both path operands and supplies their representation.
    finish(unsafe { super::os::links::link(path, word, tag, kind) }, ());
}

/// Match pathlib's retry order: create the leaf first, make missing parents with
/// their default mode, then retry each child without creating its parents again.
fn mkdir(path: &str, mode: u32, parents: bool, exist_ok: bool) -> Result<()> {
    let mut pending = Vec::new();
    let (mut current, mut current_mode, mut current_exists) = (path, mode, exist_ok);
    loop {
        match fs::DirBuilder::new().mode(current_mode).create(current) {
            Ok(()) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let next = parent(current);
                if !parents || next == current {
                    return Err(files::errno_with_filename(error, Some(current)));
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
            Err(error) => return Err(files::errno_with_filename(error, Some(current))),
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
pub unsafe extern "C" fn imre_path_mkdir(
    path: *const Object,
    mode: usize,
    parents: u32,
    exist_ok: u32,
) {
    // SAFETY: both arguments stay rooted through validation and filesystem I/O.
    let result = unsafe {
        object(path).0.with(|storage| {
            let path = storage.path();
            check_nul(path, "mkdir: embedded null character in path")?;
            let mode = Int::borrow(mode).big().to_i32().ok_or_else(|| {
                Error::new(
                    Kind::OverflowError,
                    "Python int too large to convert to C int",
                )
            })?;
            mkdir(path, mode as u32, parents != 0, exist_ok != 0)
        })
    };
    finish(result, ());
}

/// # Safety
/// Root an exact Path with clear pending error state. Missing-ok suppresses only
/// a missing entry; directories and non-directory ancestors retain their errors.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_unlink(path: *const Object, missing_ok: u32) {
    // SAFETY: unlink has no language callbacks and the immutable path stays live.
    let result = unsafe {
        object(path).0.with(|storage| {
            let path = storage.path();
            check_nul(path, "unlink: embedded null character in path")?;
            match fs::remove_file(path) {
                Err(error) if missing_ok != 0 && error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result.map_err(|error| files::errno_with_filename(error, Some(path))),
            }
        })
    };
    finish(result, ());
}

/// # Safety
/// Root an exact Path with clear pending error state. Only an empty directory is
/// removed; symlinks are never followed by the final directory-removal operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_rmdir(path: *const Object) {
    // SAFETY: the rooted spelling is borrowed only for synchronous I/O.
    let result = unsafe {
        object(path).0.with(|storage| {
            let path = storage.path();
            check_nul(path, "rmdir: embedded null character in path")?;
            fs::remove_dir(path).map_err(|error| files::errno_with_filename(error, Some(path)))
        })
    };
    finish(result, ());
}

/// # Safety
/// Root an exact Path and target string with clear pending error state. A
/// successful rename transfers one new Path owner; failure returns null. Relative
/// targets use the process cwd, and target spelling is normalized only afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_replace(
    path: *const Object,
    target: *const Str,
) -> *const Object {
    // SAFETY: the roots remain live throughout rename and target normalization.
    let result = unsafe {
        object(path).0.with(|storage| {
            check_nul(storage.path(), "replace: embedded null character in src")?;
            check_nul(&*target, "replace: embedded null character in dst")?;
            fs::rename(storage.path(), (&*target).as_str()).map_err(|error| {
                files::errno_with_filenames(
                    error,
                    Some(Filename::Str(StrOwner::clone(storage.path_text()))),
                    Some(Filename::Str(StrOwner::new((*target).clone()))),
                )
            })?;
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
    Glob(traversal::Glob),
    Snapshot(std::vec::IntoIter<(OsString, rustix::fs::FileType)>),
    Exhausted,
}

fn check_directory_path(root: &str) -> Result<()> {
    if root.contains('\0') {
        return Err(Error::new(
            Kind::ValueError,
            "scandir: embedded null character in path",
        ));
    }
    Ok(())
}

/// Snapshot directory entries and close their descriptor before yielding to callers.
fn directory_entries<T>(
    root: &str,
    mut visit: impl FnMut(fs::DirEntry) -> T,
) -> io::Result<Vec<T>> {
    fs::read_dir(root)?
        .map(|entry| entry.map(&mut visit))
        .collect()
}

fn snapshot(root: &str, suppress_errors: bool) -> Result<Vec<OsString>> {
    check_directory_path(root)?;
    let entries = directory_entries(root, |entry| entry.file_name()).map_err(files::os_error);
    if suppress_errors {
        Ok(entries.unwrap_or_default())
    } else {
        entries
    }
}

impl PathIterator {
    fn new(root: Str, pattern: Option<&str>) -> Result<Self> {
        let state = if let Some(pattern) = pattern {
            IterationState::Glob(traversal::Glob::new(
                root.clone(),
                pattern,
                0,
                false,
                false,
            )?)
        } else {
            check_directory_path(&root)?;
            let entries = super::os::directory_entry_types(&root)
                .map_err(|error| files::errno_with_filename(error, Some(&root)))?;
            IterationState::Snapshot(entries.into_iter())
        };
        Ok(Self { root, state })
    }

    pub(super) fn next(&mut self) -> Result<*const Object> {
        let next = match &mut self.state {
            IterationState::Glob(glob) => glob
                .next()
                .map(|next| next.map(|path| path_with_text(PathText::from_parsed_string(path)))),
            IterationState::Snapshot(names) => names
                .next()
                .map(|(name, kind)| {
                    name.into_string()
                        .map(|name| {
                            path_with_text(PathText::from_entry(join(&self.root, &name), kind))
                        })
                        .map_err(|_| {
                            Error::new(
                                Kind::UnicodeDecodeError,
                                "directory entry is not valid UTF-8",
                            )
                        })
                })
                .transpose(),
            IterationState::Exhausted => Ok(None),
        };
        match next {
            Ok(Some(path)) => Ok(path),
            result => {
                self.state = IterationState::Exhausted;
                result.map(|_| std::ptr::null())
            }
        }
    }

    fn into_object(self) -> *const Object {
        crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
            Storage::PathIterator(Box::new(self)),
        ))))
    }
}

/// # Safety
/// The path and pattern stay rooted while the selector copies its inputs; no filesystem
/// scan occurs until the first pull. A zero case word selects platform defaults.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_glob(
    path: *const Object,
    pattern: *const Str,
    case: usize,
    follow: u32,
    recursive: u32,
) -> *const Object {
    // SAFETY: construction has no user callbacks and no borrowed input escapes.
    let result = unsafe {
        object(path).0.with(|storage| {
            let root = Str::from(storage.path());
            let glob =
                traversal::Glob::new(root.clone(), &*pattern, case, follow != 0, recursive != 0)?;
            Ok(PathIterator {
                root,
                state: IterationState::Glob(glob),
            })
        })
    };
    finish(result.map(PathIterator::into_object), std::ptr::null())
}

/// # Safety
/// Root an exact Path and either a string pattern or null. Null selects iterdir:
/// it snapshots names now and propagates directory errors. A glob validates now,
/// scans on its first next call, and suppresses OS scan errors. Returns one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_iter(path: *const Object, pattern: *const Str) -> *const Object {
    // SAFETY: only the copied root and pattern survive this immutable borrow.
    let result = unsafe {
        object(path).0.with(|storage| {
            PathIterator::new(
                Str::from(storage.path()),
                pattern.as_ref().map(|text| text.as_str()),
            )
        })
    };
    finish(result.map(PathIterator::into_object), std::ptr::null())
}

/// # Safety
/// Root an exact directory iterator with clear pending error state. A nonnull
/// result owns one Path; null denotes exhaustion unless an exception is pending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_next(iterator: *const Object) -> *const Object {
    // SAFETY: the root is held through this single synchronous pull.
    finish(unsafe { pull_path(iterator) }, std::ptr::null())
}

unsafe fn pull_path(iterator: *const Object) -> Result<*const Object> {
    // SAFETY: the caller roots the cursor and the synchronous scan cannot reenter it.
    unsafe {
        object(iterator).0.with_mut(|storage| match storage {
            Storage::PathIterator(iterator) => iterator.next(),
            _ => fail("SystemError", "expected a path iterator"),
        })
    }
}

pub(super) enum Pattern {
    Literal(Str),
    Wildcard(Vec<Token>),
}

pub(super) enum Token {
    Literal(char),
    Any,
    Star,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl Pattern {
    #[cfg(test)]
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
        Ok(Self::component(pattern))
    }

    pub(super) fn component(pattern: &str) -> Self {
        if !pattern.contains(['*', '?', '[']) {
            return Self::Literal(Str::from(pattern));
        }
        let chars: Vec<_> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let character = chars[index];
            index += 1;
            match character {
                '*' if matches!(tokens.last(), Some(Token::Star)) => {}
                '*' => tokens.push(Token::Star),
                '?' => tokens.push(Token::Any),
                '[' => {
                    let start = index;
                    let Some(end) = class_end(&chars, index) else {
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
        Self::Wildcard(tokens)
    }

    /// Greedy star matching remembers the most recent star and advances its
    /// Unicode-scalar boundary on mismatch, avoiding recursive backtracking.
    pub(super) fn matches(&self, name: &str) -> bool {
        let tokens = match self {
            Self::Literal(text) => return text == name,
            Self::Wildcard(tokens) => tokens,
        };
        let (mut token, mut at) = (0, 0);
        let mut star = None;
        while at < name.len() {
            if matches!(tokens.get(token), Some(Token::Star)) {
                star = Some((token, at));
                token += 1;
                continue;
            }
            let character = name[at..].chars().next().unwrap();
            if tokens
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
        while matches!(tokens.get(token), Some(Token::Star)) {
            token += 1;
        }
        token == tokens.len()
    }
}

pub(super) fn class_end(chars: &[char], start: usize) -> Option<usize> {
    let content = start + usize::from(chars.get(start) == Some(&'!'));
    let search = content + usize::from(chars.get(content) == Some(&']'));
    (search..chars.len()).find(|&at| chars[at] == ']')
}

/// Python's fnmatch removes descending ranges before interpreting negation.
/// Preserve literal hyphens separately from range separators: `[b-a!]`, for
/// example, becomes a negated empty class and therefore matches any character.
pub(super) fn class_characters(chars: &[char]) -> Vec<(char, bool)> {
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
    characters
}

fn character_class(chars: &[char]) -> Token {
    let characters = class_characters(chars);
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
    use crate::objects::exceptions::{imre_error_pending, imre_error_take, imre_exception_matches};
    use crate::objects::{imre_object_release, values};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "imre-runtime-paths-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> crate::ObjectOwner {
            path(self.0.to_str().unwrap())
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn path(text: &str) -> crate::ObjectOwner {
        let text = StrOwner::new(Str::from(text));
        // SAFETY: the input is rooted and the new path transfers one owner.
        unsafe { crate::ObjectOwner::from_raw(imre_path_new(StrOwner::as_ptr(&text))) }
    }

    #[test]
    fn metadata_views_distinguish_regular_paths_and_scanned_entries_after_mutation() {
        let directory = Directory::new();
        let filename = directory.0.join("entry");
        let entry_name = filename.to_str().unwrap();
        let ordinary = path(entry_name);
        // SAFETY: each ABI result transfers one owner; take the view before dropping
        // its path to also check the independently retained spelling.
        let view = |p: &crate::ObjectOwner| unsafe {
            crate::ObjectOwner::from_raw(metadata::imre_path_info(crate::ObjectOwner::as_ptr(p)))
        };
        // SAFETY: both object kinds accepted by this predicate remain rooted here.
        let query = |p: &crate::ObjectOwner, kind, follow| unsafe {
            metadata::imre_path_query(crate::ObjectOwner::as_ptr(p), kind, u32::from(follow)) != 0
        };
        let first = view(&ordinary);
        assert!(crate::ObjectOwner::ptr_eq(&first, &view(&ordinary)));
        assert!(!query(&first, 0, true));
        drop(ordinary);
        fs::write(&filename, "file").unwrap();
        assert!(!query(&first, 2, true));
        assert!(query(&first, 2, false));
        assert!(query(&view(&path(entry_name)), 2, true));

        // SAFETY: the directory is rooted until acquisition; the single entry
        // and its view outlive the directory iterator's snapshot.
        let scanned = unsafe {
            let dir = directory.path();
            let it = crate::ObjectOwner::from_raw(imre_path_iter(
                crate::ObjectOwner::as_ptr(&dir),
                std::ptr::null(),
            ));
            let item = imre_path_next(crate::ObjectOwner::as_ptr(&it));
            assert!(!item.is_null());
            crate::ObjectOwner::from_raw(item)
        };
        let entry = view(&scanned);
        fs::remove_file(&filename).unwrap();
        assert!(query(&entry, 0, false));
        assert!(query(&entry, 2, true));
        assert!(!query(&entry, 0, true));
        fs::create_dir(&filename).unwrap();
        assert!(query(&entry, 0, true));
        assert!(query(&entry, 2, false));
        assert!(!query(&entry, 1, true));
        assert!(query(&scanned, 1, true));

        // Filesystems that report DT_UNKNOWN defer both type and link detection.
        // Exercise this independently of whether the machine's temp fs supplies it.
        let unknown = || {
            let text = PathText::from_entry(Str::from(entry_name), rustix::fs::FileType::Unknown);
            // SAFETY: recover the sole path owner, then take its independently owned view.
            view(&unsafe { crate::ObjectOwner::from_raw(path_with_text(text)) })
        };
        let remembered = unknown();
        let unqueried = unknown();
        assert!(query(&remembered, 1, false));
        fs::remove_dir(&filename).unwrap();
        symlink("late-target", &filename).unwrap();
        assert!(query(&remembered, 1, true)); // share successful non-link lstat
        assert!(!query(&remembered, 3, false));
        assert!(query(&unqueried, 0, false)); // no stat needed
        assert!(query(&unqueried, 3, false));
        assert!(!query(&unqueried, 0, true)); // a failed follow lookup can retry
        let target = directory.0.join("late-target");
        fs::write(&target, "target").unwrap();
        assert!(query(&unqueried, 2, true));
        fs::remove_file(target).unwrap();
        assert!(query(&unqueried, 0, true)); // retain successful target stat
        assert!(!query(&unqueried, 2, false));
    }

    #[test]
    fn live_parents_share_their_large_spelling() {
        let prefix = "a-long-directory-component-that-cannot-fit-inline";
        // SAFETY: each returned raw pointer transfers one owner. All calls borrow
        // a live exact Path, and each returned owner is recovered just once.
        unsafe {
            let path = crate::ObjectOwner::from_raw(new_path(Str::from(format!("{prefix}/leaf"))));
            let first =
                crate::ObjectOwner::from_raw(imre_path_parent(crate::ObjectOwner::as_ptr(&path)));
            let second =
                crate::ObjectOwner::from_raw(imre_path_parent(crate::ObjectOwner::as_ptr(&path)));
            assert!(!crate::ObjectOwner::ptr_eq(&first, &second));
            let first_text = StrOwner::from_raw(imre_path_text(crate::ObjectOwner::as_ptr(&first)));
            let second_text =
                StrOwner::from_raw(imre_path_text(crate::ObjectOwner::as_ptr(&second)));
            assert!(StrOwner::ptr_eq(&first_text, &second_text));
            let old = StrOwner::downgrade(&first_text);
            drop((first_text, second_text, first, second));
            assert!(old.upgrade().is_none());
            let fresh =
                crate::ObjectOwner::from_raw(imre_path_parent(crate::ObjectOwner::as_ptr(&path)));
            assert!(fresh.0.with(|storage| storage.path() == prefix));
        }
    }

    #[test]
    fn shared_spelling_and_basename_survive_other_owners() {
        let mut source = StrOwner::new(Str::from(
            "//r/../é\0/this-long-basename-is-stored-in-a-heap-buffer",
        ));
        let original = (*source).clone();
        // SAFETY: every input remains rooted through its call and each native
        // return transfers one owner. Source and results use normal Rc COW.
        unsafe {
            let path = crate::ObjectOwner::from_raw(imre_path_new(StrOwner::as_ptr(&source)));
            let first = StrOwner::from_raw(imre_path_text(crate::ObjectOwner::as_ptr(&path)));
            let component = StrOwner::from_raw(imre_path_name(crate::ObjectOwner::as_ptr(&path)));
            let stem = StrOwner::from_raw(imre_path_stem(crate::ObjectOwner::as_ptr(&path)));
            assert!(StrOwner::ptr_eq(&stem, &component));
            let mut other = StrOwner::from_raw(imre_path_name(crate::ObjectOwner::as_ptr(&path)));
            assert!(StrOwner::ptr_eq(&first, &source));
            assert!(StrOwner::ptr_eq(&other, &component));
            StrOwner::make_mut(&mut source).push('!');
            StrOwner::make_mut(&mut other).push('?');
            drop(path);
            assert_eq!(*stem, *component);
            assert_eq!(*first, original);
            assert_eq!(
                component.as_str(),
                "this-long-basename-is-stored-in-a-heap-buffer"
            );
        }
        for root in ["", "/", "//", "///"] {
            for a in ["", ".", "..", ".hidden", "a", "é\0"] {
                for b in ["", ".", "..", "b", "/c", "あ"] {
                    let path = format!("{root}{a}/{b}");
                    assert_eq!(is_normalized(&path), normalize(&path) == path);
                }
            }
        }
    }
    fn take_error(kind: Kind) {
        assert_eq!(imre_error_pending(), 1);
        let error = imre_error_take();
        // SAFETY: take transferred the live exception, which is inspected then released.
        unsafe {
            assert_eq!(imre_exception_matches(error, kind as u32), 1);
            imre_object_release(error);
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
            let next = unsafe { crate::ObjectOwner::from_raw(next) };
            // SAFETY: the immutable Path remains rooted during this copied-name read.
            result.push(unsafe { next.0.with(|storage| name(storage.path()).to_owned()) });
        }
        result.sort();
        result
    }

    #[test]
    fn joined_spelling_is_deferred_and_owns_its_operands() {
        let parent = path("/parent");
        let mut child = StrOwner::new(Str::from(
            "child/./a-long-component-that-needs-character-storage",
        ));
        // SAFETY: both inputs are rooted through the join, whose result owns
        // its operands. Each later text call transfers one independent owner.
        unsafe {
            let joined = crate::ObjectOwner::from_raw(imre_path_join(
                crate::ObjectOwner::as_ptr(&parent),
                StrOwner::as_ptr(&child),
            ));
            drop(parent);
            StrOwner::make_mut(&mut child).push('!');
            let tail = StrOwner::new(Str::from(".."));
            let descendant = crate::ObjectOwner::from_raw(imre_path_join(
                crate::ObjectOwner::as_ptr(&joined),
                StrOwner::as_ptr(&tail),
            ));
            let rendered =
                StrOwner::from_raw(imre_path_text(crate::ObjectOwner::as_ptr(&descendant)));
            assert_eq!(
                rendered.as_str(),
                "/parent/child/a-long-component-that-needs-character-storage/.."
            );
            assert_eq!(
                imre_path_is_absolute(crate::ObjectOwner::as_ptr(&joined)),
                1
            );
            joined.0.with(|storage| {
                let PathText::Joined { spelling, .. } = storage.path_value() else {
                    panic!("expected a deferred path");
                };
                assert!(spelling.get().is_none());
            });
            let first = StrOwner::from_raw(imre_path_text(crate::ObjectOwner::as_ptr(&joined)));
            let second = StrOwner::from_raw(imre_path_text(crate::ObjectOwner::as_ptr(&joined)));
            drop(joined);
            assert!(StrOwner::ptr_eq(&first, &second));
            assert_eq!(
                first.as_str(),
                "/parent/child/a-long-component-that-needs-character-storage"
            );
        }
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
                let pointer = crate::ObjectOwner::as_ptr(&value);
                assert_eq!(
                    StrOwner::from_raw(imre_path_name(pointer)).as_str(),
                    expected_name
                );
                assert_eq!(StrOwner::from_raw(imre_path_stem(pointer)).as_str(), stem);
                assert_eq!(
                    StrOwner::from_raw(imre_path_suffix(pointer)).as_str(),
                    suffix
                );
                let parent = crate::ObjectOwner::from_raw(imre_path_parent(pointer));
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
            assert_eq!(
                imre_path_equal(
                    crate::ObjectOwner::as_ptr(&left),
                    crate::ObjectOwner::as_ptr(&right)
                ),
                1
            );
            assert!(values::equal(
                crate::ObjectOwner::as_ptr(&left) as usize,
                crate::ObjectOwner::as_ptr(&right) as usize,
                super::super::words::WordKind::new(5)
            ));
            assert_eq!(
                StrOwner::from_raw(values::imre_value_repr(
                    crate::ObjectOwner::as_ptr(&left) as usize,
                    5
                ))
                .as_str(),
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
            let pointer = crate::ObjectOwner::as_ptr(&value);
            assert_eq!(imre_path_exists(pointer), 0);
            assert_eq!(imre_path_is_dir(crate::ObjectOwner::as_ptr(&root)), 1);
            let count = Int(imre_path_write_text(pointer, &Str::from("é\r\n🦀\r")));
            assert_eq!(count.as_small(), Some(5));
            assert_eq!(imre_path_is_file(pointer), 1);
            assert_eq!(
                StrOwner::from_raw(imre_path_read_text(pointer)).as_str(),
                "é\n🦀\n"
            );
            fs::write(&filename, [0xff]).unwrap();
            assert!(imre_path_read_text(pointer).is_null());
            take_error(Kind::UnicodeDecodeError);
            fs::remove_file(&filename).unwrap();
            assert!(imre_path_read_text(pointer).is_null());
            take_error(Kind::FileNotFoundError);
            assert!(imre_path_read_text(crate::ObjectOwner::as_ptr(&root)).is_null());
            take_error(Kind::IsADirectoryError);
            let nul = path("a\0b");
            assert_eq!(imre_path_exists(crate::ObjectOwner::as_ptr(&nul)), 0);
            assert_eq!(imre_path_is_file(crate::ObjectOwner::as_ptr(&nul)), 0);
            assert_eq!(imre_path_is_dir(crate::ObjectOwner::as_ptr(&nul)), 0);
            assert!(imre_path_read_text(crate::ObjectOwner::as_ptr(&nul)).is_null());
            take_error(Kind::ValueError);
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn mkdir_uses_leaf_mode_and_distinguishes_existing_directories_from_files() {
        use std::os::unix::fs::PermissionsExt;

        let directory = Directory::new();
        let leaf = path(directory.0.join("parent/leaf").to_str().unwrap());
        let mode = Int::small(0o700).unwrap();
        // SAFETY: exact rooted Path/Int values remain live, and every failure is taken.
        unsafe {
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&leaf), mode.0, 0, 0);
            take_error(Kind::FileNotFoundError);
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&leaf), mode.0, 1, 0);
            assert_eq!(imre_error_pending(), 0);
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
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&leaf), mode.0, 1, 0);
            take_error(Kind::FileExistsError);
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&leaf), mode.0, 1, 1);
            assert_eq!(imre_error_pending(), 0);
            symlink("parent/leaf", directory.0.join("link")).unwrap();
            let link = path(directory.0.join("link").to_str().unwrap());
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&link), mode.0, 0, 1);
            assert_eq!(imre_error_pending(), 0);
            fs::write(directory.0.join("file"), "kept").unwrap();
            let file = path(directory.0.join("file").to_str().unwrap());
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&file), mode.0, 1, 1);
            take_error(Kind::FileExistsError);
            let child = path(directory.0.join("file/child").to_str().unwrap());
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&child), mode.0, 1, 1);
            take_error(Kind::NotADirectoryError);
            let huge = Int::new(num_bigint::BigInt::from(1_u8) << 100);
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&leaf), huge.0, 1, 1);
            take_error(Kind::OverflowError);
            let nul = path("invalid\0path");
            imre_path_mkdir(crate::ObjectOwner::as_ptr(&nul), huge.0, 1, 1);
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
            let replaced =
                imre_path_replace(crate::ObjectOwner::as_ptr(&source_path), &target_text);
            assert!(!replaced.is_null());
            let replaced = crate::ObjectOwner::from_raw(replaced);
            assert_eq!(imre_error_pending(), 0);
            assert_eq!(
                replaced.0.with(|storage| storage.path().to_owned()),
                target_text.as_str()
            );
            assert!(!source.exists());
            assert_eq!(fs::read_to_string(&target).unwrap(), "replacement");
            assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
            assert!(
                imre_path_replace(crate::ObjectOwner::as_ptr(&source_path), &target_text).is_null()
            );
            take_error(Kind::FileNotFoundError);
            imre_path_unlink(crate::ObjectOwner::as_ptr(&replaced), 0);
            imre_path_unlink(crate::ObjectOwner::as_ptr(&replaced), 1);
            assert_eq!(imre_error_pending(), 0);
            symlink(&outside, &target).unwrap();
            imre_path_rmdir(crate::ObjectOwner::as_ptr(&replaced));
            take_error(Kind::NotADirectoryError);
            imre_path_unlink(crate::ObjectOwner::as_ptr(&replaced), 0);
            assert_eq!(fs::read_to_string(outside).unwrap(), "original");
            let non_directory = path(directory.0.join("preserved/child").to_str().unwrap());
            imre_path_unlink(crate::ObjectOwner::as_ptr(&non_directory), 1);
            take_error(Kind::NotADirectoryError);
            let root = directory.path();
            imre_path_unlink(crate::ObjectOwner::as_ptr(&root), 1);
            // Darwin returns EPERM and Linux returns EISDIR for unlink on a directory.
            take_error(if cfg!(target_os = "macos") {
                Kind::PermissionError
            } else {
                Kind::IsADirectoryError
            });
        }
        assert_eq!(imre_error_pending(), 0);
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
            imre_object_release(first);
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
        for (spelling, yes, no) in [
            ("literal.β", "literal.β", "other.β"),
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
            let pattern = Pattern::new(spelling).unwrap_or_else(|_| panic!());
            assert!(pattern.matches(yes), "{spelling} {yes}");
            assert!(!pattern.matches(no), "{spelling} {no}");
        }
    }
}
