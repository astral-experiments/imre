//! Runtime for checked Relar IR on 64-bit hosts.
//!
//! Integer handles are either signed 63-bit values tagged with a low one bit, or
//! live `Rc<BigInt>` pointers to values outside that range. This canonical form
//! is always nonzero and lets compiler range proofs imply representation;
//! extension adapters must normalize incoming integers before exposing them to
//! generated code.
//! String and list handles are live `Rc` pointers of
//! their respective types. ABI arguments borrow these handles; results transfer
//! one owned reference unless an entry point explicitly documents a borrowed
//! result. Release functions consume one reference. Generated code
//! must never forge, confuse, or reuse released handles. There are no callbacks
//! or finalizers in this runtime, so a container access cannot reenter user code.
//! Handles belong to one execution thread. An operation must finish before
//! another operation accesses the same mutable storage; no storage reference or
//! buffer address may escape. Aliased handles are allowed and observe sequential
//! mutations. Future callback or threaded extension boundaries must uphold these
//! invariants before entering this ABI.
//!
//! Recoverable calls enter with pending state clear. Language errors install an
//! owned thread-local exception; generated code checks the entry point's error
//! result immediately, before acquiring a returned word or reading out parameters.
//! Int-list reads/pop return raw zero on failure, and Int-list replacements return
//! status zero on failure or one on success. Other integer failures return tagged
//! zero, pointer failures return null, and generic slot-word failures return an
//! uninterpreted zero. Successful calls do not clear a preexisting exception.
//! These private ABI contracts require a matching compiler and runtime archive.
//! Cleanup/error-state entry points remain valid while pending. Invalid ABI
//! inputs and allocator exhaustion remain fatal; neither uses Rust unwinding.

mod objects;

use objects::exceptions::{Error, Kind, Result, finish};

use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cell::UnsafeCell;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};

const MIN_SMALL: i64 = -(1_i64 << 62);
const MAX_SMALL: i64 = (1_i64 << 62) - 1;
const _: () = assert!(size_of::<usize>() == 8 && align_of::<BigInt>() >= 2);

/// Lists use nonzero elements at least eight bytes wide, and a Vec allocation
/// cannot exceed `isize::MAX` bytes. The compiler shares this inclusive bound
/// when proving that a list length and derived indices fit tagged small integers.
pub const MAX_LIST_LEN: usize = (isize::MAX as usize) / 8;

/// Owns exactly one integer reference; small integers require no allocation.
struct Int(usize);

const _: () = assert!(size_of::<Int>() >= 8 && MAX_LIST_LEN <= MAX_SMALL as usize);

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

/// Interior storage whose aliases are mediated by the closed runtime ABI.
///
/// The higher-ranked closures cannot return references tied to their input.
/// Access remains unsafe: a closure could otherwise reenter through an aliased
/// handle while its storage reference is live. Owned elements may escape after
/// acquisition; storage references and buffer pointers may not.
struct StorageCell<T>(UnsafeCell<T>);

impl<T> StorageCell<T> {
    fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// # Safety
    /// No mutable reference to this storage may be live. The closure must not
    /// reenter an operation on this cell, and no storage reference or buffer
    /// address may escape. All access must remain on the owning thread.
    unsafe fn with<R>(&self, read: impl for<'a> FnOnce(&'a T) -> R) -> R {
        // SAFETY: the caller guarantees nonoverlapping, nonreentrant access.
        read(unsafe { &*self.0.get() })
    }

    /// # Safety
    /// No other reference to this storage may be live. The closure must not
    /// reenter an operation on this cell, and no storage reference or buffer
    /// address may escape. All access must remain on the owning thread. Return
    /// displaced owners so they are dropped only after mutable access ends.
    unsafe fn with_mut<R>(&self, write: impl for<'a> FnOnce(&'a mut T) -> R) -> R {
        // SAFETY: the caller guarantees exclusive, nonreentrant access.
        write(unsafe { &mut *self.0.get() })
    }
}

/// An opaque, nonmoving list allocation. Its resizable buffer stays inside Rust.
pub struct List(StorageCell<Vec<Int>>);

/// Immutable UTF-8 storage behind an opaque `Rc` handle. Short strings fit inside
/// that allocation; longer strings own a separate buffer. LLVM never reads this
/// representation directly.
pub type Str = compact_str::CompactString;

