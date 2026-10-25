//! Process-local logging state for the native single-threaded runtime.
use crate::StrOwner;
mod cache;
mod config;
mod handler;
mod hierarchy;
mod levels;
pub(super) mod locks;
pub(super) mod timed;

pub(super) mod formatter;
pub(super) mod record;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::time::{SystemTime, UNIX_EPOCH};

use super::exceptions::Result;
use super::mappings::IntKey;
use super::{Error, Int, Kind, Object, Storage, StorageCell, Str, finish, object};

struct Entry {
    handle: crate::ObjectOwner,
    level: Int,
    creation_order: usize,
    enabled: cache::EnabledCache,
}

impl Entry {
    fn new(name: &str, level: i64, creation_order: usize) -> Self {
        Self {
            handle: crate::ObjectOwner::new(Object(StorageCell::new(Storage::Logger(
                StrOwner::new(Str::from(name)),
            )))),
            level: Int::small(level).unwrap(),
            creation_order,
            enabled: cache::EnabledCache::new(),
        }
    }
}

struct Registry {
    disable: Int,
    entries: HashMap<String, Entry>,
    format: Option<Vec<Field>>,
    datefmt: Option<String>,
}

impl Registry {
    fn new() -> Self {
        Self {
            disable: Int::small(0).unwrap(),
            entries: HashMap::from([("root".into(), Entry::new("root", 30, 0))]),
            format: None,
            datefmt: None,
        }
    }

    fn set_level(&mut self, name: &str, value: &Int) {
        self.entries.get_mut(name).unwrap().level = value.clone();
        self.clear_enabled();
    }

    /// Previously returned caches stay live when a level or the manager limit changes.
    fn clear_enabled(&self) {
        // Manager._clear_cache invalidates every logger, even when the level is unchanged.
        for entry in self.entries.values() {
            entry.enabled.clear();
        }
    }

    fn parent_name<'a>(&self, name: &'a str) -> Option<&'a str> {
        if name == "root" {
            return None;
        }
        let mut end = name.len();
        while let Some(i) = name.as_bytes()[..end]
            .iter()
            .rposition(|&byte| byte == b'.')
        {
            if i == 0 {
                break;
            }
            let parent = &name[..i];
            // _fixupChildren keeps an existing parent whose name starts with
            // the new ancestor's name. A child of "root" can predate "r",
            // "ro", or "roo" without becoming its child when it is created.
            if let Some(entry) = self.entries.get(parent)
                && (!"root".starts_with(parent)
                    || entry.creation_order < self.entries[name].creation_order)
            {
                return Some(parent);
            }
            // Manager._fixupParents searches for the preceding dot before i - 1.
            // In particular, "a..b" sees "a." and skips an adjacent "a" prefix.
            end = i - 1;
        }
        Some("root")
    }

    fn effective(&self, mut name: &str) -> &Int {
        loop {
            let level = &self.entries[name].level;
            if level.as_small() != Some(0) {
                return level;
            }
            let Some(parent) = self.parent_name(name) else {
                return level;
            };
            name = parent;
        }
    }
}

thread_local! { static LOGGERS: RefCell<Registry> = RefCell::new(Registry::new()); }

enum Field {
    Literal(String),
    Name,
    LevelName,
    LevelNo,
    Message,
    Time,
}

fn parse_field(tail: &str) -> Option<(Field, &str)> {
    [
        ("(name)s", Field::Name),
        ("(levelname)s", Field::LevelName),
        ("(levelno)d", Field::LevelNo),
        ("(message)s", Field::Message),
        ("(asctime)s", Field::Time),
    ]
    .into_iter()
    .find_map(|(prefix, field)| tail.strip_prefix(prefix).map(|rest| (field, rest)))
}

fn fields(mut fmt: &str) -> Result<Vec<Field>> {
    let original = fmt;
    let mut parts = Vec::new();
    let mut has_field = false;
    while let Some((literal, tail)) = fmt.split_once('%') {
        if !literal.is_empty() {
            parts.push(Field::Literal(literal.into()));
        }
        if let Some(rest) = tail.strip_prefix('%') {
            // PercentStyle validates against the original format, even when
            // the matching field starts at the second percent of a literal %%.
            has_field |= parse_field(rest).is_some();
            parts.push(Field::Literal("%".into()));
            fmt = rest;
            continue;
        }
        let (field, rest) = parse_field(tail).ok_or_else(|| {
            Error::new(
                Kind::ValueError,
                format!("Invalid format '{original}' for '%' style"),
            )
        })?;
        parts.push(field);
        has_field = true;
        fmt = rest;
    }
    if !has_field {
        return Err(Error::new(
            Kind::ValueError,
            format!("Invalid format '{original}' for '%' style"),
        ));
    }
    if !fmt.is_empty() {
        parts.push(Field::Literal(fmt.into()));
    }
    Ok(parts)
}

