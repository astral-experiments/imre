//! The interpreter's recursion limit is shared; each native thread owns its active frame depth.
use crate::{Error, Int, Kind, Str, StrOwner};
use num_traits::ToPrimitive;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicI32, Ordering};

static LIMIT: AtomicI32 = AtomicI32::new(1000);
thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static MODULES: RefCell<Vec<StrOwner>> = const { RefCell::new(Vec::new()) };
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

/// Enter a Python source frame whose module name can be inspected by a callee.
///
/// # Safety
/// The compiler transfers one owned reference to its pooled module name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_named_frame_enter(module: *const Str) {
    // SAFETY: the pooled name's reference is transferred to this frame.
    let name = unsafe { StrOwner::from_raw(module) };
    MODULES.with_borrow_mut(|frames| frames.push(name));
    imre_recursion_enter();
}

/// Pop the same frame on both return and exceptional unwind.
#[unsafe(no_mangle)]
pub extern "C" fn imre_named_frame_leave() {
    MODULES.with_borrow_mut(|frames| {
        frames.pop();
    });
    imre_recursion_leave();
}

/// Return a retained module name, or the absent optional string beyond the stack.
///
/// # Safety
/// The caller keeps the tagged Python index alive through the conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sys_getframemodulename(raw: usize) -> usize {
    // SAFETY: the caller roots the index until the query returns.
    let value = unsafe { Int::borrow(raw) };
    let Some(depth) = c_int(&value) else {
        return Error::new(
            Kind::OverflowError,
            "Python int too large to convert to C int",
        )
        .returning(0);
    };
    MODULES.with_borrow(|frames| {
        frames
            .iter()
            .rev()
            .nth(depth.max(0) as usize)
            .map_or(0, |name| StrOwner::into_raw(name.clone()) as usize)
    })
}

/// Convert the Python index to the C int used for a recursion limit or frame depth.
fn c_int(value: &Int) -> Option<i32> {
    if let Some(small) = value.as_small() {
        i32::try_from(small).ok()
    } else {
        value.big().to_i32()
    }
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
    match c_int(&value) {
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
