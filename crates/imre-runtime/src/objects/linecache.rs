//! Filesystem-backed source lines, using the same dictionary, tuple and list
//! objects exposed to Python. Cache mutation and list aliases stay observable.
use crate::StrOwner;
mod source;

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};

use super::exceptions::Result;
use super::{
    Error, Int, Kind, Object, Str, Words, finish, mappings, records,
    words::{OwnedWord, WordKind},
};
use num_traits::ToPrimitive;

thread_local! {
    // Like other Python containers, the closed native interpreter owns this on
    // its execution thread. Source loader callbacks/threads are not admitted.
    // SAFETY: imre_dict_new transfers one owned str-keyed, tuple-valued dictionary.
    static CACHE: crate::ObjectOwner = unsafe { crate::ObjectOwner::from_raw(mappings::imre_dict_new(3,6)) };
    static SAFE_PATH: Cell<bool> = const { Cell::new(false) };
    static SEARCH: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Snapshot PYTHONPATH before user code can chdir or mutate os.environ.
#[unsafe(no_mangle)]
pub extern "C" fn imre_linecache_init() {
    SAFE_PATH.set(std::env::var_os("PYTHONSAFEPATH").is_some_and(|s| !s.is_empty()));
    SEARCH.with_borrow_mut(|dirs| {
        dirs.clear();
        if let Some(value) = std::env::var_os("PYTHONPATH") {
            dirs.extend(std::env::split_paths(&value).filter_map(|p| {
                std::path::absolute(if p.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &p
                })
                .ok()
            }));
        }
    });
}

/// Shares the ordinary mutable dictionary; its value layout is fixed by the frontend.
#[unsafe(no_mangle)]
pub extern "C" fn imre_linecache_cache() -> *const Object {
    CACHE.with(|cache| crate::ObjectOwner::into_raw(cache.clone()))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_linecache_clear() {
    // SAFETY: the thread-local root is the same typed dictionary returned by cache().
    CACHE.with(|cache| unsafe { mappings::imre_dict_clear(crate::ObjectOwner::as_ptr(cache)) });
}

fn stamp(meta: &fs::Metadata) -> Option<(u64, f64)> {
    // CPython forms st_mtime as whole seconds + nanoseconds / 1e9. Comparing
    // exact SystemTime would invalidate some files CPython considers unchanged.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((
            meta.len(),
            meta.mtime() as f64 + meta.mtime_nsec() as f64 * 1e-9,
        ))
    }
    #[cfg(not(unix))]
    {
        use std::time::UNIX_EPOCH;
        let time = meta.modified().ok()?;
        let mtime = match time.duration_since(UNIX_EPOCH) {
            Ok(t) => t.as_secs_f64(),
            Err(early) => -early.duration().as_secs_f64(),
        };
        Some((meta.len(), mtime))
    }
}

struct SourceFile {
    path: PathBuf,
    original: Option<StrOwner>,
    stamp: (u64, f64),
}

fn stat(
    filename: &StrOwner,
    globals: Option<&Object>,
    root: &str,
    env: bool,
) -> Option<SourceFile> {
    if filename.is_empty()
        || filename.contains('\0')
        || (filename.starts_with('<')
            && filename.ends_with('>')
            && !filename.starts_with("<frozen "))
    {
        return None;
    }
    let fullname = if filename.starts_with("<frozen ") {
        let globals = globals?;
        let key = StrOwner::new(Str::new("__file__"));
        // SAFETY: globals is string-to-string and both it and the key are rooted.
        let file = unsafe {
            mappings::lookup(std::ptr::from_ref(globals), StrOwner::as_ptr(&key) as usize)
        }?;
        // SAFETY: transfer the acquired, correctly typed string owner.
        unsafe { StrOwner::from_raw(file.into_word() as *const Str) }
    } else {
        filename.clone()
    };
    if fullname.contains('\0') {
        return None;
    }
    let probe = |path: PathBuf, original| {
        Some(SourceFile {
            stamp: stamp(&fs::metadata(&path).ok()?)?,
            path,
            original,
        })
    };
    if let Some(file) = probe(PathBuf::from(fullname.as_str()), Some(fullname)) {
        return Some(file);
    }
    let name = Path::new(filename.as_str());
    if name.is_absolute() {
        return None;
    }
    if !root.is_empty()
        && !SAFE_PATH.get()
        && let Some(file) = probe(Path::new(root).join(name), None)
    {
        return Some(file);
    }
    if env {
        SEARCH.with_borrow(|dirs| dirs.iter().find_map(|dir| probe(dir.join(name), None)))
    } else {
        None
    }
}

