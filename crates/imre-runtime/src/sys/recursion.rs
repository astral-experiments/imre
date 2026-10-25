//! The interpreter's recursion limit is shared; each native thread owns its active frame depth.
use crate::{Error, Int, Kind};
use num_traits::ToPrimitive;
use std::cell::Cell;
use std::sync::atomic::{AtomicI32, Ordering};

static LIMIT: AtomicI32 = AtomicI32::new(1000);
thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Count an attempted entry until its normal or exceptional cleanup leaves the frame.
#[unsafe(no_mangle)]
pub extern "C" fn imre_recursion_enter() {
    let depth = DEPTH.get() + 1;
    DEPTH.set(depth);
    if depth > LIMIT.load(Ordering::Relaxed) as u32 {
        Error::new(Kind::RecursionError, "maximum recursion depth exceeded").raise();
    }
}

/// Release one source frame, including an entry rejected at the limit.
#[unsafe(no_mangle)]
pub extern "C" fn imre_recursion_leave() {
    DEPTH.set(DEPTH.get() - 1);
}

/// Return the limit observed by the running interpreter.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_getrecursionlimit() -> usize {
    Int::from_usize(LIMIT.load(Ordering::Relaxed) as usize).into_raw()
}

/// Change the limit only when the current thread is still below it.
///
/// # Safety
/// The caller keeps its tagged Python integer alive for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_setrecursionlimit(raw: usize) {
    // SAFETY: the caller roots the integer for the duration of this conversion.
    let value = unsafe { Int::borrow(raw) };
    let limit = if let Some(small) = value.as_small() {
        i32::try_from(small).ok()
    } else {
        value.big().to_i32()
    };
    match limit {
        None => Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )
        .raise(),
        Some(limit) if limit < 1 => {
            Error::new(
                Kind::ValueError,
                "recursion limit must be greater or equal than 1",
            )
            .raise();
        }
        Some(limit) => {
            let depth = DEPTH.get();
            if depth >= limit as u32 {
                Error::new(Kind::RecursionError, format!(
                    "cannot set the recursion limit to {limit} at the recursion depth {depth}: the limit is too low"
                )).raise();
            } else {
                LIMIT.store(limit, Ordering::Relaxed);
            }
        }
    }
}