/// Standard output uses bounded block buffering for pipes and files, while
/// terminals flush each printed line. LLVM supplies the process entry point, so
/// input, fatal errors, and normal entry-point return explicitly flush it.
struct Output {
    writer: BufWriter<io::Stdout>,
    line_buffered: bool,
}

static OUTPUT: LazyLock<Mutex<Output>> = LazyLock::new(|| {
    let stdout = io::stdout();
    let line_buffered = stdout.is_terminal();
    Mutex::new(Output {
        writer: BufWriter::with_capacity(8 * 1024, stdout),
        line_buffered,
    })
});

/// Prints one value and a newline, returning with the writer lock released.
/// Only primitive formatters run here; they cannot call user code.
fn write_output(
    write: impl FnOnce(&mut BufWriter<io::Stdout>) -> io::Result<()>,
) -> io::Result<()> {
    let mut output = OUTPUT.lock().unwrap_or_else(|poison| poison.into_inner());
    write(&mut output.writer)?;
    output.writer.write_all(b"\n")?;
    if output.line_buffered {
        output.writer.flush()?;
    }
    Ok(())
}

/// Handles write failures only after releasing the writer lock.
fn output(write: impl FnOnce(&mut BufWriter<io::Stdout>) -> io::Result<()>) {
    if write_output(write).is_err() {
        Error::new(Kind::OSError, "could not write standard output").raise();
    }
}

/// Writes already encoded text without the general formatting machinery.
fn output_bytes(value: &[u8]) {
    output(|writer| writer.write_all(value));
}

/// The caller handles errors only after this function releases the writer lock.
fn flush_stdout() -> io::Result<()> {
    OUTPUT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .writer
        .flush()
}

/// Reports an unrecoverable allocation failure or violated internal ABI contract.
#[cold]
#[inline(never)]
fn fail(kind: &str, message: &str) -> ! {
    // Preserve pending output and the original exception category. A failed
    // best-effort flush must neither recurse through fail nor hold this lock.
    let _ = flush_stdout();
    let _ = writeln!(io::stderr().lock(), "{kind}: {message}");
    std::process::exit(1)
}

/// Flushes pending output before generated code returns from the process entry.
#[unsafe(no_mangle)]
pub extern "C" fn relar_flush_stdout() {
    if flush_stdout().is_err() {
        Error::new(Kind::OSError, "could not write standard output").raise();
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
                    return Error::new(
                        Kind::ZeroDivisionError,
                        "integer division or modulo by zero",
                    )
                    .returning(1);
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
                return Error::new(
                    Kind::ZeroDivisionError,
                    "integer division or modulo by zero",
                )
                .returning(1);
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
// Generated code compares small integers directly. Keep BigInt comparison code
// from inflating its hot callers during ThinLTO.
#[cold]
#[inline(never)]
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
        let mut digits = itoa::Buffer::new();
        output_bytes(digits.format(value).as_bytes());
    } else {
        output(|writer| write!(writer, "{}", value.big()));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_print_bool(value: u8) {
    output_bytes(if value == 0 { b"False" } else { b"True" });
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_print_none() {
    output_bytes(b"None");
}

/// # Safety
/// The byte span must remain readable for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_literal(data: *const u8, length: usize) -> *const Str {
    // SAFETY: the compiler supplies the readable constant span.
    Rc::into_raw(Rc::new(Str::new(unsafe { text(data, length) })))
}

/// # Safety
/// The argument must be a live string handle or null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_retain(value: *const Str) {
    if !value.is_null() {
        // SAFETY: the caller owns a live Rc<Str> reference.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Consumes one string reference, or accepts null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_release(value: *const Str) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live Rc<Str> reference.
        unsafe { Rc::decrement_strong_count(value) };
    }
}

/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_print_str(value: *const Str) {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    output_bytes(unsafe { &*value }.as_bytes());
}

/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_len(value: *const Str) -> usize {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    Int::from_usize(unsafe { &*value }.chars().count()).into_raw()
}

/// # Safety
/// Both arguments must be live string handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_equal(a: *const Str, b: *const Str) -> u8 {
    // SAFETY: both immutable strings are rooted by the caller.
    u8::from(unsafe { *a == *b })
}

