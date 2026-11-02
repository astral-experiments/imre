//! Process-local logging state for the native single-threaded runtime.
use crate::StrOwner;
mod cache;
mod config;
mod delivery;
mod handler;
mod hierarchy;
mod levels;
pub(crate) use locks::HandlerLock;
pub(super) mod locks;
pub(super) mod manager;
pub(super) mod timed;

mod fields;
pub(super) mod formatter;
pub(super) mod record;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use super::exceptions::Result;
use super::mappings::IntKey;
use super::{Error, Int, Kind, Object, Storage, StorageCell, Str, finish, object};

/// A logger owns its state whether or not a manager registered its name.
pub(crate) struct Logger {
    name: StrOwner,
    level: RefCell<Int>,
    enabled: cache::EnabledCache,
    handlers: RefCell<crate::ObjectOwner>,
    standalone: bool,
    propagate: Cell<bool>,
    disabled: Cell<bool>,
}

impl Logger {
    fn new(name: StrOwner, level: Int, standalone: bool) -> Rc<Self> {
        Rc::new(Self {
            name,
            level: RefCell::new(level),
            enabled: cache::EnabledCache::new(),
            handlers: RefCell::new(super::Words::new(6).into_object()),
            standalone,
            propagate: Cell::new(true),
            disabled: Cell::new(false),
        })
    }

    fn handle(self: &Rc<Self>) -> crate::ObjectOwner {
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::Logger(self.clone()))))
    }

    fn enabled_at(&self, value: &Int) -> Result<bool> {
        LOGGERS.with_borrow(|logs| {
            let key = IntKey(value.clone());
            if let Some(enabled) = self.enabled.get(&key) {
                return Ok(enabled);
            }
            let threshold = logs.effective(self);
            let enabled = match (
                value.as_small(),
                logs.disable.as_small(),
                threshold.as_small(),
            ) {
                (Some(level), Some(disable), Some(threshold)) => {
                    level > disable && level >= threshold
                }
                _ => value.big() > logs.disable.big() && value.big() >= threshold.big(),
            };
            self.enabled.insert(key, enabled)?;
            Ok(enabled)
        })
    }
}

struct Entry {
    handle: crate::ObjectOwner,
    logger: Rc<Logger>,
    creation_order: usize,
}

impl Entry {
    fn new(name: &str, level: i64, creation_order: usize) -> Self {
        let logger = Logger::new(
            StrOwner::new(Str::from(name)),
            Int::small(level).unwrap(),
            false,
        );
        Self {
            handle: logger.handle(),
            logger,
            creation_order,
        }
    }
}

struct Registry {
    disable: Int,
    entries: HashMap<String, Entry>,
}

impl Registry {
    fn new() -> Self {
        Self {
            disable: Int::small(0).unwrap(),
            entries: HashMap::from([("root".into(), Entry::new("root", 30, 0))]),
        }
    }

    fn set_level(&self, logger: &Logger, value: &Int) {
        logger.level.replace(value.clone());
        self.clear_enabled();
    }

    fn entry(&mut self, name: &str) -> &Entry {
        let name = if name.is_empty() { "root" } else { name };
        if !self.entries.contains_key(name) {
            let entry = Entry::new(name, 0, self.entries.len());
            self.entries.insert(name.into(), entry);
        }
        &self.entries[name]
    }

    /// Previously returned caches stay live when a level or the manager limit changes.
    fn clear_enabled(&self) {
        // Manager._clear_cache visits root and loggerDict, even when the level is unchanged.
        for entry in self.entries.values() {
            entry.logger.enabled.clear();
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

    fn parent(&self, logger: &Logger) -> Option<&Entry> {
        if logger.standalone {
            return None;
        }
        self.parent_name(&logger.name)
            .map(|name| &self.entries[name])
    }

    fn effective<'a>(&'a self, mut logger: &'a Logger) -> Int {
        loop {
            let level = logger.level.borrow();
            if level.as_small() != Some(0) {
                return level.clone();
            }
            let Some(parent) = self.parent(logger) else {
                return level.clone();
            };
            logger = &parent.logger;
        }
    }
}

thread_local! { static LOGGERS: RefCell<Registry> = RefCell::new(Registry::new()); }

/// Format the time captured by a LogRecord.
fn format_timestamp(pattern: &str, local: &libc::tm) -> String {
    let pattern = pattern.chars().map(u32::from).collect::<Vec<_>>();
    let points = crate::sys::with_process_locale(|| super::time::format_clock(&pattern, local));
    points
        .into_iter()
        .map(|point| char::from_u32(point).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

unsafe fn state(handle: *const Object) -> Rc<Logger> {
    // SAFETY: the caller supplies a rooted native Logger.
    unsafe {
        object(handle).0.with(|storage| match storage {
            Storage::Logger(logger) => logger.clone(),
            _ => super::fail("SystemError", "expected logging.Logger"),
        })
    }
}

unsafe fn name(handle: *const Object) -> StrOwner {
    // SAFETY: the caller retains the logger through the name read.
    unsafe { state(handle) }.name.clone()
}

/// Construct a Logger without adding its name to the manager.
///
/// # Safety
/// The caller owns the exact name and validated integer until this returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_new(name: *const Str, level: usize) -> *const Object {
    // SAFETY: the name receives its own reference; the tagged integer is cloned.
    let (name, level) = unsafe {
        (
            StrOwner::acquire_word(name as usize),
            (*Int::borrow(level)).clone(),
        )
    };
    crate::ObjectOwner::into_raw(Logger::new(name, level, true).handle())
}

/// # Safety
/// A nonzero word points to a rooted exact string. The returned logger is owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_get(word: usize) -> *const Object {
    // SAFETY: non-null optional payloads remain rooted for this call.
    let name = unsafe { (word as *const Str).as_ref() }.map_or("", Str::as_str);
    LOGGERS.with_borrow_mut(|logs| crate::ObjectOwner::into_raw(logs.entry(name).handle.clone()))
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
    let logger = unsafe { state(logger) };
    LOGGERS.with_borrow(|logs| {
        logs.parent(&logger).map_or(0, |parent| {
            crate::ObjectOwner::into_raw(parent.handle.clone()) as usize
        })
    })
}

/// # Safety
/// The logger is rooted; the result owns its exact integer level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_level(logger: *const Object) -> usize {
    // SAFETY: the caller roots this logger while we acquire its name.
    unsafe { state(logger) }.level.borrow().clone().into_raw()
}

/// # Safety
/// The logger is rooted; the result owns the inherited level.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_effective_level(logger: *const Object) -> usize {
    // SAFETY: the caller roots this logger while we acquire its name.
    let logger = unsafe { state(logger) };
    LOGGERS.with_borrow(|logs| logs.effective(&logger).into_raw())
}

/// # Safety
/// The logger and exact tagged level remain rooted during assignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_set_level(logger: *const Object, level: usize) {
    // SAFETY: both arguments are rooted by the caller.
    let (logger, value) = unsafe { (state(logger), Int::borrow(level)) };
    LOGGERS.with_borrow(|logs| logs.set_level(&logger, &value));
}
