//! Runtime for checked Relar IR on 64-bit hosts.
//!
//! Integer handles are either signed 63-bit values tagged with a low one bit, or
//! live `Rc<BigInt>` pointers. String and list handles are live `Rc` pointers of
//! their respective types. ABI arguments borrow these handles; results transfer
//! one owned reference. Release functions consume one reference. Generated code
//! must never forge, confuse, or reuse released handles. There are no callbacks
//! or finalizers in this runtime, so a container borrow cannot reenter user code.

use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cell::RefCell;
use std::io::{self, Write};
use std::mem::ManuallyDrop;
use std::rc::Rc;

const MIN_SMALL: i64 = -(1_i64 << 62);
const MAX_SMALL: i64 = (1_i64 << 62) - 1;
const _: () = assert!(size_of::<usize>() == 8 && align_of::<BigInt>() >= 2);

/// Owns exactly one integer reference; small integers require no allocation.
struct Int(usize);

impl Int {
    fn from_usize(value: usize) -> Self {
        if value <= MAX_SMALL as usize {
            Self((value << 1) | 1)
        } else {
            Self::new(BigInt::from(value))
        }
    }

    fn new(value: BigInt) -> Self {
        if let Some(value) = value.to_i64()
            && let Some(small) = Self::small(value)
        {
            return small;
        }
        Self(Rc::into_raw(Rc::new(value)) as usize)
    }

    fn small(value: i64) -> Option<Self> {
        (MIN_SMALL..=MAX_SMALL)
            .contains(&value)
            .then(|| Self(((value as usize) << 1) | 1))
    }

    fn as_small(&self) -> Option<i64> {
        (self.0 & 1 == 1).then_some((self.0 as i64) >> 1)
    }

    /// Borrows the allocation while the owning handle remains live.
    fn big(&self) -> Cow<'_, BigInt> {
        if let Some(value) = self.as_small() {
            Cow::Owned(BigInt::from(value))
        } else {
            // SAFETY: Int's invariant owns a live Rc<BigInt> for an untagged handle.
            Cow::Borrowed(unsafe { &*(self.0 as *const BigInt) })
        }
    }

    fn into_raw(self) -> usize {
        ManuallyDrop::new(self).0
    }

    /// Borrows an ABI argument without acquiring or consuming its reference.
    unsafe fn borrow(raw: usize) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Self(raw))
    }
}

impl Clone for Int {
    fn clone(&self) -> Self {
        if self.as_small().is_none() {
            // SAFETY: the original Int owns a live strong reference.
            unsafe { Rc::increment_strong_count(self.0 as *const BigInt) };
        }
        Self(self.0)
    }
}

impl Drop for Int {
    fn drop(&mut self) {
        if self.as_small().is_none() {
            // SAFETY: this Int owns exactly the reference being released.
            unsafe { Rc::decrement_strong_count(self.0 as *const BigInt) };
        }
    }
}

/// An opaque, nonmoving list allocation. Its resizable buffer stays inside Rust.
pub struct List(RefCell<Vec<Int>>);

fn fail(kind: &str, message: &str) -> ! {
    let _ = writeln!(io::stderr().lock(), "{kind}: {message}");
    std::process::exit(1)
}

fn output(value: impl std::fmt::Display) {
    if writeln!(io::stdout().lock(), "{value}").is_err() {
        fail("OSError", "could not write standard output");
    }
}

/// Reads a compiler-provided UTF-8 byte span without taking ownership.
unsafe fn text<'a>(data: *const u8, length: usize) -> &'a str {
    // SAFETY: the ABI caller provides a readable span for the duration of the call.
    let bytes = unsafe { std::slice::from_raw_parts(data, length) };
    std::str::from_utf8(bytes).unwrap_or_else(|_| fail("ValueError", "invalid UTF-8"))
}