/// Parses the prototype's explicit ASCII decimal input format.
///
/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_int_parse(value: *const Str) -> usize {
    // SAFETY: the caller keeps the immutable string alive.
    let source = unsafe { &*value }.trim();
    // Rust's signed decimal parser accepts a subset of our grammar. Ordinary
    // small inputs need neither normalization nor temporary BigInt allocations;
    // separators and larger values still take the checked path below.
    if let Ok(value) = source.parse::<i64>()
        && let Some(value) = Int::small(value)
    {
        return value.into_raw();
    }
    let digits = source.strip_prefix(['+', '-']).unwrap_or(source);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|c| c.is_ascii_digit() || c == b'_')
    {
        return Error::new(Kind::ValueError, "expected an ASCII decimal integer").returning(1);
    }
    finish(
        BigInt::parse_bytes(source.replace('_', "").as_bytes(), 10)
            .map(|value| Int::new(value).into_raw())
            .ok_or_else(|| Error::new(Kind::ValueError, "invalid integer input")),
        1,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_input() -> *const Str {
    // A preceding print must be visible before waiting for the next input line.
    if flush_stdout().is_err() {
        return Error::new(Kind::OSError, "could not write standard output")
            .returning(std::ptr::null());
    }
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) => {
            return Error::new(Kind::EOFError, "EOF when reading a line")
                .returning(std::ptr::null());
        }
        Ok(_) => {}
        Err(_) => {
            return Error::new(Kind::OSError, "could not read standard input")
                .returning(std::ptr::null());
        }
    }
    if line.ends_with('\n') {
        line.pop();
    }
    Rc::into_raw(Rc::new(Str::from(line)))
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_list_new() -> *const List {
    Rc::into_raw(Rc::new(List(StorageCell::new(Vec::new()))))
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

/// Converts an index before a storage borrow, without truncating large integers.
#[inline]
fn decode_index(value: &Int) -> Result<isize> {
    if let Some(raw) = value
        .as_small()
        .and_then(|value| isize::try_from(value).ok())
    {
        Ok(raw)
    } else {
        big_index(&value.big()).ok_or_else(Error::index)
    }
}

/// Normalizes negative indices and rejects out-of-bounds accesses.
// Expose the bounds check to get/set so LLVM can remove their redundant checks.
#[inline]
fn index(raw: isize, length: usize) -> Result<usize> {
    let normalized = if raw < 0 {
        // An underflow wraps above `length`, so the same bounds check rejects it.
        length.wrapping_add_signed(raw)
    } else {
        raw as usize
    };
    if normalized >= length {
        return Err(Error::index());
    }
    Ok(normalized)
}

/// Keeps arbitrary-precision conversion out of the common indexed-access path.
#[cold]
#[inline(never)]
fn big_index(value: &BigInt) -> Option<isize> {
    value.to_isize()
}

/// Returns an owned nonzero integer word, or raw zero with a pending error.
///
/// # Safety
/// The arguments must be live list and integer handles, respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_get(list: *const List, at: usize) -> usize {
    // SAFETY: the caller roots the index; conversion cannot call user code.
    let at = unsafe { Int::borrow(at) };
    let at = match decode_index(&at) {
        Ok(at) => at,
        Err(error) => return error.returning(0),
    };
    // SAFETY: this rooted list is accessed only for the nonreentrant clone.
    let owned = unsafe {
        (&*list)
            .0
            .with(|list| Ok(list[index(at, list.len())?].clone()))
    };
    finish(owned.map(Int::into_raw), 0)
}

/// Reads an element using a compiler-proven nonnegative, decoded index.
/// Returns an owned nonzero integer word, or raw zero with a pending error.
///
/// # Safety
/// `list` must be a rooted list handle. `at` is the exact mathematical index,
/// represented without truncation as a native unsigned integer, not a tagged
/// integer handle. The compiler must prove nonnegativity before selecting this
/// entry point; bounds are still checked, and the result owns one reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_get_index(list: *const List, at: usize) -> usize {
    // SAFETY: the caller roots the list, and the owned clone cannot reenter.
    let owned = unsafe {
        (&*list)
            .0
            .with(|list| list.get(at).cloned().ok_or_else(Error::index))
    };
    finish(owned.map(Int::into_raw), 0)
}

/// Borrows an integer word without retaining it or exposing list storage.
/// Returns a nonzero borrowed word, or raw zero with a pending error.
///
/// # Safety
/// `list` must remain rooted, and its selected element must not be replaced or
/// removed until the returned word has been consumed. The result is a borrowed
/// canonical integer handle, not an owned reference, and must not be released.
/// `at` must be the exact nonnegative native index described by
/// `relar_list_get_index`; current bounds are still checked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_borrow_index(list: *const List, at: usize) -> usize {
    // SAFETY: the caller roots the list and its element throughout consumption.
    // Only the word escapes this nonreentrant access, never a storage reference.
    let result = unsafe {
        (&*list)
            .0
            .with(|list| list.get(at).map(|value| value.0).ok_or_else(Error::index))
    };
    finish(result, 0)
}