fn timestamp(format: Option<&str>) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    timed::civil(now.as_secs() as libc::time_t, false)
        .map(|local| format_timestamp(format, &local, now))
        .unwrap_or_default()
}

/// Renders either an emitted timestamp or the time captured earlier by a LogRecord.
fn format_timestamp(format: Option<&str>, local: &libc::tm, now: std::time::Duration) -> String {
    let custom = format.filter(|value| !value.is_empty());
    let mut text = String::new();
    for (index, part) in custom
        .unwrap_or("%Y-%m-%d %H:%M:%S")
        .split('\0')
        .enumerate()
    {
        if index != 0 {
            text.push('\0');
        }
        if part.is_empty() {
            continue;
        }
        // strftime takes a C string. Format each NUL-free segment at the same instant.
        // The sentinel distinguishes an empty expansion from an undersized buffer.
        let fmt = CString::new(format!("{part}!")).expect("the segment contains no NUL");
        let mut buf = vec![0u8; fmt.as_bytes().len().max(64)];
        loop {
            // SAFETY: the calendar time and format are valid and the buffer is writable.
            let len =
                unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), local) };
            if len > 0 {
                text.push_str(&String::from_utf8_lossy(&buf[..len - 1]));
                break;
            }
            if buf.len() > fmt.as_bytes().len().saturating_mul(256).max(4096) {
                return String::new();
            }
            buf.resize(buf.len() * 2, 0);
        }
    }
    if custom.is_none() {
        text.push_str(&format!(",{:03}", now.subsec_millis()));
    }
    text
}

unsafe fn name(handle: *const Object) -> StrOwner {
    // SAFETY: each caller supplies a rooted native Logger; the name acquires its own owner.
    unsafe {
        object(handle).0.with(|storage| match storage {
            Storage::Logger(name) => name.clone(),
            _ => super::fail("SystemError", "expected logging.Logger"),
        })
    }
}

/// # Safety
/// A nonzero word points to a rooted exact string. The returned logger is owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_get(word: usize) -> *const Object {
    // SAFETY: non-null optional payloads remain rooted for this call.
    let name = unsafe { (word as *const Str).as_ref() }.map_or("", Str::as_str);
    let name = if name.is_empty() { "root" } else { name };
    LOGGERS.with_borrow_mut(|logs| {
        if let Some(entry) = logs.entries.get(name) {
            return crate::ObjectOwner::into_raw(entry.handle.clone());
        }
        let entry = Entry::new(name, 0, logs.entries.len());
        let result = crate::ObjectOwner::into_raw(entry.handle.clone());
        logs.entries.insert(name.into(), entry);
        result
    })
}

/// # Safety
/// All nonzero optional words are rooted values of their declared kinds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_config(level: usize, fmt: usize, datefmt: usize, force: bool) {
    let result = LOGGERS.with_borrow_mut(|logs| {
        if logs.format.is_some() && !force {
            return Ok(());
        }
        if force {
            logs.format = None;
        }
        // SAFETY: both nullable strings remain rooted until configuration returns.
        let (fmt, datefmt) = unsafe {
            (
                (fmt as *const Str).as_ref(),
                (datefmt as *const Str).as_ref(),
            )
        };
        let fmt = fmt.map_or("", Str::as_str);
        let format = fields(if fmt.is_empty() { "%(message)s" } else { fmt })?;
        let datefmt = datefmt.map(ToString::to_string);
        if level != 0 {
            // SAFETY: obtain an independent owner of the supplied integer.
            let level = unsafe { Int::borrow(level) };
            logs.set_level("root", &level);
        }
        logs.format = Some(format);
        logs.datefmt = datefmt;
        Ok(())
    });
    finish(result, ());
}

/// # Safety
/// The logger is rooted; the result owns its name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_name(logger: *const Object) -> *const Str {
    // SAFETY: the caller retains the logger while its name acquires an owner.
    StrOwner::into_raw(unsafe { name(logger) })
}

/// A receiver has already been evaluated and rooted by the caller.
#[unsafe(no_mangle)]
pub extern "C" fn imre_log_root(_logger: *const Object) -> *const Object {
    LOGGERS.with_borrow(|logs| crate::ObjectOwner::into_raw(logs.entries["root"].handle.clone()))
}