/// # Safety
/// `value` must be a live integer handle as described in the module contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_retain(value: usize) {
    // SAFETY: the ABI contract provides the borrowed integer.
    let value = unsafe { Int::borrow(value) };
    let _ = (*value).clone().into_raw();
}

/// # Safety
/// Consumes one owned integer reference under the module contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_release(value: usize) {
    drop(Int(value));
}

/// # Safety
/// The byte span must be readable and contain a Ruff-validated integer literal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_literal(data: *const u8, length: usize) -> usize {
    // SAFETY: the caller owns the compiler-generated literal span.
    let literal = unsafe { text(data, length) }.replace('_', "");
    let (digits, radix) = if literal.starts_with("0x") || literal.starts_with("0X") {
        (&literal[2..], 16)
    } else if literal.starts_with("0o") || literal.starts_with("0O") {
        (&literal[2..], 8)
    } else if literal.starts_with("0b") || literal.starts_with("0B") {
        (&literal[2..], 2)
    } else {
        (literal.as_str(), 10)
    };
    Int::new(
        BigInt::parse_bytes(digits.as_bytes(), radix)
            .unwrap_or_else(|| fail("ValueError", "invalid integer literal")),
    )
    .into_raw()
}

/// # Safety
/// Both arguments must be live integer handles; the result is a new reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_binary(a: usize, b: usize, op: u32) -> usize {
    // SAFETY: both handles are borrowed by the ABI call.
    let (a, b) = unsafe { (Int::borrow(a), Int::borrow(b)) };
    if let (Some(x), Some(y)) = (a.as_small(), b.as_small()) {
        let result = match op {
            0 => x.checked_add(y),
            1 => x.checked_sub(y),
            2 => x.checked_mul(y),
            3 | 4 => {
                if y == 0 {
                    fail("ZeroDivisionError", "integer division or modulo by zero");
                }
                let (q, r) = (x / y, x % y);
                let adjust = r != 0 && (r < 0) != (y < 0);
                Some(if op == 3 {
                    q - i64::from(adjust)
                } else if adjust {
                    r + y
                } else {
                    r
                })
            }
            _ => fail("SystemError", "invalid integer operation"),
        };
        if let Some(value) = result.and_then(Int::small) {
            return value.into_raw();
        }
    }
    let (a, b) = (a.big(), b.big());
    let result = match op {
        0 => a.as_ref() + b.as_ref(),
        1 => a.as_ref() - b.as_ref(),
        2 => a.as_ref() * b.as_ref(),
        3 | 4 => {
            if b.is_zero() {
                fail("ZeroDivisionError", "integer division or modulo by zero");
            }
            let (q, r) = (a.as_ref() / b.as_ref(), a.as_ref() % b.as_ref());
            let adjust = !r.is_zero() && r.is_negative() != b.is_negative();
            if op == 3 {
                q - BigInt::from(u8::from(adjust))
            } else if adjust {
                r + b.as_ref()
            } else {
                r
            }
        }
        _ => fail("SystemError", "invalid integer operation"),
    };
    Int::new(result).into_raw()
}

/// # Safety
/// Both arguments must be live integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_compare(a: usize, b: usize) -> i32 {
    // SAFETY: the ABI caller keeps both handles alive.
    let (a, b) = unsafe { (Int::borrow(a), Int::borrow(b)) };
    let order = if let (Some(a), Some(b)) = (a.as_small(), b.as_small()) {
        a.cmp(&b)
    } else {
        a.big().cmp(&b.big())
    };
    match order {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// # Safety
/// The argument must be a live integer handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_print_int(value: usize) {
    // SAFETY: the caller keeps the integer alive while it is formatted.
    let value = unsafe { Int::borrow(value) };
    if let Some(value) = value.as_small() {
        output(value);
    } else {
        output(value.big());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_print_bool(value: u8) {
    output(if value == 0 { "False" } else { "True" });
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_print_none() {
    output("None");
}

/// # Safety
/// The byte span must remain readable for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_literal(data: *const u8, length: usize) -> *const String {
    // SAFETY: the compiler supplies the readable constant span.
    Rc::into_raw(Rc::new(unsafe { text(data, length) }.to_owned()))
}

/// # Safety
/// The argument must be a live string handle or null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_retain(value: *const String) {
    if !value.is_null() {
        // SAFETY: the caller owns a live Rc<String> reference.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Consumes one string reference, or accepts null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_release(value: *const String) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live Rc<String> reference.
        unsafe { Rc::decrement_strong_count(value) };
    }
}

/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_print_str(value: *const String) {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    output(unsafe { &*value });
}

/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_len(value: *const String) -> usize {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    Int::from_usize(unsafe { &*value }.chars().count()).into_raw()
}

/// # Safety
/// Both arguments must be live string handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_equal(a: *const String, b: *const String) -> u8 {
    // SAFETY: both immutable strings are rooted by the caller.
    u8::from(unsafe { *a == *b })
}