/// Returns one after replacement, or zero with a pending error and no mutation.
///
/// # Safety
/// The arguments must be live list and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_set(list: *const List, at: usize, value: usize) -> u8 {
    // SAFETY: the caller roots both integers; conversion and acquisition cannot reenter.
    let (at, value) = unsafe { (Int::borrow(at), Int::borrow(value)) };
    let at = match decode_index(&at) {
        Ok(at) => at,
        Err(error) => return error.returning(0),
    };
    // SAFETY: no storage reference escapes this exclusive, nonreentrant access.
    let old = unsafe {
        (&*list).0.with_mut(|list| {
            let at = index(at, list.len())?;
            Ok(std::mem::replace(&mut list[at], (*value).clone()))
        })
    };
    finish(
        old.map(|old| {
            drop(old);
            1
        }),
        0,
    )
}

/// Replaces an element using a compiler-proven nonnegative, decoded index.
/// Returns one after replacement, or zero with a pending error and no mutation.
///
/// # Safety
/// `list` and `value` must be rooted list and integer handles. `at` must be the
/// exact nonnegative native index described by `relar_list_get_index`. Bounds are
/// checked at this call, including after any intervening mutation through an
/// alias. The replacement is acquired before the displaced owner is released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_set_index(list: *const List, at: usize, value: usize) -> u8 {
    // SAFETY: the caller roots both handles for the operation.
    let value = unsafe { Int::borrow(value) };
    // SAFETY: replacement cannot reenter and returns the displaced owner.
    let old = unsafe {
        (&*list).0.with_mut(|list| {
            let slot = list.get_mut(at).ok_or_else(Error::index)?;
            Ok(std::mem::replace(slot, (*value).clone()))
        })
    };
    finish(
        old.map(|old| {
            drop(old);
            1
        }),
        0,
    )
}

/// Copies an element within one list, acquiring the source before replacement.
/// Returns one after replacement, or zero with a pending error and no mutation.
///
/// # Safety
/// `list` must be rooted. Both indices must be exact nonnegative native indices,
/// as described by `relar_list_get_index`. The source is checked and acquired
/// before the destination is checked, preserving assignment's evaluation order.
/// The displaced owner is released only after mutable storage access ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_copy_index(
    list: *const List,
    destination: usize,
    source: usize,
) -> u8 {
    let mut copied = None;
    // SAFETY: the caller roots the list; cloning cannot reenter, and only the
    // displaced owner escapes this exclusive access. Self-copy acquires first.
    let old = unsafe {
        (&*list).0.with_mut(|list| {
            copied = Some(list.get(source).ok_or_else(Error::index)?.clone());
            let slot = list.get_mut(destination).ok_or_else(Error::index)?;
            Ok(std::mem::replace(slot, copied.take().unwrap()))
        })
    };
    // A failed destination check also releases the acquired source only here,
    // after exclusive storage access has ended.
    drop(copied);
    finish(
        old.map(|old| {
            drop(old);
            1
        }),
        0,
    )
}

/// # Safety
/// The arguments must be live list and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_append(list: *const List, value: usize) {
    // SAFETY: the caller roots both handles; allocation and acquisition cannot
    // reenter the runtime, and no storage reference escapes the closure.
    unsafe {
        let value = Int::borrow(value);
        (&*list).0.with_mut(|list| {
            list.try_reserve(1)
                .unwrap_or_else(|_| fail("MemoryError", "list allocation failed"));
            list.push((*value).clone());
        });
    }
}

/// Returns an owned nonzero integer word, or raw zero with a pending error.
///
/// # Safety
/// The argument must be a live list handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_pop(list: *const List) -> usize {
    // SAFETY: the caller roots the list, and removal returns an independent owner.
    let owned = unsafe {
        (&*list).0.with_mut(|list| {
            list.pop()
                .ok_or_else(|| Error::new(Kind::IndexError, "pop from empty list"))
        })
    };
    finish(owned.map(Int::into_raw), 0)
}

/// # Safety
/// The argument must be a live list handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_len(list: *const List) -> usize {
    // SAFETY: the rooted list is accessed only for a nonreentrant length read.
    let length = unsafe { (&*list).0.with(Vec::len) };
    debug_assert!(length <= MAX_LIST_LEN);
    // Vec<Int>'s allocation limit guarantees that this is a small integer.
    (length << 1) | 1
}

