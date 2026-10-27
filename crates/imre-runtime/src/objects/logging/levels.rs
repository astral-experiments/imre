//! The two dictionaries are independent: renaming a number preserves its old
//! name as an alias, and reassigning a name preserves the other number's label.
use crate::StrOwner;

use super::super::exceptions::string_repr;
use super::super::mappings::{imre_dict_new, imre_dict_set, lookup};
use super::super::{Error, Int, Kind, Object, Str, finish};

struct Levels {
    names: crate::ObjectOwner,
    numbers: crate::ObjectOwner,
}

impl Levels {
    fn new() -> Self {
        // SAFETY: each constructor transfers a fresh dictionary owner.
        let (names, numbers) = unsafe {
            (
                crate::ObjectOwner::from_raw(imre_dict_new(3, 2)),
                crate::ObjectOwner::from_raw(imre_dict_new(2, 3)),
            )
        };
        let levels = Self { names, numbers };
        for (name, number) in [
            ("CRITICAL", 50),
            ("FATAL", 50),
            ("ERROR", 40),
            ("WARN", 30),
            ("WARNING", 30),
            ("INFO", 20),
            ("DEBUG", 10),
            ("NOTSET", 0),
        ] {
            let name = StrOwner::new(Str::from(name));
            let number = Int::small(number).unwrap();
            let key = StrOwner::as_ptr(&name) as usize;
            // SAFETY: each input remains rooted while the typed dictionary acquires it.
            unsafe {
                imre_dict_set(crate::ObjectOwner::as_ptr(&levels.names), key, number.0);
                if name.as_str() != "FATAL" && name.as_str() != "WARN" {
                    imre_dict_set(crate::ObjectOwner::as_ptr(&levels.numbers), number.0, key);
                }
            }
        }
        levels
    }

    fn number(&self, name: &Str) -> Option<Int> {
        // SAFETY: the registry and caller root the dictionary and its exact string key.
        unsafe {
            lookup(
                crate::ObjectOwner::as_ptr(&self.names),
                std::ptr::from_ref(name) as usize,
            )
        }
        .map(|value| Int(value.into_word()))
    }
}

thread_local! { static LEVELS: Levels = Levels::new(); }

/// Looks up a level name in the same process-local registry used for emission and records.
pub(super) fn number_for(name: &Str) -> Option<Int> {
    LEVELS.with(|levels| levels.number(name))
}

/// Resolve a configured name through the same aliased registry as Logger.setLevel.
pub(crate) fn named_level(name: &Str) -> Result<Int, Error> {
    number_for(name).ok_or_else(|| {
        Error::new(
            Kind::ValueError,
            format!("Unknown level: {}", string_repr(name)),
        )
    })
}

pub(crate) fn level_name(level: &Int) -> StrOwner {
    LEVELS.with(|levels| {
        // SAFETY: the registry roots the integer-key dictionary; lookup returns a string owner.
        unsafe { lookup(crate::ObjectOwner::as_ptr(&levels.numbers), level.0) }.map_or_else(
            || StrOwner::new(Str::from(format!("Level {}", level.big()))),
            // SAFETY: every value in this typed dictionary is an owned exact string.
            |value| unsafe { StrOwner::from_raw(value.into_word() as *const Str) },
        )
    })
}

/// # Safety
/// Both exact arguments are borrowed; each dictionary acquires the entries it stores.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_add_level(level: usize, name: *const Str) {
    LEVELS.with(|levels| {
        // SAFETY: the caller roots the integer and string across both dictionary writes.
        unsafe {
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&levels.numbers),
                level,
                name as usize,
            );
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&levels.names),
                name as usize,
                level,
            );
        }
    });
}

/// # Safety
/// The integer is rooted. Returns an independently owned exact string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_level_name(level: usize) -> *const Str {
    // SAFETY: the caller owns this exact tagged integer.
    let level = unsafe { Int::borrow(level) };
    StrOwner::into_raw(level_name(&level))
}

/// # Safety
/// The exact string is rooted. Returns an owned int or str in the union ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_level_for_name(name: *const Str) -> usize {
    // SAFETY: the caller retains the string until lookup completes.
    let name = unsafe { &*name };
    number_for(name).map_or_else(
        || StrOwner::into_raw(StrOwner::new(Str::from(format!("Level {name}")))) as usize,
        super::super::unions::integer,
    )
}

/// # Safety
/// The exact string is rooted. Unknown names raise ValueError for level setters.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_check_level(name: *const Str) -> usize {
    // SAFETY: the caller retains the string until lookup and error formatting finish.
    let name = unsafe { &*name };
    finish(named_level(name).map(Int::into_raw), 0)
}

/// # Safety
/// All inputs are rooted as for basicConfig; the first argument is an exact
/// string. Configuration installs the handler before attempting level lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_config_named(
    name: *const Str,
    fmt: usize,
    datefmt: usize,
    force: bool,
) {
    if !force && super::LOGGERS.with_borrow(|logs| logs.format.is_some()) {
        return;
    }
    // SAFETY: all configuration inputs retain their declared types and lifetime.
    unsafe {
        super::imre_log_config(0, fmt, datefmt, force);
        if super::super::exceptions::imre_error_pending() != 0 {
            return;
        }
        let level = imre_log_check_level(name);
        if level != 0 {
            let level = Int(level);
            let root = crate::ObjectOwner::from_raw(super::imre_log_root(std::ptr::null()));
            super::imre_log_set_level(crate::ObjectOwner::as_ptr(&root), level.0);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_log_names() -> *const Object {
    LEVELS.with(|levels| crate::ObjectOwner::into_raw(levels.names.clone()))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_log_numbers() -> *const Object {
    LEVELS.with(|levels| crate::ObjectOwner::into_raw(levels.numbers.clone()))
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_log_names_copy() -> *const Object {
    let copy = imre_dict_new(3, 2);
    LEVELS.with(|levels| {
        // SAFETY: the source roots all keys/values; only the fresh destination is mutated.
        unsafe {
            levels.names.0.with(|storage| {
                for (name, level) in storage.dictionary().string_entries() {
                    imre_dict_set(copy, std::ptr::from_ref(name) as usize, level);
                }
            });
        }
    });
    copy
}
