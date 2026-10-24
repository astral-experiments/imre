//! Named loggers share the root level configured by basicConfig().

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use super::{Int, Object, Storage, StorageCell, Str, acquire_rc, object};

thread_local! {
    static LEVEL: Cell<Option<i64>> = const { Cell::new(None) };
    static LOGGERS: RefCell<HashMap<String, Rc<Object>>> = RefCell::default();
}

/// Return the named logger, acquiring its persistent entry for the caller.
///
/// # Safety
/// The name is a rooted Relar string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_logger_get(name: *const Str) -> *const Object {
    // SAFETY: the caller roots the name for lookup and the logger retains it.
    unsafe {
        LOGGERS.with_borrow_mut(|loggers| {
            let name = acquire_rc::<Str>(name as usize);
            let logger = loggers
                .entry(name.to_string())
                .or_insert_with(|| Rc::new(Object(StorageCell::new(Storage::Logger(name)))));
            Rc::into_raw(Rc::clone(logger))
        })
    }
}

/// Install the first basic configuration, as Python does for an existing root handler.
///
/// # Safety
/// The level is a live integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_logger_configure(level: usize) {
    // SAFETY: the caller supplies an exact integer level.
    let level = unsafe { Int::borrow(level) };
    LEVEL.with(|root| {
        if root.get().is_none() {
            let level = level.as_small().unwrap_or_else(|| {
                if level.big().sign() == num_bigint::Sign::Minus {
                    i64::MIN
                } else {
                    i64::MAX
                }
            });
            root.set(Some(level));
        }
    })
}

/// Emit a configured INFO or DEBUG message using the default format.
///
/// # Safety
/// The caller roots a named logger and its formatted text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_logger_write(
    logger: *const Object,
    level: u32,
    message: *const Str,
) {
    if i64::from(level) < LEVEL.get().unwrap_or(30) {
        return;
    }
    // SAFETY: both arguments remain rooted while formatting and writing.
    unsafe {
        object(logger).0.with(|storage| {
            let Storage::Logger(name) = storage else {
                unreachable!("typed logger receiver")
            };
            let label = if level == 10 { "DEBUG" } else { "INFO" };
            let line = Str::new(format!("{label}:{name}:{}", *message));
            crate::relar_stderr_line(&line);
        });
    }
}