/// # Safety
/// The arguments must be live list and integer handles. Returns a new list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_list_repeat(list: *const List, count: usize) -> *const List {
    // SAFETY: the caller roots the count; integer conversion cannot reenter.
    let count = unsafe { Int::borrow(count) };
    let count = count
        .as_small()
        .map(|value| value as isize)
        .or_else(|| count.big().to_isize());
    let Some(count) = count else {
        return Error::new(Kind::OverflowError, "list repeat count is too large")
            .returning(std::ptr::null());
    };
    if count <= 0 {
        return relar_list_new();
    }
    let count = count as usize;
    // SAFETY: cloning creates owned elements without reentry; only the new
    // vector escapes, and no references into the source buffer are retained.
    let result = unsafe {
        (&*list).0.with(|list| {
            let length = list
                .len()
                .checked_mul(count)
                .filter(|&length| length <= isize::MAX as usize / size_of::<Int>())
                .ok_or_else(|| Error::new(Kind::MemoryError, "list is too large"))?;
            let mut result = Vec::new();
            result
                .try_reserve_exact(length)
                .unwrap_or_else(|_| fail("MemoryError", "list allocation failed"));
            if !list.is_empty() {
                for _ in 0..count {
                    result.extend(list.iter().cloned());
                }
            }
            Ok(result)
        })
    };
    finish(
        result.map(|values| Rc::into_raw(Rc::new(List(StorageCell::new(values))))),
        std::ptr::null(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_assert_fail() {
    Error::new(Kind::AssertionError, "").raise();
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_range_step_zero() {
    Error::new(Kind::ValueError, "range() arg 3 must not be zero").raise();
}

#[cfg(test)]
mod tests {
    use super::*;
    use objects::exceptions::{relar_error_pending, relar_error_take, relar_exception_matches};

    fn clear_index_error() {
        assert_eq!(relar_error_pending(), 1);
        let error = relar_error_take();
        assert!(!error.is_null());
        // SAFETY: take transfers one owned exception; inspect and release it once.
        unsafe {
            assert_eq!(relar_exception_matches(error, Kind::IndexError as u32), 1);
            objects::relar_object_release(error);
        }
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn decimal_input_preserves_signs_separators_and_tag_boundaries() {
        for (source, expected, small) in [
            ("\u{2003}+00017\t", "17", true),
            ("-0", "0", true),
            ("-4_611_686_018_427_387_904", "-4611686018427387904", true),
            ("4611686018427387903", "4611686018427387903", true),
            ("-4611686018427387904", "-4611686018427387904", true),
            ("4611686018427387904", "4611686018427387904", false),
            ("-4611686018427387905", "-4611686018427387905", false),
            ("9223372036854775807", "9223372036854775807", false),
            ("-9223372036854775808", "-9223372036854775808", false),
            ("9223372036854775808", "9223372036854775808", false),
            ("-9223372036854775809", "-9223372036854775809", false),
            (
                "+100_000_000_000_000_000_000_000_000_000_000",
                "100000000000000000000000000000000",
                false,
            ),
        ] {
            let source = Str::new(source);
            // SAFETY: source remains alive throughout the call; Int owns its result.
            let parsed = Int(unsafe { relar_int_parse(&source) });
            assert_eq!(parsed.big().to_string(), expected);
            assert_eq!(parsed.as_small().is_some(), small);
        }
    }

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
            assert_eq!(relar_list_set(list, zero.0, zero.0), 1);
            relar_list_release(list);
            drop(big);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u64 << 63));
        }
    }

    #[test]
    fn decoded_indices_acquire_before_replacing_the_only_stored_owner() {
        let big = Rc::new(BigInt::from(1_u128 << 100));
        let weak = Rc::downgrade(&big);
        let word = Rc::as_ptr(&big) as usize;
        let list = relar_list_new();
        // SAFETY: the list roots the integer after its original owner is dropped.
        // Decoded zero is a native index; every acquired result is released.
        unsafe {
            relar_list_append(list, word);
            drop(big);
            assert_eq!(relar_list_set_index(list, 1, word), 0);
            clear_index_error();
            assert_eq!(weak.strong_count(), 1);
            let huge = Int::new(BigInt::from(1_u128 << 100));
            assert_eq!(relar_list_set(list, huge.0, word), 0);
            clear_index_error();
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(relar_list_set_index(list, 0, word), 1);
            let saved = Int(relar_list_get_index(list, 0));
            assert_eq!(relar_list_set_index(list, 0, Int::small(7).unwrap().0), 1);
            assert_eq!(Int(relar_list_len(list)).as_small(), Some(1));
            relar_list_release(list);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u128 << 100));
            assert!(weak.upgrade().is_some());
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn borrowed_indices_and_list_copy_preserve_bigint_owners() {
        let source = Rc::new(BigInt::from(1_u128 << 100));
        let displaced = Rc::new(BigInt::from(1_u128 << 120));
        let source_weak = Rc::downgrade(&source);
        let displaced_weak = Rc::downgrade(&displaced);
        let list = relar_list_new();
        // SAFETY: the list roots both integers after their original owners are
        // dropped. Borrowed words are consumed before any list mutation; owned
        // results and the list reference are each released exactly once.
        unsafe {
            relar_list_append(list, Rc::as_ptr(&source) as usize);
            relar_list_append(list, Rc::as_ptr(&displaced) as usize);
            drop(source);
            drop(displaced);
            assert_eq!(relar_list_copy_index(list, 2, 0), 0);
            clear_index_error();
            assert_eq!(source_weak.strong_count(), 1);
            assert_eq!(displaced_weak.strong_count(), 1);
            assert_eq!(relar_list_copy_index(list, 1, 2), 0);
            clear_index_error();
            assert_eq!(source_weak.strong_count(), 1);
            assert_eq!(displaced_weak.strong_count(), 1);
            {
                let borrowed = Int::borrow(relar_list_borrow_index(list, 0));
                assert_eq!(borrowed.big().as_ref(), &BigInt::from(1_u128 << 100));
                assert_eq!(source_weak.strong_count(), 1);
            }
            assert_eq!(relar_list_copy_index(list, 0, 0), 1);
            assert_eq!(source_weak.strong_count(), 1);
            assert_eq!(relar_list_copy_index(list, 1, 0), 1);
            assert_eq!(source_weak.strong_count(), 2);
            assert!(displaced_weak.upgrade().is_none());
            assert_eq!(relar_list_set_index(list, 0, Int::small(7).unwrap().0), 1);
            assert_eq!(source_weak.strong_count(), 1);
            {
                let borrowed = Int::borrow(relar_list_borrow_index(list, 1));
                assert_eq!(borrowed.big().as_ref(), &BigInt::from(1_u128 << 100));
                assert_eq!(source_weak.strong_count(), 1);
            }
            let saved = Int(relar_list_get_index(list, 1));
            relar_list_release(list);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u128 << 100));
            assert_eq!(source_weak.strong_count(), 1);
            drop(saved);
        }
        assert!(source_weak.upgrade().is_none());
    }

    #[test]
    fn list_error_words_distinguish_numeric_zero_and_allow_recovery() {
        let list = relar_list_new();
        let zero = Int::small(0).unwrap();
        let last = Int::small(-1).unwrap();
        let before_start = Int::small(-2).unwrap();
        let huge = Int::new(BigInt::from(1_u8) << 100_u32);
        // SAFETY: list and index inputs stay rooted. Successful zero results
        // are immediate integers; error words are inspected without adoption,
        // and each pending exception is cleared before the next fallible call.
        unsafe {
            relar_list_append(list, zero.0);
            assert_eq!(relar_list_get(list, last.0), 1);
            assert_eq!(relar_list_get_index(list, 0), 1);
            assert_eq!(relar_list_borrow_index(list, 0), 1);
            assert_eq!(relar_error_pending(), 0);
            assert_eq!(relar_list_get(list, before_start.0), 0);
            clear_index_error();
            assert_eq!(relar_list_get(list, huge.0), 0);
            clear_index_error();
            assert_eq!(relar_list_get_index(list, 1), 0);
            clear_index_error();
            assert_eq!(relar_list_borrow_index(list, 1), 0);
            clear_index_error();
            assert_eq!(relar_list_set(list, last.0, zero.0), 1);
            assert_eq!(relar_list_pop(list), 1);
            assert_eq!(relar_error_pending(), 0);
            assert_eq!(relar_list_pop(list), 0);
            clear_index_error();
            relar_list_append(list, zero.0);
            assert_eq!(relar_list_pop(list), 1);
            assert_eq!(relar_error_pending(), 0);
            relar_list_release(list);
        }
    }
}