/// Parses the prototype's explicit ASCII decimal input format.
///
/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_parse(value: *const String) -> usize {
    // SAFETY: the caller keeps the immutable string alive.
    let source = unsafe { &*value }.trim();
    let digits = source.strip_prefix(['+', '-']).unwrap_or(source);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|c| c.is_ascii_digit() || c == b'_')
    {
        fail("ValueError", "expected an ASCII decimal integer");
    }
    Int::new(
        BigInt::parse_bytes(source.replace('_', "").as_bytes(), 10)
            .unwrap_or_else(|| fail("ValueError", "invalid integer input")),
    )
    .into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_input() -> *const String {
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) => fail("EOFError", "EOF when reading a line"),
        Ok(_) => {}
        Err(_) => fail("OSError", "could not read standard input"),
    }
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Rc::into_raw(Rc::new(line))
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_list_new() -> *const List {
    Rc::into_raw(Rc::new(List(RefCell::new(Vec::new()))))
}

/// # Safety
/// The argument must be a live list handle or null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_retain(value: *const List) {
    if !value.is_null() {
        // SAFETY: the caller owns a live Rc<List> reference.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Consumes one list reference, or accepts null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_release(value: *const List) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live Rc<List> reference.
        unsafe { Rc::decrement_strong_count(value) };
    }
}

/// Normalizes a Python index without truncating a large integer to machine size.
// Expose the bounds check to get/set so LLVM can remove their redundant checks.
#[inline]
fn index(value: &Int, length: usize) -> usize {
    let raw = value
        .as_small()
        .and_then(|value| isize::try_from(value).ok())
        .or_else(|| value.big().to_isize())
        .unwrap_or_else(|| fail("IndexError", "list index out of range"));
    let normalized = if raw < 0 {
        // An underflow wraps above `length`, so the same bounds check rejects it.
        length.wrapping_add_signed(raw)
    } else {
        raw as usize
    };
    if normalized >= length {
        fail("IndexError", "list index out of range");
    }
    normalized
}

/// # Safety
/// The arguments must be live list and integer handles, respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_get(list: *const List, at: usize) -> usize {
    // SAFETY: the caller roots both arguments; this runtime cannot call user code.
    let (list, at) = unsafe { ((&*list).0.borrow(), Int::borrow(at)) };
    list[index(&at, list.len())].clone().into_raw()
}

/// # Safety
/// The arguments must be live list and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_set(list: *const List, at: usize, value: usize) {
    // SAFETY: the caller roots all three arguments for the duration of the call.
    let (mut list, at, value) =
        unsafe { ((&*list).0.borrow_mut(), Int::borrow(at), Int::borrow(value)) };
    let at = index(&at, list.len());
    let old = std::mem::replace(&mut list[at], (*value).clone());
    drop(list);
    drop(old);
}

/// # Safety
/// The arguments must be live list and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_append(list: *const List, value: usize) {
    // SAFETY: the caller roots both arguments; no callbacks occur under the borrow.
    let (mut list, value) = unsafe { ((&*list).0.borrow_mut(), Int::borrow(value)) };
    list.try_reserve(1)
        .unwrap_or_else(|_| fail("MemoryError", "list allocation failed"));
    list.push((*value).clone());
}