fn empty() -> crate::ObjectOwner {
    Words::new(3).into_object()
}

// Removing via the mapping API preserves aliases and releases the old tuple only
// after the dictionary mutation has finished.
fn remove(cache: &crate::ObjectOwner, key: &Str) {
    let mut old = 0;
    // SAFETY: cache roots a string-key, tuple-value dictionary and key lives for the call.
    unsafe {
        if mappings::imre_dict_try_pop(
            crate::ObjectOwner::as_ptr(cache),
            std::ptr::from_ref(key) as usize,
            &mut old,
        ) != 0
        {
            drop(OwnedWord::from_owned(old, WordKind::new(6)));
        }
    }
}

fn load(
    cache: &crate::ObjectOwner,
    name: &StrOwner,
    globals: Option<&Object>,
    root: &str,
    newline: &StrOwner,
    env: bool,
    layout: *const records::RecordDescriptor,
) -> Result<crate::ObjectOwner> {
    let Some(SourceFile {
        path,
        original,
        stamp: (size, mtime),
    }) = stat(name, globals, root, env)
    else {
        return Ok(empty());
    };
    let Ok(raw) = fs::read(&path) else {
        return Ok(empty());
    };
    let Some(text) = source::decode(&raw)? else {
        return Ok(empty());
    };
    let mut list = Words::new(3);
    for line in source::lines(&text).chain(text.is_empty().then_some("")) {
        list.values
            .try_reserve(1)
            .map_err(|_| Error::new(Kind::MemoryError, "source line allocation failed"))?;
        let item = if line.is_empty() {
            newline.clone()
        } else {
            let mut item = Str::new(line);
            item.push('\n');
            StrOwner::new(item)
        };
        list.values.push(StrOwner::into_raw(item) as usize);
    }
    let lines = list.into_object();
    let fullname = match original {
        Some(original) => original,
        None => StrOwner::new(Str::new(path.to_str().ok_or_else(|| {
            Error::new(
                Kind::NotImplementedError,
                "linecache: source search path contains a non-Unicode filename",
            )
        })?)),
    };
    // SAFETY: the compiler supplies a static layout for (int,float,list[str],str).
    // The tuple owns a reference to its exact mutable list; the caller gains another.
    unsafe {
        let record = records::allocate_record(layout);
        if record.is_null() {
            return Err(Error::new(
                Kind::MemoryError,
                "source record allocation failed",
            ));
        }
        let owner = OwnedWord::from_owned(record as usize, WordKind::new(6));
        for (i, (kind, value)) in [
            (2, Int::new(size.into()).into_raw()),
            (7, mtime.to_bits() as usize),
            (5, crate::ObjectOwner::into_raw(lines.clone()) as usize),
            (3, StrOwner::into_raw(fullname) as usize),
        ]
        .into_iter()
        .enumerate()
        {
            OwnedWord::from_owned(value, WordKind::new(kind))
                .initialize_record_field(record.add(i));
        }
        mappings::dict_set(
            crate::ObjectOwner::as_ptr(cache),
            StrOwner::as_ptr(name) as usize,
            owner.word,
        )?;
    }
    Ok(lines)
}