/// # Safety
/// The logger is live. Return an owned parent or the absent optional word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_parent(logger: *const Object) -> usize {
    // SAFETY: the caller roots this logger while we acquire its name.
    let name = unsafe { name(logger) };
    LOGGERS.with_borrow(|logs| {
        logs.parent_name(&name).map_or(0, |parent| {
            crate::ObjectOwner::into_raw(logs.entries[parent].handle.clone()) as usize
        })
    })
}

/// # Safety
/// The logger is rooted; the result owns its exact integer level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_level(logger: *const Object) -> usize {
    // SAFETY: the caller roots this logger while we acquire its name.
    let name = unsafe { name(logger) };
    LOGGERS.with_borrow(|logs| logs.entries[name.as_str()].level.clone().into_raw())
}

/// # Safety
/// The logger is rooted; the result owns the inherited level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_effective_level(logger: *const Object) -> usize {
    // SAFETY: the caller roots this logger while we acquire its name.
    let name = unsafe { name(logger) };
    LOGGERS.with_borrow(|logs| logs.effective(&name).clone().into_raw())
}

/// # Safety
/// The logger and exact tagged level remain rooted during assignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_set_level(logger: *const Object, level: usize) {
    // SAFETY: both arguments are rooted by the caller.
    let (name, value) = unsafe { (name(logger), Int::borrow(level)) };
    LOGGERS.with_borrow_mut(|logs| logs.set_level(&name, &value));
}

/// # Safety
/// The rooted arguments are a native logger and integer level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_enabled(logger: *const Object, level: usize) -> bool {
    // SAFETY: both arguments are rooted by the caller.
    let (name, value) = unsafe { (name(logger), Int::borrow(level)) };
    let result = LOGGERS.with_borrow(|logs| {
        let key = IntKey((*value).clone());
        if let Some(enabled) = logs.entries[name.as_str()].enabled.get(&key) {
            return Ok(enabled);
        }
        let threshold = logs.effective(&name);
        let enabled = match (
            value.as_small(),
            logs.disable.as_small(),
            threshold.as_small(),
        ) {
            (Some(level), Some(disable), Some(threshold)) => level > disable && level >= threshold,
            _ => value.big() > logs.disable.big() && value.big() >= threshold.big(),
        };
        logs.entries[name.as_str()].enabled.insert(key, enabled)?;
        Ok(enabled)
    });
    finish(result, false)
}

/// # Safety
/// The compiler has checked the level and roots all three arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_emit(logger: *const Object, level: usize, message: *const Str) {
    // SAFETY: each exact input is rooted through the synchronous stderr write.
    let (name, level, message) = unsafe { (name(logger), Int::borrow(level), &*message) };
    LOGGERS.with_borrow(|logs| {
        let mut line = String::new();
        if let Some(fields) = &logs.format {
            let mut asctime = None;
            for field in fields {
                match field {
                    Field::Literal(text) => line.push_str(text),
                    Field::Time => line.push_str(
                        asctime.get_or_insert_with(|| timestamp(logs.datefmt.as_deref())),
                    ),
                    Field::Message => line.push_str(message),
                    Field::LevelNo => line.push_str(&level.big().to_string()),
                    Field::LevelName => line.push_str(&levels::level_name(&level)),
                    Field::Name => line.push_str(&name),
                }
            }
        } else {
            if level.big().as_ref() < &num_bigint::BigInt::from(30) {
                return;
            }
            line.push_str(message);
        }
        // A handler writes and flushes the same text stream as sys.stderr.write.
        let _ = crate::sys::write_stderr(&line, true);
    });
}

/// Look up a logger for callers compiled with the original named-logger ABI.
///
/// # Safety
/// The caller roots the string for the duration of the lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logger_get(name: *const Str) -> *const Object {
    // SAFETY: a rooted string pointer is a present optional string word.
    unsafe { imre_log_get(name as usize) }
}

/// Install the original level-only configuration in the shared registry.
///
/// # Safety
/// The caller roots the exact tagged integer level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logger_configure(level: usize) {
    let format = Str::from("%(levelname)s:%(name)s:%(message)s");
    // SAFETY: both inputs stay live while the registry copies the configuration.
    unsafe { imre_log_config(level, std::ptr::from_ref(&format) as usize, 0, false) }
}

/// Emit a preformatted message from the original named-logger ABI.
///
/// # Safety
/// The caller roots the logger and message for the whole call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logger_write(logger: *const Object, level: u32, message: *const Str) {
    let level = Int::small(i64::from(level)).unwrap().into_raw();
    // SAFETY: the caller roots both objects and every u32 level fits in an immediate.
    unsafe {
        if config::imre_log_is_enabled(logger, level) {
            imre_log_emit(logger, level, message);
        }
    }
}