/// # Safety
/// The argument must be a live list handle. Returns one owned integer reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_pop(list: *const List) -> usize {
    // SAFETY: the caller roots the list and this operation does not reenter.
    unsafe { &*list }
        .0
        .borrow_mut()
        .pop()
        .unwrap_or_else(|| fail("IndexError", "pop from empty list"))
        .into_raw()
}

/// # Safety
/// The argument must be a live list handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_len(list: *const List) -> usize {
    // SAFETY: the caller roots the list while its length is read.
    Int::from_usize(unsafe { &*list }.0.borrow().len()).into_raw()
}

/// # Safety
/// The arguments must be live list and integer handles. Returns a new list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_repeat(list: *const List, count: usize) -> *const List {
    // SAFETY: both handles are rooted by the caller; integer conversion cannot reenter.
    let (list, count) = unsafe { ((&*list).0.borrow(), Int::borrow(count)) };
    let count = count
        .as_small()
        .map(|value| value as isize)
        .unwrap_or_else(|| {
            count
                .big()
                .to_isize()
                .unwrap_or_else(|| fail("OverflowError", "list repeat count is too large"))
        });
    if count <= 0 {
        return relar_list_new();
    }
    let count = count as usize;
    let length = list
        .len()
        .checked_mul(count)
        .unwrap_or_else(|| fail("MemoryError", "list is too large"));
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .unwrap_or_else(|_| fail("MemoryError", "list allocation failed"));
    if !list.is_empty() {
        for _ in 0..count {
            result.extend(list.iter().cloned());
        }
    }
    Rc::into_raw(Rc::new(List(RefCell::new(result))))
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_assert_fail() -> ! {
    fail("AssertionError", "assertion failed")
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_range_step_zero() -> ! {
    fail("ValueError", "range() arg 3 must not be zero")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_promote_and_demote_without_losing_values() {
        let a = Int::new(BigInt::from(MAX_SMALL));
        let b = Int::new(BigInt::from(1));
        // SAFETY: test-owned handles satisfy the ABI borrowing contract.
        let sum = Int(unsafe { relar_int_binary(a.0, b.0, 0) });
        assert_eq!(sum.big().as_ref(), &BigInt::from(1_u64 << 62));
        assert!(sum.as_small().is_none());
        // SAFETY: test-owned handles satisfy the ABI borrowing contract.
        let original = Int(unsafe { relar_int_binary(sum.0, b.0, 1) });
        assert_eq!(original.as_small(), Some(MAX_SMALL));
    }

    #[test]
    fn floor_division_and_remainder_match_python_signs() {
        for (a, b, q, r) in [(-7, 3, -3, 2), (7, -3, -3, -2), (-7, -3, 2, -1)] {
            let (a, b) = (Int::new(a.into()), Int::new(b.into()));
            // SAFETY: all arguments are live, owned integers.
            let quotient = Int(unsafe { relar_int_binary(a.0, b.0, 3) });
            // SAFETY: all arguments are live, owned integers.
            let remainder = Int(unsafe { relar_int_binary(a.0, b.0, 4) });
            assert_eq!(quotient.as_small(), Some(q));
            assert_eq!(remainder.as_small(), Some(r));
        }
    }

    #[test]
    fn container_ownership_survives_replacement_and_release() {
        let big = Int::new(BigInt::from(1_u64 << 63));
        let list = relar_list_new();
        let zero = Int::new(BigInt::zero());
        // SAFETY: handles are live; each acquired reference is released once below.
        unsafe {
            relar_list_append(list, big.0);
            let saved = Int(relar_list_get(list, zero.0));
            relar_list_set(list, zero.0, zero.0);
            relar_list_release(list);
            drop(big);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u64 << 63));
        }
    }
}