/// # Safety
/// Name/root are rooted strings; layout is the static public cache-entry tuple.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_linecache_lines(
    name: *const Str,
    globals: *const Object,
    root: *const Str,
    newline: *const Str,
    env: u32,
    update: u32,
    layout: *const records::RecordDescriptor,
) -> *const Object {
    // SAFETY: cache and strings live while the entry/lines acquire independent roots.
    let result = unsafe {
        cached_lines(
            &*name,
            globals.as_ref(),
            (&*root).as_str(),
            &*newline,
            env != 0,
            update != 0,
            layout,
        )
    };
    finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// Read the real public cache, including mutations to its entries and source lists.
///
/// # Safety
/// The immutable layout describes the standard four-field cache entry. The
/// borrowed name and newline belong to live native Rc strings: cache misses
/// retain those exact objects when creating a new entry.
pub(super) unsafe fn cached_lines(
    name: &Str,
    globals: Option<&Object>,
    root: &str,
    newline: &Str,
    env: bool,
    update: bool,
    layout: *const records::RecordDescriptor,
) -> Result<crate::ObjectOwner> {
    // SAFETY: the typed mapping roots the entry and source list during the lookup.
    CACHE.with(|cache| unsafe {
        if !update
            && let Some(entry) = mappings::lookup(
                crate::ObjectOwner::as_ptr(cache),
                std::ptr::from_ref(name) as usize,
            )
        {
            return Ok(crate::ObjectOwner::acquire_word(entry_field(&entry, 2, 5)));
        }
        remove(cache, name);
        match load(
            cache,
            &crate::StrOwner::acquire_word(std::ptr::from_ref(name) as usize),
            globals,
            root,
            &crate::StrOwner::acquire_word(std::ptr::from_ref(newline) as usize),
            env,
            layout,
        ) {
            Err(e) if !update && e.is_kind(Kind::MemoryError) => {
                mappings::imre_dict_clear(crate::ObjectOwner::as_ptr(cache));
                Ok(empty())
            }
            result => result,
        }
    })
}

fn entry_field(entry: &OwnedWord, i: usize, kind: u32) -> usize {
    // SAFETY: only this exact four-field tuple can enter the public typed mapping.
    unsafe { WordKind::new(kind).read_record_field((entry.word as *const usize).add(i)) }
}

fn check(cache: &crate::ObjectOwner, key: &Str) {
    // SAFETY: the dictionary roots typed entries and the key lives for the lookup.
    let Some(entry) = (unsafe {
        mappings::lookup(
            crate::ObjectOwner::as_ptr(cache),
            std::ptr::from_ref(key) as usize,
        )
    }) else {
        return;
    };
    // SAFETY: this acquired entry roots its integer and fullname string.
    let (size, fullname) = unsafe {
        (
            Int::borrow(entry_field(&entry, 0, 2)),
            &*(entry_field(&entry, 3, 3) as *const Str),
        )
    };
    let old_mtime = f64::from_bits(entry_field(&entry, 1, 7) as u64);
    let valid = fs::metadata(fullname.as_str())
        .ok()
        .and_then(|s| stamp(&s))
        .is_some_and(|(n, ts)| old_mtime == ts && size.big().to_u64() == Some(n));
    if !valid {
        remove(cache, key);
    }
}

/// # Safety
/// A non-null filename borrows an exact string; null checks the whole dictionary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_linecache_check(name: *const Str) {
    CACHE.with(|cache| {
        // SAFETY: the optional filename is rooted for this operation.
        if let Some(name) = unsafe { name.as_ref() } {
            check(cache, name);
        } else {
            // SAFETY: acquire each exact immutable key while its dictionary entry owns it.
            let keys = unsafe {
                cache.0.with(|s| {
                    s.dictionary()
                        .string_entries()
                        .map(|(k, _)| crate::StrOwner::acquire_word(std::ptr::from_ref(k) as usize))
                        .collect::<Vec<_>>()
                })
            };
            for key in keys {
                check(cache, &key);
            }
        }
    });
}

/// # Safety
/// The list and canonical integer are both rooted; an in-range element is an exact string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_linecache_line_int(
    lines: *const Object,
    n: usize,
    missing: *const Str,
) -> *const Str {
    // SAFETY: input owners survive lookup; a missing row acquires the caller's pooled empty string.
    StrOwner::into_raw(unsafe {
        line_at(super::object(lines), n).unwrap_or_else(|| StrOwner::acquire_word(missing as usize))
    })
}

/// # Safety
/// The caller roots an exact str list and a tagged Python integer.
pub(super) unsafe fn line_at(lines: &Object, n: usize) -> Option<crate::StrOwner> {
    // SAFETY: the borrowed list roots each selected string until acquired.
    unsafe {
        lines.0.with(|s| {
            Int::borrow(n)
                .as_small()
                .and_then(|i| usize::try_from(i).ok()?.checked_sub(1))
                .and_then(|i| s.sequence().values.get(i))
                .map(|&word| crate::StrOwner::acquire_word(word))
        })
    }
}

/// # Safety
/// The exact string list remains rooted during the range check.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_linecache_line_float(
    lines: *const Object,
    n: f64,
    missing: *const Str,
) -> *const Str {
    // SAFETY: the list lives during its length query.
    let len = unsafe { super::object(lines).0.with(|s| s.sequence().values.len()) };
    if 1.0 <= n && n <= len as f64 {
        Error::new(
            Kind::TypeError,
            "list indices must be integers or slices, not float",
        )
        .returning(std::ptr::null())
    } else {
        // SAFETY: the rooted literal transfers an independent return reference.
        StrOwner::into_raw(unsafe { crate::StrOwner::acquire_word(missing as usize) })
    }
}
