//! Runtime for checked Imre IR on 64-bit hosts.
//!
//! Integer handles are either signed 63-bit values tagged with a low one bit, or
//! live `Rc<BigInt>` pointers to values outside that range. This canonical form
//! is always nonzero and lets compiler range proofs imply representation;
//! extension adapters must normalize incoming integers before exposing them to
//! generated code.
//! String and list handles are live reference-counted pointers of
//! their respective types. ABI arguments borrow these handles; results transfer
//! one owned reference unless an entry point explicitly documents a borrowed
//! result. Release functions consume one reference. Generated code
//! must never forge, confuse, or reuse released handles. Container operations
//! run without calling user code. Rebuilt C extensions can invoke callbacks
//! after the individual container operation has released all storage borrows.
//! Final resource release may close a file, but never installs a language error.
//! Handles belong to one execution thread. An operation must finish before
//! another operation accesses the same mutable storage; no storage reference may
//! escape. Buffer addresses stay private except for explicitly documented readonly
//! loop views whose compiler proof keeps the owner rooted and prevents all storage
//! mutation. Such views never enter language values or escape the loop. Aliased
//! handles are allowed and observe sequential mutations. Python.h APIs and
//! callbacks run on the same thread as the originating extension call.
//!
//! Recoverable calls enter with pending state clear. Language errors install an
//! owned thread-local exception; generated code checks the entry point's error
//! result immediately, before acquiring a returned word or reading out parameters.
//! Int-list reads/pop and text writes return raw zero on failure; Int-list replacements return
//! status zero on failure or one on success. Other integer failures return tagged
//! zero, pointer failures return null, and generic slot-word failures return an
//! uninterpreted zero. Successful calls do not clear a preexisting exception.
//! These private ABI contracts require a matching compiler and runtime archive.
//! Cleanup/error-state entry points remain valid while pending. Invalid ABI
//! inputs and allocator exhaustion remain fatal; neither uses Rust unwinding.

mod extensions;
mod floats;
mod shared;
mod string_owner;
use shared::Shared;
type ObjectOwner = Shared<objects::Object>;
type WeakObject = shared::Weak<objects::Object>;
use string_owner::{StrOwner, WeakStr};
mod integer_power;
mod integer_text;
mod nominal_classes;
mod objects;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod run_start;

use objects::exceptions::{Error, Kind, Result, finish};

use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::io::{self, IsTerminal, Write};
use std::mem::ManuallyDrop;
use std::os::fd::AsFd;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex, Once};

const MIN_SMALL: i64 = -(1_i64 << 62);
const MAX_SMALL: i64 = (1_i64 << 62) - 1;
const _: () = assert!(size_of::<usize>() == 8 && align_of::<BigInt>() >= 2);

/// Lists use nonzero elements at least eight bytes wide, and a Vec allocation
/// cannot exceed `isize::MAX` bytes. The compiler shares this inclusive bound
/// when proving that a list length and derived indices fit tagged small integers.
pub const MAX_LIST_LEN: usize = (isize::MAX as usize) / 8;

/// Owns exactly one integer reference; small integers require no allocation.
#[repr(transparent)]
struct Int(usize);

const _: () =
    assert!(size_of::<Int>() == 8 && align_of::<Int>() == 8 && MAX_LIST_LEN <= MAX_SMALL as usize);

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
/// acquisition; storage references and buffer pointers may not escape these
/// closures. Readonly loop views use a separate whole-loop proof.
#[repr(transparent)]
struct StorageCell<T>(UnsafeCell<T>);

impl<T> StorageCell<T> {
    fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    fn into_inner(self) -> T {
        self.0.into_inner()
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

/// An opaque, nonmoving list allocation. Only a proven readonly loop borrows its buffer.
pub struct List(
    StorageCell<Vec<Int>>,
    // Capacity charged to this list in the thread's range-hint budget.
    Cell<usize>,
);

impl List {
    fn new(values: Vec<Int>) -> Self {
        Self(StorageCell::new(values), Cell::new(0))
    }

    fn release_range_hint(&self) {
        let slots = self.1.replace(0);
        if slots != 0 {
            let _ = RANGE_HINT_SLOTS.try_with(|used| used.set(used.get() - slots));
        }
    }
}

// An unallocated Vec never reads, writes, or frees this address. A real push
// replaces it with an allocation, even if a later pop empties the list again.
static SORTING_EMPTY: Int = Int(1);

fn sorting_empty_ints() -> Vec<Int> {
    let marker = std::ptr::from_ref(&SORTING_EMPTY).cast_mut();
    // SAFETY: a zero-capacity Vec only requires a nonnull pointer aligned for Int.
    unsafe { Vec::from_raw_parts(marker, 0, 0) }
}

fn is_sorting_empty_ints(values: &[Int]) -> bool {
    std::ptr::eq(values.as_ptr(), &SORTING_EMPTY)
}

const MAX_REUSED_LIST_CAPACITY: usize = 32_768;
const MAX_LIVE_RANGE_HINT_SLOTS: usize = 2 * MAX_REUSED_LIST_CAPACITY;

thread_local! {
    // A loop can break or raise before filling its output. Bound speculative
    // capacity across all live lists, including buffers obtained from the cache.
    static RANGE_HINT_SLOTS: Cell<usize> = const { Cell::new(0) };
    // Only empty buffers enter these newest-first slots: no integer owners or
    // list identities survive here. Retention is bounded to 512 KiB per thread.
    static LIST_BUFFERS: RefCell<[Vec<Int>; 2]> = const {
        RefCell::new([Vec::new(), Vec::new()])
    };
    // Headers contain neither element owners nor buffer capacity. Only unique
    // allocations without Weak references enter these bounded slots.
    static LIST_HEADERS: RefCell<[Option<Rc<List>>; 2]> = const {
        RefCell::new([None, None])
    };
}

/// Publishes a fresh list identity, reusing an unobservable empty header when
/// available. Live aliases and Weak references never enter the private cache.
fn new_list(values: Vec<Int>) -> *const List {
    let cached = LIST_HEADERS
        .try_with(|headers| {
            let Ok(mut headers) = headers.try_borrow_mut() else {
                return None;
            };
            let next = headers[1].take();
            std::mem::replace(&mut headers[0], next)
        })
        .ok()
        .flatten();
    let owner = if let Some(mut owner) = cached {
        let list = Rc::get_mut(&mut owner).expect("cached list header is unique");
        let storage = list.0.0.get_mut();
        debug_assert!(storage.is_empty() && storage.capacity() == 0);
        *storage = values;
        owner
    } else {
        Rc::new(List::new(values))
    };
    Rc::into_raw(owner)
}

/// Consumes a list owner, caching at most two headers whose identities can no
/// longer be observed. Elements are destroyed before caching, using the same
/// BigInt cleanup and bounded buffer reuse as ordinary List destruction.
/// `Rc::get_mut` excludes Weak observers as well as other strong owners.
fn recycle_list_header(mut owner: Rc<List>) {
    let Some(list) = Rc::get_mut(&mut owner) else {
        return;
    };
    list.release_range_hint();
    let values = std::mem::take(list.0.0.get_mut());
    drop(List::new(values));
    let evicted = LIST_HEADERS.try_with(|headers| {
        let Ok(mut headers) = headers.try_borrow_mut() else {
            return Some(owner);
        };
        let previous = headers[0].replace(owner);
        std::mem::replace(&mut headers[1], previous)
    });
    // Reentry and TLS destruction fall back to ordinary drop. Empty List::drop
    // never accesses this header cache, including when cached owners are evicted.
    drop(evicted);
}

/// Reuses the smallest suitable empty buffer for a known result size. Limiting
/// excess capacity avoids attaching large allocations to small, long-lived lists.
/// Cache misses leave allocation and its existing error handling to the caller.
fn take_list_buffer(requested: usize) -> Vec<Int> {
    if requested == 0 || requested > MAX_REUSED_LIST_CAPACITY {
        return Vec::new();
    }
    LIST_BUFFERS
        .try_with(|buffers| {
            let Ok(mut buffers) = buffers.try_borrow_mut() else {
                return Vec::new();
            };
            let candidate = buffers
                .iter()
                .enumerate()
                .filter(|(_, buffer)| (requested..=requested * 2).contains(&buffer.capacity()))
                .min_by_key(|(_, buffer)| buffer.capacity())
                .map(|(index, _)| index);
            let Some(index) = candidate else {
                return Vec::new();
            };
            let buffer = std::mem::take(&mut buffers[index]);
            if index == 0 {
                buffers.swap(0, 1);
            }
            buffer
        })
        .unwrap_or_default()
}

/// Retains only empty, bounded buffers. Evicted storage is freed after the TLS
/// borrow ends; reentry or TLS destruction simply frees the incoming buffer.
fn recycle_list_buffer(values: &mut Vec<Int>) {
    if !values.is_empty() || !(1..=MAX_REUSED_LIST_CAPACITY).contains(&values.capacity()) {
        return;
    }
    let buffer = std::mem::take(values);
    let evicted = LIST_BUFFERS.try_with(|buffers| {
        let Ok(mut buffers) = buffers.try_borrow_mut() else {
            return buffer;
        };
        let previous = std::mem::replace(&mut buffers[0], buffer);
        std::mem::replace(&mut buffers[1], previous)
    });
    drop(evicted);
}

impl Drop for List {
    /// A vectorizable tag reduction skips element cleanup when every integer is
    /// immediate. Any BigInt leaves the ordinary Vec drop path responsible for
    /// releasing all owners, including repeated references to the same integer.
    fn drop(&mut self) {
        self.release_range_hint();
        let values = self.0.0.get_mut();
        let common_tags = values.iter().fold(1_usize, |tags, value| tags & value.0);
        if common_tags & 1 != 0 {
            // SAFETY: a low one bit proves each Int owns no allocation, so its
            // destructor has no effect. Exclusive List destruction owns the Vec;
            // clearing its length permits bounded buffer reuse without owners.
            unsafe { values.set_len(0) };
            recycle_list_buffer(values);
        }
    }
}

/// Immutable UTF-8 storage behind an opaque shared handle. Short strings fit inside
/// that allocation; longer strings own a separate buffer. LLVM never reads this
/// representation directly.
pub type Str = compact_str::CompactString;

/// Immutable byte storage; native handles share ownership and expose no mutable view.
pub type Bytes = Vec<u8>;
/// All byte handles crossing the native ABI point into this strong owner.
pub(crate) type BytesOwner = triomphe::Arc<Bytes>;

/// The caller roots a byte handle for the duration of this ownership increment.
unsafe fn acquire_bytes(word: usize) -> BytesOwner {
    if word == 0 {
        fail("TypeError", "unexpected null runtime handle");
    }
    // SAFETY: the caller supplies the data pointer of a live BytesOwner.
    unsafe { triomphe::ArcBorrow::from_ptr(word as *const Bytes) }.clone_arc()
}

unsafe fn retain_bytes(value: *const Bytes) {
    // SAFETY: the caller roots this exact bytes handle.
    let _ = BytesOwner::into_raw(unsafe { acquire_bytes(value as usize) });
}

unsafe fn release_bytes(value: *const Bytes) {
    // SAFETY: the caller transfers one strong reference.
    drop(unsafe { BytesOwner::from_raw(value) });
}

/// Generated main installs Python's I/O signal dispositions before user code.
/// Native callers that enter through a writer use the same initialization.
fn initialize_io() {
    static INITIALIZED: Once = Once::new();
    INITIALIZED.call_once(|| {
        for signal in [libc::SIGPIPE, libc::SIGXFSZ] {
            // SAFETY: SIG_IGN is a valid disposition for both fixed signals.
            if unsafe { libc::signal(signal, libc::SIG_IGN) } == libc::SIG_ERR {
                // Avoid fail(), which would recursively initialize OUTPUT here.
                let error = io::Error::last_os_error();
                let _ = writeln!(
                    io::stderr().lock(),
                    "OSError: could not initialize I/O signals: {error}"
                );
                std::process::exit(1);
            }
        }
    });
}

/// Standard output uses bounded block buffering for pipes and files, while
/// terminals flush each printed line. LLVM supplies the process entry point, so
/// input, fatal errors, and normal entry-point return explicitly flush it.
struct Output<W = RawStdout> {
    raw: BinaryOutput<W>,
    text: text_output::PendingText,
    line_buffered: bool,
    captures: Vec<Vec<u8>>,
    signature_emitted: bool,
}

struct BinaryOutput<W> {
    inner: W,
    buffer: Vec<u8>,
    written: usize,
}

/// A buffered write can accept bytes into the kernel and its own buffer before it stalls.
#[derive(Debug)]
struct BlockedWrite {
    errno: i32,
    accepted: usize,
}

impl BlockedWrite {
    const DESCRIPTION: &'static str = "write could not complete without blocking";

    fn error(error: io::Error, accepted: usize) -> io::Error {
        if error.kind() != io::ErrorKind::WouldBlock {
            return error;
        }
        io::Error::new(
            io::ErrorKind::WouldBlock,
            Self {
                errno: error.raw_os_error().unwrap_or(libc::EAGAIN),
                accepted,
            },
        )
    }
}

impl std::fmt::Display for BlockedWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(Self::DESCRIPTION)
    }
}

impl std::error::Error for BlockedWrite {}

/// Writes directly to fd 1 so the native binary buffer observes descriptor errors.
struct RawStdout;

impl Write for RawStdout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        rustix::io::write(io::stdout(), bytes).map_err(Into::into)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct StdioSettings {
    stdout_capacity: usize,
    stderr_capacity: usize,
    stdout_line_buffered: bool,
}

static STDIO_SETTINGS: LazyLock<StdioSettings> = LazyLock::new(|| {
    fn capacity(stream: impl AsFd) -> usize {
        let block_size = rustix::fs::fstat(stream).map_or(0, |stat| stat.st_blksize.into());
        text_output::binary_capacity(block_size)
    }
    let stdout = io::stdout();
    StdioSettings {
        stdout_capacity: capacity(&stdout),
        stderr_capacity: capacity(io::stderr()),
        stdout_line_buffered: stdout.is_terminal(),
    }
});

/// Capture buffering from the startup descriptors before user code can replace them.
#[unsafe(no_mangle)]
pub extern "C" fn imre_stdio_init() {
    prepare_stdio(false, None);
}

/// Ignore Python environment settings for an isolated command without changing os.environ.
#[unsafe(no_mangle)]
pub extern "C" fn imre_stdio_isolated_init() {
    prepare_stdio(true, None);
}

/// Initialize streams with the effective flags of a native Python command.
#[unsafe(no_mangle)]
pub extern "C" fn imre_stdio_command_init(isolated: u32, utf8: u32) {
    prepare_stdio(isolated != 0, Some(utf8 != 0));
}

fn prepare_stdio(isolated: bool, utf8: Option<bool>) {
    initialize_io();
    sys::initialize_encoding(isolated, utf8);
    LazyLock::force(&STDIO_SETTINGS);
}

impl<W: Write> Output<W> {
    /// Line buffering inspects the input characters before the codec maps them to bytes.
    fn write_encoded(&mut self, encoded: &[u8], original: &[u8]) -> io::Result<()> {
        let flush_line = self.line_buffered && memchr::memchr2(b'\r', b'\n', original).is_some();
        self.text.write(&mut self.raw, encoded, flush_line)
    }

    fn with_stream(stream: W, capacity: usize, line_buffered: bool) -> Self {
        // With zero capacity every write goes directly to the stream.
        let mut buffer = Vec::new();
        let _ = buffer.try_reserve_exact(capacity);
        Self {
            raw: BinaryOutput {
                inner: stream,
                buffer,
                written: 0,
            },
            text: text_output::PendingText::default(),
            line_buffered,
            captures: Vec::new(),
            signature_emitted: false,
        }
    }
}

impl<W: Write> Write for Output<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_all(bytes)?;
        Ok(bytes.len())
    }

    #[inline]
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if let Some(captured) = self.captures.last_mut() {
            captured
                .try_reserve(bytes.len())
                .map_err(|_| io::ErrorKind::OutOfMemory)?;
            captured.extend_from_slice(bytes);
            return Ok(());
        }
        self.write_encoded(bytes, bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.captures.is_empty() {
            return Ok(());
        }
        self.text.flush(&mut self.raw)
    }
}

impl<W: Write> BinaryOutput<W> {
    /// Buffer writes to a file or pipe up to its configured binary capacity.
    fn with_buffer(buffer: Vec<u8>, inner: W) -> Self {
        Self {
            inner,
            buffer,
            written: 0,
        }
    }

    #[cold]
    fn write_all_slow(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        let supplied = bytes.len();
        if let Err(error) = self.flush_buffer() {
            return self.buffer_after_error(bytes, 0, error);
        }
        while !bytes.is_empty() && bytes.len() >= self.buffer.capacity() {
            match self.inner.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return self.buffer_after_error(bytes, supplied - bytes.len(), error),
            }
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    /// After a nonblocking write stalls, keep as much of the new input as fits
    /// behind the unwritten portion of the previous buffer.
    fn buffer_after_error(
        &mut self,
        bytes: &[u8],
        sent: usize,
        error: io::Error,
    ) -> io::Result<()> {
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        self.buffer.drain(..self.written);
        self.written = 0;
        let count = bytes.len().min(self.buffer.capacity() - self.buffer.len());
        self.buffer.extend_from_slice(&bytes[..count]);
        if count < bytes.len() {
            return Err(BlockedWrite::error(error, sent + count));
        }
        Ok(())
    }

    fn flush_buffer(&mut self) -> io::Result<()> {
        while self.written < self.buffer.len() {
            match self.inner.write(&self.buffer[self.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => self.written += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.buffer.clear();
        self.written = 0;
        Ok(())
    }
}

impl<W: Write> Write for BinaryOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_all(bytes)?;
        Ok(bytes.len())
    }

    #[inline]
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() < self.buffer.capacity()
            && bytes.len() <= self.buffer.capacity() - self.buffer.len()
        {
            self.buffer.extend_from_slice(bytes);
            return Ok(());
        }
        self.write_all_slow(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_buffer()
            .and_then(|()| self.inner.flush())
            .map_err(|error| BlockedWrite::error(error, 0))
    }
}

static OUTPUT: LazyLock<Mutex<Output>> = LazyLock::new(|| {
    initialize_io();
    Mutex::new(Output::with_stream(
        RawStdout,
        STDIO_SETTINGS.stdout_capacity,
        STDIO_SETTINGS.stdout_line_buffered,
    ))
});

mod doctest;
mod sys;
mod text_output;

/// Writes a UTF-8 line through the native standard-error handle.
///
/// # Safety
/// The caller roots the immutable string until the write returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_stderr_line(text: *const Str) {
    // SAFETY: the caller owns the text for this call.
    let text = unsafe { &*text };
    if let Err(error) = sys::write_stderr(text, true) {
        objects::errno_error(error).raise();
    }
}

/// Raise encoding and descriptor errors after releasing the stdout lock.
fn output_text(value: &str) {
    if let Err(error) = sys::write_stdout(value, true) {
        error.raise();
    }
}

/// The caller handles errors only after this function releases the writer lock.
fn flush_stdout() -> io::Result<()> {
    let Some(output) = LazyLock::get(&OUTPUT) else {
        return Ok(());
    };
    output
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .flush()
}

/// Reports an unrecoverable allocation failure or violated internal ABI contract.
#[cold]
#[inline(never)]
fn fail(kind: &str, message: &str) -> ! {
    // Preserve pending output and the original exception category. A failed
    // best-effort flush must neither recurse through fail nor hold this lock.
    let _ = flush_stdout();
    let _ = sys::flush_stderr();
    let _ = writeln!(io::stderr().lock(), "{kind}: {message}");
    std::process::exit(1)
}

/// Flush both standard streams and return the process's final exit status.
#[unsafe(no_mangle)]
pub extern "C" fn imre_finish_stdio() -> i32 {
    let stderr = sys::flush_stderr();
    objects::exceptions::status_after_flush(0, flush_stdout(), stderr)
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
pub unsafe extern "C" fn imre_int_retain(value: usize) {
    // SAFETY: the ABI contract provides the borrowed integer.
    let value = unsafe { Int::borrow(value) };
    let _ = (*value).clone().into_raw();
}

/// # Safety
/// Consumes one owned integer reference under the module contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_release(value: usize) {
    drop(Int(value));
}

/// # Safety
/// The byte span must be readable and contain a Ruff-validated integer literal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_literal(data: *const u8, length: usize) -> usize {
    // SAFETY: the caller owns the compiler-generated literal span.
    let literal = unsafe { text(data, length) };
    Int::new(
        parse_integer_literal(literal)
            .unwrap_or_else(|| fail("ValueError", "invalid integer literal")),
    )
    .into_raw()
}

/// Parse the spelling of a source integer, including signed literals stored in
/// an argparse schema. Runtime conversion of a string argument stays decimal.
fn parse_integer_literal(literal: &str) -> Option<BigInt> {
    let literal = literal.replace('_', "");
    let (literal, negative) = literal
        .strip_prefix('-')
        .map_or((literal.as_str(), false), |s| (s, true));
    let (digits, radix) = if literal.starts_with("0x") || literal.starts_with("0X") {
        (&literal[2..], 16)
    } else if literal.starts_with("0o") || literal.starts_with("0O") {
        (&literal[2..], 8)
    } else if literal.starts_with("0b") || literal.starts_with("0B") {
        (&literal[2..], 2)
    } else {
        (literal, 10)
    };
    BigInt::parse_bytes(digits.as_bytes(), radix).map(|value| if negative { -value } else { value })
}

/// # Safety
/// Both arguments must be live integer handles; the result is a new reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_binary(a: usize, b: usize, op: u32) -> usize {
    // SAFETY: both handles are borrowed by the ABI call.
    let (a, b) = unsafe { (Int::borrow(a), Int::borrow(b)) };
    if matches!(op, 8 | 9) {
        return finish(integer_shift(&a, &b, op == 8).map(Int::into_raw), 1);
    }
    if let (Some(x), Some(y)) = (a.as_small(), b.as_small()) {
        let result = match op {
            0 => x.checked_add(y),
            1 => x.checked_sub(y),
            2 => x.checked_mul(y),
            5 => Some(x & y),
            6 => Some(x | y),
            7 => Some(x ^ y),
            3 | 4 => {
                if y == 0 {
                    return Error::new(Kind::ZeroDivisionError, "division by zero").returning(1);
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
        5 => a.as_ref() & b.as_ref(),
        6 => a.as_ref() | b.as_ref(),
        7 => a.as_ref() ^ b.as_ref(),
        3 | 4 => {
            if b.is_zero() {
                return Error::new(Kind::ZeroDivisionError, "division by zero").returning(1);
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

/// Validates the count before zero shortcuts. Unbounded right shifts retain
/// the infinite sign extension; impossible left shifts follow CPython 3.14's
/// signed-64-bit bit-count limit without attempting an enormous allocation.
fn integer_shift(value: &Int, count: &Int, left: bool) -> Result<Int> {
    let shift = if let Some(count) = count.as_small() {
        if count < 0 {
            return Err(Error::new(Kind::ValueError, "negative shift count"));
        }
        Some(count as u64)
    } else {
        let count = count.big();
        if count.is_negative() {
            return Err(Error::new(Kind::ValueError, "negative shift count"));
        }
        count.to_u64()
    };
    if value.as_small() == Some(0) || shift == Some(0) {
        return Ok(value.clone());
    }
    if let Some(value) = value.as_small() {
        if !left {
            return Ok(Int::small(value >> shift.unwrap_or(u64::MAX).min(63)).unwrap());
        }
        if let Some(shift) = shift.filter(|&shift| shift < 63)
            && let Ok(result) = i64::try_from(i128::from(value) << shift)
            && let Some(result) = Int::small(result)
        {
            return Ok(result);
        }
    }
    let value = value.big();
    if !left {
        return Ok(if shift.is_none_or(|shift| shift >= value.bits()) {
            Int::small(-i64::from(value.is_negative())).unwrap()
        } else {
            Int::new(value.as_ref() >> shift.unwrap() as usize)
        });
    }
    let shift = shift
        .filter(|&shift| shift <= i64::MAX as u64)
        .ok_or_else(|| Error::new(Kind::OverflowError, "too many digits in integer"))?;
    // CPython's 64-bit limit counts 30-bit digits, including the carry digit
    // reserved before normalization. This also prevents native size overflow.
    let digits = value.bits().div_ceil(30) + shift / 30 + u64::from(shift % 30 != 0);
    if digits > (i64::MAX as u64 - 1) / 30 {
        return Err(Error::new(
            Kind::OverflowError,
            "too many digits in integer",
        ));
    }
    Ok(Int::new(value.as_ref() << shift as usize))
}

/// # Safety
/// Both arguments must be live integer handles.
// Generated code compares small integers directly. Keep BigInt comparison code
// from inflating its hot callers during ThinLTO.
#[cold]
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_compare(a: usize, b: usize) -> i32 {
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
pub unsafe extern "C" fn imre_print_int(value: usize) {
    // SAFETY: the caller keeps the integer alive while it is formatted.
    let value = unsafe { Int::borrow(value) };
    if let Some(value) = value.as_small() {
        let mut digits = itoa::Buffer::new();
        output_text(digits.format(value));
    } else {
        output_text(&value.big().to_string());
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_print_bool(value: u32) {
    output_text(if value == 0 { "False" } else { "True" });
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_print_none() {
    output_text("None");
}

/// Formats an integer without changing its ownership or arbitrary precision.
///
/// # Safety
/// `value` must be a rooted canonical integer handle. Returns an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_str(value: usize) -> *const Str {
    // SAFETY: the caller keeps the integer alive through formatting.
    let value = unsafe { Int::borrow(value) };
    match integer_text::decimal_string(&value).and_then(checked_owned_str) {
        Ok(text) => text,
        Err(error) => error.returning(std::ptr::null()),
    }
}

/// Transfer a freshly formatted string to generated code, preserving allocation failure.
fn checked_owned_str(text: Str) -> Result<*const Str> {
    StrOwner::try_new(text)
        .map(StrOwner::into_raw)
        .map_err(|_| Error::allocation())
}

/// Returns an owned Python spelling for a compiler-validated boolean.
#[unsafe(no_mangle)]
pub extern "C" fn imre_bool_str(value: u32) -> *const Str {
    StrOwner::into_raw(StrOwner::new(Str::new(if value == 0 {
        "False"
    } else {
        "True"
    })))
}

/// Returns an owned string containing Python's None spelling.
#[unsafe(no_mangle)]
pub extern "C" fn imre_none_str() -> *const Str {
    StrOwner::into_raw(StrOwner::new(Str::new("None")))
}

/// # Safety
/// The byte span must remain readable for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_literal(data: *const u8, length: usize) -> *const Str {
    // SAFETY: the compiler supplies the readable constant span.
    let literal = unsafe { text(data, length) };
    StrOwner::into_raw(objects::literal_string(literal))
}

/// # Safety
/// The compiler supplies immutable UTF-8 bytes that remain readable for
/// the program's lifetime. A zero-length span may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_static_literal(data: *const u8, length: usize) -> *const Str {
    // SAFETY: the compiler's string globals have static storage duration.
    let literal: &'static str = unsafe { text(data, length) };
    StrOwner::into_raw(if literal.len() <= 2 {
        objects::literal_string(literal)
    } else {
        StrOwner::new(Str::const_new(literal))
    })
}

/// # Safety
/// The argument must be a live string handle or null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_retain(value: *const Str) {
    if !value.is_null() {
        // SAFETY: the caller owns a live StrOwner reference.
        unsafe { StrOwner::increment_strong_count(value) };
    }
}

/// # Safety
/// Consumes one string reference, or accepts null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_release(value: *const Str) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live StrOwner reference.
        unsafe { StrOwner::decrement_strong_count(value) };
    }
}

/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_print_str(value: *const Str) {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    let text = unsafe { &*value };
    if let Err(error) = sys::write_stdout_with(text, true, || {
        // SAFETY: the caller roots this same string until the write returns.
        Ok(unsafe { StrOwner::acquire_word(value as usize) })
    }) {
        error.raise();
    }
}

/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_len(value: *const Str) -> usize {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    Int::from_usize(unsafe { &*value }.chars().count()).into_raw()
}

/// Tests emptiness without counting Unicode code points.
///
/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_truthy(value: *const Str) -> u32 {
    // SAFETY: the caller roots this immutable string for the duration of the call.
    u32::from(!unsafe { &*value }.is_empty())
}

/// # Safety
/// Both arguments must be live string handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_equal(a: *const Str, b: *const Str) -> u32 {
    // SAFETY: both immutable strings are rooted by the caller.
    u32::from(unsafe { *a == *b })
}

/// # Safety
/// Both words are absent zero or a rooted canonical integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_optional_int_equal(a: usize, b: usize) -> u32 {
    if a == 0 || b == 0 {
        return u32::from(a == b);
    }
    // SAFETY: both nonzero canonical inputs remain rooted through comparison.
    u32::from(unsafe { imre_int_compare(a, b) } == 0)
}

/// # Safety
/// Each argument must be null (Python `None`) or a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_optional_str_equal(a: usize, b: usize) -> u32 {
    if a == 0 || b == 0 {
        u32::from(a == b)
    } else {
        // SAFETY: both handles are non-null, rooted strings.
        unsafe { imre_str_equal(a as *const Str, b as *const Str) }
    }
}

/// Parses Python's base-ten text grammar with Unicode 16.0 decimal digits.
///
/// # Safety
/// The argument must be a live string handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_parse(value: *const Str) -> usize {
    // SAFETY: the caller keeps the immutable string alive.
    let original = unsafe { &*value };
    finish(
        parse_decimal_int(original)
            .map(Int::into_raw)
            .ok_or_else(|| integer_text::invalid_literal(original)),
        1,
    )
}

/// Parses a decimal integer without setting a pending Python exception. Callers
/// such as argparse can report conversion failures in their own error context.
fn parse_decimal_int(original: &str) -> Option<Int> {
    let source = original.trim();
    // Rust's signed decimal parser accepts a subset of our grammar. Ordinary
    // small inputs need neither normalization nor temporary BigInt allocations;
    // separators and larger values still take the checked path below.
    if let Ok(value) = source.parse::<i64>()
        && let Some(value) = Int::small(value)
    {
        return Some(value);
    }
    let source = if source.is_ascii() {
        Cow::Borrowed(source)
    } else {
        let mut normalized = String::with_capacity(source.len());
        for character in source.chars() {
            if character.is_ascii() {
                normalized.push(character);
            } else {
                normalized.push(char::from(b'0' + integer_text::decimal_digit(character)?));
            }
        }
        if let Ok(value) = normalized.parse::<i64>()
            && let Some(value) = Int::small(value)
        {
            return Some(value);
        }
        Cow::Owned(normalized)
    };
    let digits = source.strip_prefix(['+', '-']).unwrap_or(&source);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.bytes().all(|c| c.is_ascii_digit() || c == b'_')
    {
        return None;
    }
    BigInt::parse_bytes(source.replace('_', "").as_bytes(), 10).map(Int::new)
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_input() -> *const Str {
    // A preceding print must be visible before waiting for the next input line.
    if let Err(error) = flush_stdout() {
        return objects::errno_error(error).returning(std::ptr::null());
    }
    finish(
        sys::read_input_line().and_then(|line| {
            StrOwner::try_new(Str::from(line))
                .map(StrOwner::into_raw)
                .map_err(|_| Error::allocation())
        }),
        std::ptr::null(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_list_new() -> *const List {
    new_list(Vec::new())
}

/// Test an optional integer against a homogeneous integer list. Small integers
/// are canonical words; only two distinct heap integers need a value comparison.
///
/// # Safety
/// The caller roots the list and its optional integer (zero represents None).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_list_contains(needle: usize, list: *const List) -> u32 {
    if needle == 0 {
        return 0;
    }
    // SAFETY: neither the list scan nor integer equality can reenter or mutate it.
    unsafe {
        u32::from((&*list).0.with(|values| {
            values.iter().any(|v| {
                v.0 == needle || ((v.0 | needle) & 1 == 0 && imre_int_compare(v.0, needle) == 0)
            })
        }))
    }
}

/// # Safety
/// The argument must be a live list handle or null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_retain(value: *const List) {
    if !value.is_null() {
        // SAFETY: the caller owns a live Rc<List> reference.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Consumes one list reference, or accepts null during slot cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_release(value: *const List) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live Rc<List> reference.
        recycle_list_header(unsafe { Rc::from_raw(value) });
    }
}

/// Converts an index before a storage borrow, without truncating large integers.
#[inline]
fn signed_index(value: &Int) -> Option<isize> {
    if let Some(raw) = value.as_small().and_then(|n| isize::try_from(n).ok()) {
        Some(raw)
    } else {
        big_index(&value.big())
    }
}

fn decode_index(value: &Int) -> Result<isize> {
    signed_index(value).ok_or_else(Error::index)
}

#[inline]
fn normalized_index(raw: isize, length: usize) -> Option<usize> {
    // An underflow wraps above length, so the same bounds check rejects it.
    let at = if raw < 0 {
        length.wrapping_add_signed(raw)
    } else {
        raw as usize
    };
    (at < length).then_some(at)
}

/// Normalizes negative list indices and rejects out-of-bounds accesses.
#[inline]
fn index(raw: isize, length: usize) -> Result<usize> {
    normalized_index(raw, length).ok_or_else(Error::index)
}

/// Returns a bounded fixed-tuple field offset, or sets IndexError before the load.
///
/// # Safety
/// The caller roots the exact integer and supplies the tuple's real length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_tuple_index(word: usize, length: usize) -> usize {
    // SAFETY: the generated call preserves the exact integer for this conversion.
    let value = unsafe { Int::borrow(word) };
    let Some(raw) = signed_index(&value) else {
        return Error::new(
            Kind::IndexError,
            "cannot fit 'int' into an index-sized integer",
        )
        .returning(0);
    };
    normalized_index(raw, length)
        .unwrap_or_else(|| Error::new(Kind::IndexError, "tuple index out of range").returning(0))
}

/// Keeps arbitrary-precision conversion out of the common indexed-access path.
#[cold]
#[inline(never)]
fn big_index(value: &BigInt) -> Option<isize> {
    value.to_isize()
}

/// Empty an integer list in place, detaching its allocation before releasing owners.
///
/// # Safety
/// The caller roots a live integer list for the duration of the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_clear(list: *const List) {
    // SAFETY: the handle stays rooted; the exclusive borrow only moves its buffer.
    let list = unsafe { &*list };
    // SAFETY: no element is destroyed while the storage is borrowed.
    let previous = unsafe {
        list.0
            .with_mut(|values| (!is_sorting_empty_ints(values)).then(|| std::mem::take(values)))
    };
    if let Some(mut values) = previous {
        list.release_range_hint();
        while let Some(value) = values.pop() {
            drop(value);
        }
    }
}

/// Reverse the tagged integer words in place without changing any owner.
///
/// # Safety
/// The caller keeps this list rooted for the duration of the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_reverse(list: *const List) {
    // SAFETY: only swaps within the buffer occur under the exclusive borrow.
    unsafe { (&*list).0.with_mut(|values| values.reverse()) };
}

/// Returns an owned nonzero integer word, or raw zero with a pending error.
///
/// # Safety
/// The arguments must be live list and integer handles, respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_get(list: *const List, at: usize) -> usize {
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
pub unsafe extern "C" fn imre_list_get_index(list: *const List, at: usize) -> usize {
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
/// `imre_list_get_index`; current bounds are still checked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_borrow_index(list: *const List, at: usize) -> usize {
    // SAFETY: the caller roots the list and its element throughout consumption.
    // Only the word escapes this nonreentrant access, never a storage reference.
    let result = unsafe {
        (&*list)
            .0
            .with(|list| list.get(at).map(|value| value.0).ok_or_else(Error::index))
    };
    finish(result, 0)
}

/// Borrows a list element after the compiler has proved its current bounds.
///
/// # Safety
/// `list` must remain rooted, and `at` must be an exact decoded native index
/// strictly below the current length. No storage access may overlap this call.
/// The selected element must remain in place until the returned canonical word
/// is consumed. This does not acquire a reference; the word must not be released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_borrow_index_proven(list: *const List, at: usize) -> usize {
    // SAFETY: the caller proves the index is in bounds and roots its element.
    // Only its borrowed word escapes this nonreentrant storage access.
    unsafe { (&*list).0.with(|values| values.get_unchecked(at).0) }
}

/// Borrows the contiguous, eight-byte-aligned tagged words in an integer list.
///
/// # Safety
/// Keep `list` rooted on its owning thread and exclude mutation through every
/// alias until the last read. The pointer is private to the proven loop and
/// must not cross a callback. Read only indices below the unchanged length.
/// Each word borrows its element. An empty list provides no readable words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_data_readonly(list: *const List) -> *const usize {
    // SAFETY: the whole-loop proof keeps these initialized elements live and
    // in place. Int is transparent over usize, so the pointer and stride agree.
    unsafe { (&*(*list).0.0.get()).as_ptr().cast() }
}

/// Returns one after replacement, or zero with a pending error and no mutation.
///
/// # Safety
/// The arguments must be live list and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_set(list: *const List, at: usize, value: usize) -> u32 {
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
/// exact nonnegative native index described by `imre_list_get_index`. Bounds are
/// checked at this call, including after any intervening mutation through an
/// alias. The replacement is acquired before the displaced owner is released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_set_index(list: *const List, at: usize, value: usize) -> u32 {
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
/// as described by `imre_list_get_index`. The source is checked and acquired
/// before the destination is checked, preserving assignment's evaluation order.
/// The displaced owner is released only after mutable storage access ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_copy_index(
    list: *const List,
    destination: usize,
    source: usize,
) -> u32 {
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

fn try_append_int(list: &mut Vec<Int>, value: &Int) -> bool {
    if list.try_reserve(1).is_err() {
        objects::exceptions::sequence_allocation_failed();
        return false;
    }
    list.push(value.clone());
    true
}

/// # Safety
/// The arguments must be live list and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_append(list: *const List, value: usize) -> i32 {
    // SAFETY: the caller roots both handles; allocation and acquisition cannot
    // reenter the runtime, and no storage reference escapes the closure.
    unsafe {
        let value = Int::borrow(value);
        (&*list)
            .0
            .with_mut(|list| i32::from(try_append_int(list, &value)))
    }
}

/// Appends two already-evaluated integers with one capacity check. The compiler
/// selects this only when the first append cannot affect the second operand.
///
/// # Safety
/// Root the exact list and both canonical integer handles throughout the call.
/// Each stored element acquires its own owner, including repeated BigInts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_append_pair(
    list: *const List,
    first: usize,
    second: usize,
) -> i32 {
    // SAFETY: these borrowed words remain rooted even when resizing moves the
    // list's buffer. Acquisition and allocation cannot call user code.
    unsafe {
        let first = Int::borrow(first);
        let second = Int::borrow(second);
        (&*list).0.with_mut(|list| {
            if list.capacity() - list.len() < 2 {
                // The first source append can fit even if growing for the second fails.
                return i32::from(try_append_int(list, &first) && try_append_int(list, &second));
            }
            let length = list.len();
            // The capacity check proves that both slots fit. Cloning exact
            // Int owners cannot unwind; publish only after both are initialized.
            let output = list.as_mut_ptr().add(length);
            output.write((*first).clone());
            output.add(1).write((*second).clone());
            list.set_len(length + 2);
            1
        })
    }
}

/// Returns an owned nonzero integer word, or raw zero with a pending error.
///
/// # Safety
/// The caller roots the exact integer returned by a Python length slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_length_result(value: usize) -> usize {
    // SAFETY: this is the nonzero tagged result of the user's __len__ method.
    let n = unsafe { Int::borrow(value) };
    let negative = n
        .as_small()
        .map_or_else(|| n.big().sign() == num_bigint::Sign::Minus, |i| i < 0);
    if negative {
        return Error::new(Kind::ValueError, "__len__() should return >= 0").returning(0);
    }
    if decode_index(&n).is_err() {
        return Error::new(
            Kind::OverflowError,
            "cannot fit 'int' into an index-sized integer",
        )
        .returning(0);
    }
    (*n).clone().into_raw()
}

/// # Safety
/// The argument must be a live list handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_pop(list: *const List) -> usize {
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
pub unsafe extern "C" fn imre_list_len(list: *const List) -> usize {
    // SAFETY: the rooted list is accessed only for a nonreentrant length read.
    let length = unsafe { (&*list).0.with(Vec::len) };
    debug_assert!(length <= MAX_LIST_LEN);
    // Vec<Int>'s allocation limit guarantees that this is a small integer.
    (length << 1) | 1
}

/// Sums an exact integer list and an already-evaluated starting value.
///
/// An i32 reduction avoids per-element integer owners and arithmetic calls.
/// Other representations retain arbitrary-precision addition. Addition cannot
/// invoke user code, so adding the start after the list subtotal is equivalent
/// and leaves pending exception state unchanged.
///
/// # Safety
/// The caller roots the list and canonical integer start throughout this call.
/// This nonreentrant call never mutates the list or exposes its storage.
/// The result transfers one independently owned canonical integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_sum(list: *const List, start: usize) -> usize {
    // SAFETY: the rooted list is read without callbacks or escaped borrows.
    let subtotal = unsafe { (&*list).0.with(|values| sum_ints(values.iter())) };
    // SAFETY: start remains rooted, and subtotal owns the other integer.
    unsafe { imre_int_binary(start, subtotal.0, 0) }
}

/// Shares exact reduction between whole lists and normalized slice selections.
/// Cloning the borrowed iterator permits a checked fast pass without acquiring
/// element owners; neither pass can mutate the source or call user code.
fn sum_ints<'a>(values: impl Iterator<Item = &'a Int> + Clone) -> Int {
    let (length, upper) = values.size_hint();
    // Callers use standard slice iterators and adapters. Equal bounds retain
    // the exact-count proof for a chain even though Chain cannot implement
    // ExactSizeIterator generally: its component lengths might overflow usize.
    if upper == Some(length) && length <= i32::MAX as usize {
        let (sum, invalid) = values
            .clone()
            .fold((0_i64, 0_usize), |(sum, invalid), value| {
                // Biasing the tagged i32 interval into 33 bits also rejects heap
                // words by their even tag. Decoding reads bits, never a pointer.
                (
                    sum.wrapping_add((value.0 as i64) >> 1),
                    invalid | (value.0.wrapping_add(1_usize << 32) >> 33) | (!value.0 & 1),
                )
            });
        if invalid == 0 {
            // At most 2^31-1 signed i32 values sum within
            // [-2^62+2^31, 2^62-2^32+1], including every prefix.
            return Int::small(sum).unwrap();
        }
    }
    values.fold(Int::small(0).unwrap(), |sum, value| {
        // SAFETY: both owned/borrowed canonical integers remain live for addition.
        Int(unsafe { imre_int_binary(sum.0, value.0, 0) })
    })
}

/// Counts a nonempty range only when its captured components are tagged small.
///
/// # Safety
/// All arguments must be rooted canonical integer handles.
unsafe fn small_range_trips(start: usize, stop: usize, step: usize) -> Option<usize> {
    // SAFETY: the components remain rooted during these nonreentrant reads.
    let (start, stop, step) = unsafe {
        (
            Int::borrow(start).as_small()?,
            Int::borrow(stop).as_small()?,
            Int::borrow(step).as_small()?,
        )
    };
    // Differences of tagged-small values and a negated small step fit i64.
    if step > 0 && stop > start {
        Some(((stop - start - 1) / step + 1) as usize)
    } else if step < 0 && start > stop {
        Some(((start - stop - 1) / -step + 1) as usize)
    } else {
        None
    }
}

/// Gives one empty output list a best-effort capacity hint, capped at 256 KiB.
/// A break, error, or untaken branch may consume fewer slots than estimated.
/// Allocation failure is ignored; the ordinary append retains its error behavior.
///
/// # Safety
/// `list` is a rooted list or null; the range arguments are rooted canonical
/// integers. `per_iteration` is a decoded native count, not a tagged integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_reserve_range(
    list: *const List,
    start: usize,
    stop: usize,
    step: usize,
    per_iteration: usize,
) {
    if list.is_null() {
        return;
    }
    // SAFETY: all range components remain rooted throughout the estimate.
    let Some(trips) = (unsafe { small_range_trips(start, stop, step) }) else {
        return;
    };
    let capacity = trips
        .saturating_mul(per_iteration)
        .min(MAX_REUSED_LIST_CAPACITY);
    // SAFETY: the caller roots the list for this nonreentrant reservation.
    let list = unsafe { &*list };
    let displaced = RANGE_HINT_SLOTS.try_with(|used| {
        let other_slots = used.get() - list.1.get();
        let available = MAX_LIVE_RANGE_HINT_SLOTS - other_slots;
        if capacity > available {
            return Vec::new();
        }
        // SAFETY: no buffer address or storage borrow escapes, and the
        // reservation leaves existing elements untouched.
        unsafe {
            list.0.with_mut(|values| {
                if !values.is_empty()
                    || values.capacity() >= capacity
                    || is_sorting_empty_ints(values)
                {
                    return Vec::new();
                }
                let mut buffer = take_list_buffer(capacity);
                if buffer.capacity() < capacity && buffer.try_reserve_exact(capacity).is_err() {
                    return buffer;
                }
                let slots = buffer.capacity();
                if slots > available {
                    return buffer;
                }
                list.1.set(slots);
                used.set(other_slots + slots);
                std::mem::replace(values, buffer)
            })
        }
    });
    drop(displaced);
}

/// Guards native arithmetic in a loop that cannot replace this list's elements.
/// The caller keeps the list rooted and excludes mutation until the guarded
/// reads finish; arbitrary-precision values select the original checked loop.
/// Scanning is worthwhile only when the list has at most four elements per
/// planned iteration. Empty, zero-step, and arbitrary-precision ranges decline
/// specialization without reporting an error or scanning any list elements.
///
/// # Safety
/// `list` must be a rooted list handle or a null uninitialized local, accessed
/// on its owning thread. Null selects the original loop without dereferencing it.
/// The remaining arguments are rooted canonical integer range components.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_all_i32(
    list: *const List,
    start: usize,
    stop: usize,
    step: usize,
) -> u32 {
    if list.is_null() {
        return 0;
    }
    // SAFETY: all range components remain rooted throughout the estimate.
    let Some(trips) = (unsafe { small_range_trips(start, stop, step) }) else {
        return 0;
    };
    // SAFETY: this read cannot reenter or retain a storage reference.
    let valid = unsafe {
        (&*list).0.with(|values| {
            if values.len().div_ceil(4) > trips {
                return false;
            }
            // Biasing moves the tagged i32 interval into the low 33 bits.
            // Its one extra lower endpoint has an even tag and is rejected by
            // the second term. An integer OR reduction exposes SIMD lowering.
            values.iter().fold(0_usize, |invalid, value| {
                invalid | (value.0.wrapping_add(1_usize << 32) >> 33) | (!value.0 & 1)
            }) == 0
        })
    };
    u32::from(valid)
}

/// # Safety
/// The arguments must be live list and integer handles. Returns a new list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_list_repeat(list: *const List, count: usize) -> *const List {
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
        return imre_list_new();
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
                .map_err(|_| Error::allocation())?;
            if !list.is_empty() {
                for _ in 0..count {
                    result.extend(list.iter().cloned());
                }
            }
            Ok(result)
        })
    };
    finish(result.map(new_list), std::ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_assert_fail() {
    Error::new(Kind::AssertionError, "").raise();
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_range_step_zero() {
    Error::new(Kind::ValueError, "range() arg 3 must not be zero").raise();
}

#[cfg(test)]
mod tests {
    use super::*;
    use objects::exceptions::{
        imre_error_pending, imre_error_take, imre_exception_matches, imre_exception_message,
    };

    #[test]
    fn output_works_with_and_without_a_reserved_buffer() {
        for capacity in [0, 8] {
            let mut output = BinaryOutput::with_buffer(Vec::with_capacity(capacity), Vec::new());
            output.write_all(b"abc").unwrap();
            assert_eq!(output.inner.is_empty(), capacity != 0);
            output.write_all(b"0123456789").unwrap();
            output.write_all(b"z").unwrap();
            output.flush().unwrap();
            assert_eq!(output.inner, b"abc0123456789z");
        }
    }

    #[test]
    fn nested_capture_leaves_pending_standard_output_untouched() {
        let mut output = Output::with_stream(Vec::new(), 16, false);
        output.write_all(b"before ").unwrap();
        output.captures.push(Vec::new());
        output.write_all(b"outer ").unwrap();
        output.captures.push(Vec::new());
        assert_eq!(output.write(b"inner").unwrap(), 5);
        output.flush().unwrap();
        assert!(output.raw.inner.is_empty());
        assert_eq!(output.captures.pop().unwrap(), b"inner");
        output.write_all(b"example").unwrap();
        assert_eq!(output.captures.pop().unwrap(), b"outer example");
        output.write_all(b"after").unwrap();
        output.flush().unwrap();
        assert_eq!(output.raw.inner, b"before after");
    }

    #[test]
    fn output_retries_only_the_unwritten_suffix() {
        #[derive(Default)]
        struct Sink {
            bytes: Vec<u8>,
            failed: bool,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.bytes.is_empty() {
                    return self.bytes.write(&bytes[..bytes.len().min(2)]);
                }
                if !self.failed {
                    self.failed = true;
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                self.bytes.write(bytes)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut output = BinaryOutput::with_buffer(Vec::with_capacity(16), Sink::default());
        output.write_all(b"partial").unwrap();
        assert_eq!(
            output.flush().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        output.flush().unwrap();
        assert_eq!(output.inner.bytes, b"partial");
    }

    #[test]
    fn nonblocking_output_keeps_the_suffix_that_fits() {
        struct Pipe<'a>(&'a mut [u8]);

        impl Write for Pipe<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0.is_empty() && !bytes.is_empty() {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                self.0.write(bytes)
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        for (prefix, next, fits) in [
            ("", "abcdef", true),
            ("abc", "def", true),
            ("", "abcdefg", false),
            ("abc", "defg", false),
        ] {
            let mut pipe = [0; 2];
            let mut restored = [0; 4];
            let mut output = BinaryOutput::with_buffer(Vec::with_capacity(4), Pipe(&mut pipe));
            output.write_all(prefix.as_bytes()).unwrap();
            let result = output.write_all(next.as_bytes()).map_err(|e| e.kind());
            assert_eq!(
                result,
                if fits {
                    Ok(())
                } else {
                    Err(io::ErrorKind::WouldBlock)
                }
            );
            output.inner = Pipe(&mut restored);
            output.flush().unwrap();
            assert_eq!(&pipe, b"ab");
            assert_eq!(&restored, b"cdef");
        }
    }

    #[test]
    fn paired_append_preserves_order_and_independent_bigint_owners() {
        let big = Rc::new(BigInt::from(1_u128 << 100));
        let weak = Rc::downgrade(&big);
        let big_word = Rc::as_ptr(&big) as usize;
        let negative = Int::small(-7).unwrap().0;
        let values = vec![Int::small(9).unwrap()];
        let list = Rc::new(List::new(values));
        // SAFETY: both inputs and the exact list remain rooted. The second call
        // borrows its repeated input from the list itself across buffer growth.
        unsafe {
            assert_eq!(
                imre_list_append_pair(Rc::as_ptr(&list), big_word, negative),
                1
            );
            assert_eq!(Rc::strong_count(&big), 2);
            let borrowed = list.0.with(|values| values[1].0);
            assert_eq!(
                imre_list_append_pair(Rc::as_ptr(&list), borrowed, borrowed),
                1
            );
            assert_eq!(Rc::strong_count(&big), 4);
            list.0.with(|values| {
                assert_eq!(values.len(), 5);
                assert_eq!(values[0].as_small(), Some(9));
                assert_eq!(values[1].0, big_word);
                assert_eq!(values[2].as_small(), Some(-7));
                assert_eq!(values[3].0, big_word);
                assert_eq!(values[4].0, big_word);
            });
        }
        drop(big);
        assert_eq!(weak.strong_count(), 3);
        drop(list);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn list_sum_reduces_signed_i32_values_across_vector_boundaries() {
        let pattern = [i32::MIN, i32::MAX, -1, 0, 1];
        for length in [0, 1, 2, 3, 4, 7, 8, 15, 16, 17, 31, 32, 33, 257] {
            let expected = (0..length)
                .map(|index| BigInt::from(pattern[index % pattern.len()]))
                .sum::<BigInt>();
            let list = List::new(
                (0..length)
                    .map(|index| Int::small(i64::from(pattern[index % pattern.len()])).unwrap())
                    .collect(),
            );
            for start in [0, -19, MIN_SMALL, MAX_SMALL] {
                let start = Int::small(start).unwrap();
                // SAFETY: both exact arguments remain rooted for the sum.
                let result = Int(unsafe { imre_list_sum(&list, start.0) });
                assert_eq!(result.big().as_ref(), &(&expected + start.big().as_ref()));
            }
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn list_sum_preserves_large_values_cancellation_and_canonical_results() {
        let huge = BigInt::from(1) << 140_u32;
        for (values, start) in [
            (vec![], huge.clone()),
            (vec![-huge.clone(), 17.into()], huge.clone()),
            (vec![huge.clone(), 17.into()], -huge.clone()),
            (vec![huge.clone(), -huge.clone(), 5.into()], 9.into()),
            (
                vec![BigInt::from(i32::MIN) - 1, BigInt::from(i32::MAX) + 1],
                0.into(),
            ),
            (
                vec![MAX_SMALL.into(), MAX_SMALL.into(), MIN_SMALL.into()],
                0.into(),
            ),
        ] {
            let expected = values.iter().sum::<BigInt>() + &start;
            let list = List::new(values.into_iter().map(Int::new).collect());
            let start = Int::new(start);
            // SAFETY: both exact arguments remain rooted for the sum.
            let result = Int(unsafe { imre_list_sum(&list, start.0) });
            assert_eq!(result.big().as_ref(), &expected);
            assert_eq!(
                result.as_small(),
                expected
                    .to_i64()
                    .filter(|value| (MIN_SMALL..=MAX_SMALL).contains(value))
            );
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn list_sum_borrows_aliased_inputs_and_owns_results_after_mutation() {
        let positive = Rc::new(BigInt::from(1) << 120_u32);
        let negative = Rc::new(-positive.as_ref());
        let positive_weak = Rc::downgrade(&positive);
        let negative_weak = Rc::downgrade(&negative);
        let list = Rc::new(List::new(vec![
            Int(Rc::into_raw(positive.clone()) as usize),
            Int(Rc::into_raw(negative.clone()) as usize),
            Int::small(7).unwrap(),
        ]));
        let alias = list.clone();
        let start = Rc::as_ptr(&positive) as usize;
        // SAFETY: start is rooted both directly and by the list. The aliased
        // mutation occurs between reductions, after all storage borrows end.
        let result = unsafe {
            let result = Int(imre_list_sum(Rc::as_ptr(&list), start));
            assert_eq!(result.big().as_ref(), &(positive.as_ref() + 7));
            assert_eq!(Rc::strong_count(&positive), 2);
            assert_eq!(Rc::strong_count(&negative), 2);
            assert_eq!(
                imre_list_set_index(Rc::as_ptr(&alias), 0, Int::small(42).unwrap().0),
                1
            );
            let changed = Int(imre_list_sum(Rc::as_ptr(&list), start));
            assert_eq!(changed.as_small(), Some(49));
            result
        };
        drop(alias);
        drop(list);
        assert_eq!(Rc::strong_count(&positive), 1);
        assert_eq!(Rc::strong_count(&negative), 1);
        let expected = positive.as_ref() + 7;
        drop(positive);
        drop(negative);
        assert!(positive_weak.upgrade().is_none());
        assert!(negative_weak.upgrade().is_none());
        assert_eq!(result.big().as_ref(), &expected);
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn list_header_reuse_preserves_strong_and_weak_aliases() {
        std::thread::spawn(|| {
            // SAFETY: every raw owner is consumed exactly once; the extra
            // retained owner keeps the original list live across the first release.
            unsafe {
                let original = imre_list_new();
                imre_list_retain(original);
                imre_list_release(original);
                assert!(LIST_HEADERS.with(|headers| headers.borrow().iter().all(Option::is_none)));
                imre_list_append(original, Int::small(7).unwrap().0);
                let distinct = imre_list_new();
                assert_ne!(distinct, original);
                imre_list_release(distinct);
                imre_list_release(original);
                let reused = imre_list_new();
                assert_eq!(reused, original);
                assert_eq!(Int(imre_list_len(reused)).as_small(), Some(0));
                let owner = Rc::from_raw(reused);
                let weak = Rc::downgrade(&owner);
                imre_list_release(Rc::into_raw(owner));
                assert!(weak.upgrade().is_none());
                let next = imre_list_new();
                assert_ne!(next, weak.as_ptr());
                imre_list_release(next);
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn list_header_reuse_releases_elements_and_keeps_buffers_separate() {
        std::thread::spawn(|| {
            let big = Rc::new(BigInt::from(1_u8) << 200_u32);
            let weak = Rc::downgrade(&big);
            let integer = Int(Rc::into_raw(big) as usize);
            let list = new_list(vec![integer.clone(), integer]);
            // SAFETY: the sequence retains the list, then releases its last
            // owner through WordKind. Reuse must retain neither BigInt owner.
            unsafe {
                let parent = objects::imre_seq_new(4);
                objects::imre_seq_append(parent, list as usize, 4);
                imre_list_release(list);
                assert_eq!(weak.strong_count(), 2);
                objects::imre_object_release(parent);
                assert!(weak.upgrade().is_none());
                let reused = imre_list_new();
                assert_eq!(reused, list);
                imre_list_release(reused);
            }
            let mut values = Vec::with_capacity(16);
            values.push(Int::small(9).unwrap());
            let buffer = values.as_ptr();
            // SAFETY: the constructor transfers its sole owner to release.
            unsafe { imre_list_release(new_list(values)) };
            LIST_HEADERS.with(|headers| {
                for header in headers.borrow().iter().flatten() {
                    // SAFETY: cached headers cannot be accessed by the language
                    // and this read does not mutate or reenter their storage.
                    unsafe {
                        header.0.with(|values| {
                            assert!(values.is_empty());
                            assert_eq!(values.capacity(), 0);
                        });
                    }
                }
            });
            let reused = take_list_buffer(16);
            assert_eq!(reused.as_ptr(), buffer);
            assert!(reused.is_empty());
            assert_eq!(imre_error_pending(), 0);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn list_header_cache_is_bounded_and_declines_reentry() {
        std::thread::spawn(|| {
            let lists = [imre_list_new(), imre_list_new(), imre_list_new()];
            // SAFETY: these distinct, unaliased owners are consumed once.
            unsafe {
                for list in lists {
                    imre_list_release(list);
                }
            }
            LIST_HEADERS.with(|headers| {
                let borrowed = headers.borrow_mut();
                let pointers = || {
                    borrowed
                        .each_ref()
                        .map(|owner| owner.as_ref().map(Rc::as_ptr))
                };
                assert_eq!(pointers(), [Some(lists[2]), Some(lists[1])]);
                let fresh = imre_list_new();
                assert!(!pointers().contains(&Some(fresh)));
                // SAFETY: failed cache borrowing falls back to consuming this
                // new owner normally, without disturbing the borrowed slots.
                unsafe { imre_list_release(fresh) };
                assert_eq!(pointers(), [Some(lists[2]), Some(lists[1])]);
            });
        })
        .join()
        .unwrap();
    }

    #[test]
    fn list_header_cache_allows_release_after_thread_local_teardown() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct Teardown(Arc<AtomicBool>);
        impl Drop for Teardown {
            fn drop(&mut self) {
                let big = Rc::new(BigInt::from(1_u8) << 200_u32);
                let weak = Rc::downgrade(&big);
                let list = new_list(vec![Int(Rc::into_raw(big) as usize)]);
                // SAFETY: this sole owner must be released even after cache TLS
                // destruction, including the contained BigInt allocation.
                unsafe { imre_list_release(list) };
                assert!(weak.upgrade().is_none());
                self.0.store(true, Ordering::Relaxed);
            }
        }
        thread_local! {
            static TEARDOWN: RefCell<Option<Teardown>> = const { RefCell::new(None) };
        }
        let completed = Arc::new(AtomicBool::new(false));
        let observed = completed.clone();
        std::thread::spawn(move || {
            // Reverse TLS destruction drops the cache before this probe.
            TEARDOWN.with(|probe| *probe.borrow_mut() = Some(Teardown(observed)));
            // SAFETY: this cache warm-up transfers one newly constructed owner.
            unsafe { imre_list_release(imre_list_new()) };
        })
        .join()
        .unwrap();
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn list_buffer_reuse_preserves_live_lists_and_uses_a_tight_fit() {
        let mut values = Vec::with_capacity(64);
        values.extend((0..64).map(|value| Int::small(value).unwrap()));
        let allocation = values.as_ptr();
        let list = Rc::new(List::new(values));
        let alias = list.clone();
        drop(list);
        assert_eq!(take_list_buffer(48).capacity(), 0);
        drop(alias);
        let mut reused = take_list_buffer(48);
        assert_eq!(reused.as_ptr(), allocation);
        assert_eq!(reused.capacity(), 64);
        assert!(reused.is_empty());
        reused.push(Int::small(-7).unwrap());
        assert_eq!(reused[0].as_small(), Some(-7));
    }

    #[test]
    fn list_buffer_pool_is_bounded_and_declines_reentrant_access() {
        fn cache(capacity: usize) {
            let mut values = Vec::with_capacity(capacity);
            recycle_list_buffer(&mut values);
        }
        fn capacities() -> [usize; 2] {
            LIST_BUFFERS.with(|buffers| buffers.borrow().each_ref().map(Vec::capacity))
        }
        cache(16);
        cache(8);
        let mut smallest = take_list_buffer(8);
        assert_eq!(smallest.capacity(), 8);
        recycle_list_buffer(&mut smallest);
        assert_eq!(take_list_buffer(9).capacity(), 16);
        assert_eq!(take_list_buffer(3).capacity(), 0);
        assert_eq!(take_list_buffer(0).capacity(), 0);
        assert_eq!(take_list_buffer(MAX_REUSED_LIST_CAPACITY + 1).capacity(), 0);
        cache(32);
        cache(64);
        assert_eq!(capacities(), [64, 32]);
        cache(0);
        cache(MAX_REUSED_LIST_CAPACITY + 1);
        assert_eq!(capacities(), [64, 32]);
        cache(MAX_REUSED_LIST_CAPACITY);
        cache(MAX_REUSED_LIST_CAPACITY);
        assert_eq!(capacities(), [MAX_REUSED_LIST_CAPACITY; 2]);
        LIST_BUFFERS.with(|buffers| {
            let borrowed = buffers.borrow_mut();
            assert_eq!(take_list_buffer(MAX_REUSED_LIST_CAPACITY).capacity(), 0);
            let mut values = Vec::with_capacity(8);
            recycle_list_buffer(&mut values);
            assert_eq!(values.capacity(), 0);
            assert_eq!(
                borrowed.each_ref().map(Vec::capacity),
                [MAX_REUSED_LIST_CAPACITY; 2]
            );
        });
    }

    #[test]
    fn list_buffer_pool_releases_bigints_and_survives_thread_teardown() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let bigint = Rc::new(BigInt::from(1_u128 << 100));
        let weak = Rc::downgrade(&bigint);
        let mut values = Vec::with_capacity(8);
        values.push(Int(Rc::into_raw(bigint) as usize));
        recycle_list_buffer(&mut values);
        assert_eq!(values.len(), 1);
        assert_eq!(weak.strong_count(), 1);
        drop(List::new(values));
        assert!(weak.upgrade().is_none());
        LIST_BUFFERS.with(|buffers| {
            assert!(buffers.borrow().iter().all(|buffer| buffer.capacity() == 0));
        });

        struct Teardown(Arc<AtomicBool>);
        impl Drop for Teardown {
            fn drop(&mut self) {
                let mut values = take_list_buffer(8);
                values.try_reserve_exact(8).unwrap();
                values.push(Int::small(1).unwrap());
                drop(List::new(values));
                self.0.store(true, Ordering::Relaxed);
            }
        }
        thread_local! {
            static TEARDOWN: RefCell<Option<Teardown>> = const { RefCell::new(None) };
        }
        let completed = Arc::new(AtomicBool::new(false));
        let observed = completed.clone();
        std::thread::spawn(move || {
            // Initialize the probe first so reverse TLS teardown exercises list
            // cleanup after the buffer cache has already been destroyed.
            TEARDOWN.with(|probe| *probe.borrow_mut() = Some(Teardown(observed)));
            let mut values = Vec::with_capacity(8);
            values.push(Int::small(2).unwrap());
            drop(List::new(values));
        })
        .join()
        .unwrap();
        assert!(completed.load(Ordering::Relaxed));
    }

    #[test]
    fn list_drop_preserves_bigint_owners_at_vector_boundaries() {
        for position in [0, 1, 7, 8, 15, 16, 32] {
            let bigint = Rc::new(BigInt::from(1_u128 << 100));
            let weak = Rc::downgrade(&bigint);
            let mut values = (0..33)
                .map(|value| Int::small(value - 16).unwrap())
                .collect::<Vec<_>>();
            values[position] = Int(Rc::into_raw(bigint) as usize);
            let list = Rc::new(List::new(values));
            let alias = Rc::clone(&list);
            drop(list);
            assert_eq!(weak.strong_count(), 1);
            drop(alias);
            assert!(weak.upgrade().is_none());
        }
        for length in [0, 1, 7, 8, 15, 16, 33] {
            let values = [MIN_SMALL, 0, MAX_SMALL]
                .into_iter()
                .cycle()
                .take(length)
                .map(|value| Int::small(value).unwrap())
                .collect();
            drop(List::new(values));
        }
    }

    #[test]
    fn scalar_strings_preserve_integer_boundaries_and_python_spellings() {
        for value in [
            BigInt::from(MIN_SMALL),
            BigInt::from(MAX_SMALL),
            BigInt::from(0),
            BigInt::from(1_u128 << 100),
            -BigInt::from(1_u128 << 100),
        ] {
            let expected = value.to_string();
            let value = Int::new(value);
            // SAFETY: the integer stays rooted and the returned string transfers ownership.
            let formatted = unsafe { StrOwner::from_raw(imre_int_str(value.0)) };
            drop(value);
            assert_eq!(formatted.as_str(), expected);
        }
        // SAFETY: each conversion returns one fresh owned string handle.
        unsafe {
            assert_eq!(StrOwner::from_raw(imre_bool_str(0)).as_str(), "False");
            assert_eq!(StrOwner::from_raw(imre_bool_str(1)).as_str(), "True");
            assert_eq!(StrOwner::from_raw(imre_none_str()).as_str(), "None");
        }
    }

    fn clear_index_error() {
        assert_eq!(imre_error_pending(), 1);
        let error = imre_error_take();
        assert!(!error.is_null());
        // SAFETY: take transfers one owned exception; inspect and release it once.
        unsafe {
            assert_eq!(imre_exception_matches(error, Kind::IndexError as u32), 1);
            objects::imre_object_release(error);
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn decimal_input_preserves_signs_separators_and_tag_boundaries() {
        for (source, expected, small) in [
            ("\u{2003}+00017\t", "17", true),
            ("\u{2003}-١_２_𝟛\u{85}", "-123", true),
            (
                "+١٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠٠",
                "1000000000000000000000000000000",
                false,
            ),
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
            let parsed = Int(unsafe { imre_int_parse(&source) });
            assert_eq!(parsed.big().to_string(), expected);
            assert_eq!(parsed.as_small().is_some(), small);
        }
    }

    #[test]
    fn decimal_input_accepts_decimal_digit_blocks_without_numeric_lookalikes() {
        for start in [0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0xff10, 0x1d7ce, 0x1d7d8] {
            for digit in 0..10 {
                let source = Str::from(char::from_u32(start + digit).unwrap().to_string());
                // SAFETY: source stays rooted; the returned integer is owned.
                let parsed = Int(unsafe { imre_int_parse(&source) });
                assert_eq!(imre_error_pending(), 0);
                assert_eq!(parsed.as_small(), Some(i64::from(digit)));
            }
        }
    }

    #[test]
    fn decimal_input_errors_preserve_python_repr_and_original_whitespace() {
        for (source, literal) in [
            ("", "''"),
            ("  +1x\t", "'  +1x\\t'"),
            ("12__3", "'12__3'"),
            ("+_1", "'+_1'"),
            ("1_", "'1_'"),
            ("²", "'²'"),
            ("½", "'½'"),
            ("四", "'四'"),
            ("\u{301}", "'\u{301}'"),
            ("\u{e0100}", "'\u{e0100}'"),
            ("\u{88f}", "'\\u088f'"),
            ("\u{11de0}", "'\\U00011de0'"),
            ("\u{33479}", "'\\U00033479'"),
            ("🐍", "'🐍'"),
            ("\0", "'\\x00'"),
            ("\t", "'\\t'"),
            ("\n", "'\\n'"),
            ("\u{1c}1", "'\\x1c1'"),
            ("'", "\"'\""),
            ("\"", "'\"'"),
        ] {
            let source = Str::new(source);
            // SAFETY: source is rooted; take/message transfer owned handles.
            unsafe {
                assert_eq!(imre_int_parse(&source), 1);
                let error = imre_error_take();
                assert!(!error.is_null());
                assert_eq!(imre_exception_matches(error, Kind::ValueError as u32), 1);
                let message = StrOwner::from_raw(imre_exception_message(error));
                assert_eq!(
                    message.as_str(),
                    format!("invalid literal for int() with base 10: {literal}")
                );
                objects::imre_object_release(error);
                assert_eq!(imre_error_pending(), 0);
            }
        }
        let source = Str::from("é".repeat(220));
        // SAFETY: source is rooted; take/message transfer owned handles.
        unsafe {
            assert_eq!(imre_int_parse(&source), 1);
            let error = imre_error_take();
            assert!(!error.is_null());
            let message = StrOwner::from_raw(imre_exception_message(error));
            assert_eq!(
                message.as_str(),
                format!(
                    "invalid literal for int() with base 10: '{}",
                    "é".repeat(199)
                )
            );
            objects::imre_object_release(error);
        }
    }

    #[test]
    fn integers_promote_and_demote_without_losing_values() {
        let a = Int::new(BigInt::from(MAX_SMALL));
        let b = Int::new(BigInt::from(1));
        // SAFETY: test-owned handles satisfy the ABI borrowing contract.
        let sum = Int(unsafe { imre_int_binary(a.0, b.0, 0) });
        assert_eq!(sum.big().as_ref(), &BigInt::from(1_u64 << 62));
        assert!(sum.as_small().is_none());
        // SAFETY: test-owned handles satisfy the ABI borrowing contract.
        let original = Int(unsafe { imre_int_binary(sum.0, b.0, 1) });
        assert_eq!(original.as_small(), Some(MAX_SMALL));
    }

    #[test]
    fn floor_division_and_remainder_match_python_signs() {
        for (a, b, q, r) in [(-7, 3, -3, 2), (7, -3, -3, -2), (-7, -3, 2, -1)] {
            let (a, b) = (Int::new(a.into()), Int::new(b.into()));
            // SAFETY: all arguments are live, owned integers.
            let quotient = Int(unsafe { imre_int_binary(a.0, b.0, 3) });
            // SAFETY: all arguments are live, owned integers.
            let remainder = Int(unsafe { imre_int_binary(a.0, b.0, 4) });
            assert_eq!(quotient.as_small(), Some(q));
            assert_eq!(remainder.as_small(), Some(r));
        }
    }

    #[test]
    fn zero_integer_divisors_report_the_python_message() {
        let zero = Int::small(0).unwrap();
        for numerator in [Int::small(7).unwrap(), Int::new(BigInt::from(1) << 130_u32)] {
            for op in [3, 4] {
                // SAFETY: both integer handles remain live. Take the owned error and message.
                unsafe {
                    assert_eq!(imre_int_binary(numerator.0, zero.0, op), 1);
                    let error = imre_error_take();
                    assert_eq!(
                        imre_exception_matches(error, Kind::ZeroDivisionError as u32),
                        1
                    );
                    let message = StrOwner::from_raw(imre_exception_message(error));
                    assert_eq!(message.as_str(), "division by zero");
                    objects::imre_object_release(error);
                }
            }
        }
    }

    #[test]
    fn shifts_validate_counts_and_preserve_big_integer_owners() {
        for (value, count, operation, kind) in [
            (BigInt::from(0), BigInt::from(-1), 8, Kind::ValueError),
            (
                BigInt::from(0),
                -(BigInt::from(1) << 130_u32),
                9,
                Kind::ValueError,
            ),
            (
                BigInt::from(1),
                BigInt::from(1) << 130_u32,
                8,
                Kind::OverflowError,
            ),
            (
                BigInt::from(-1),
                BigInt::from(i64::MAX),
                8,
                Kind::OverflowError,
            ),
        ] {
            let value = Int::new(value);
            let count = Int::new(count);
            // SAFETY: the operands remain rooted; a failed integer call returns
            // tagged zero and the pending exception transfers one owned handle.
            unsafe {
                assert_eq!(imre_int_binary(value.0, count.0, operation), 1);
                assert_eq!(imre_error_pending(), 1);
                let error = imre_error_take();
                assert_eq!(imre_exception_matches(error, kind as u32), 1);
                objects::imre_object_release(error);
            }
        }
        let owner = Rc::new(BigInt::from(1) << 130_u32);
        let value = Int(Rc::into_raw(owner.clone()) as usize);
        let zero = Int::small(0).unwrap();
        // SAFETY: zero shifting returns an independent owner of the live input.
        let copy = Int(unsafe { imre_int_binary(value.0, zero.0, 8) });
        assert_eq!(Rc::strong_count(&owner), 3);
        drop(value);
        assert_eq!(copy.big().as_ref(), owner.as_ref());
        drop(copy);
        assert_eq!(Rc::strong_count(&owner), 1);
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn list_i32_guard_checks_every_value_and_current_mutations() {
        let list = imre_list_new();
        // SAFETY: all values remain rooted during insertion, and the sole list
        // owner is released after the final check. Guards acquire no owners.
        unsafe {
            assert_eq!(imre_list_all_i32(std::ptr::null(), 1, 2049, 3), 0);
            assert_eq!(imre_list_all_i32(list, 1, 2049, 3), 1);
            for value in [i32::MIN as i64, -1, 0, i32::MAX as i64] {
                let value = Int::small(value).unwrap();
                imre_list_append(list, value.0);
            }
            assert_eq!(imre_list_all_i32(list, 1, 2049, 3), 1);
            for value in [
                BigInt::from(i32::MIN) - 1,
                BigInt::from(i32::MAX) + 1,
                BigInt::from(1) << 100_u32,
                -(BigInt::from(1) << 100_u32),
            ] {
                let value = Int::new(value);
                imre_list_append(list, value.0);
                assert_eq!(imre_list_all_i32(list, 1, 2049, 3), 0);
                drop(Int(imre_list_pop(list)));
                assert_eq!(imre_list_all_i32(list, 1, 2049, 3), 1);
            }
            imre_list_release(list);
        }
    }

    #[test]
    fn list_i32_guard_bounds_scanning_without_raising_range_errors() {
        let list = imre_list_new();
        let zero = Int::small(0).unwrap();
        let one = Int::small(1).unwrap();
        let nine = Int::small(9).unwrap();
        let negative = Int::small(-1).unwrap();
        let huge = Int::new(BigInt::from(1) << 140_u32);
        // SAFETY: the exact list and integer arguments stay rooted. The guard
        // returns only a predicate and must never consume owners or raise.
        unsafe {
            for _ in 0..33 {
                imre_list_append(list, zero.0);
            }
            assert_eq!(imre_list_all_i32(list, zero.0, one.0, one.0), 0);
            assert_eq!(imre_list_all_i32(list, zero.0, nine.0, one.0), 1);
            assert_eq!(imre_list_all_i32(list, nine.0, zero.0, negative.0), 1);
            assert_eq!(imre_list_all_i32(list, zero.0, zero.0, one.0), 0);
            assert_eq!(imre_list_all_i32(list, zero.0, nine.0, zero.0), 0);
            assert_eq!(imre_list_all_i32(list, zero.0, huge.0, one.0), 0);
            assert_eq!(imre_error_pending(), 0);
            imre_list_release(list);
        }
    }

    #[test]
    fn live_range_hints_share_a_budget_and_return_it_on_release() {
        let zero = Int::small(0).unwrap();
        let stop = Int::small(MAX_SMALL).unwrap();
        let one = Int::small(1).unwrap();
        let first = imre_list_new();
        let second = imre_list_new();
        let third = imre_list_new();
        // SAFETY: each list has one live owner, and all range words are
        // immediate integers. Release each list through exactly one path.
        unsafe {
            let reserve = |list| imre_list_reserve_range(list, zero.0, stop.0, one.0, 1);
            for list in [first, second, third] {
                reserve(list);
            }
            assert_eq!((&*first).0.with(Vec::capacity), MAX_REUSED_LIST_CAPACITY);
            assert_eq!((&*second).0.with(Vec::capacity), MAX_REUSED_LIST_CAPACITY);
            assert_eq!((&*third).0.with(Vec::capacity), 0);
            imre_list_release(first);
            reserve(third);
            assert_eq!((&*third).0.with(Vec::capacity), MAX_REUSED_LIST_CAPACITY);
            drop(Rc::from_raw(second));
            imre_list_release(third);
        }
        assert_eq!(RANGE_HINT_SLOTS.with(Cell::get), 0);
    }

    #[test]
    fn range_capacity_hint_is_bounded_and_preserves_existing_values() {
        let list = imre_list_new();
        let zero = Int::small(0).unwrap();
        let one = Int::small(1).unwrap();
        let three = Int::small(3).unwrap();
        let negative = Int::small(-1).unwrap();
        let maximum = Int::small(MAX_SMALL).unwrap();
        let huge = Int::new(BigInt::from(1) << 140_u32);
        // SAFETY: exact handles stay rooted, capacity reads cannot reenter,
        // and the final release destroys the sole list owner.
        unsafe {
            imre_list_reserve_range(std::ptr::null(), zero.0, three.0, one.0, 2);
            imre_list_reserve_range(list, zero.0, three.0, zero.0, 2);
            imre_list_reserve_range(list, zero.0, huge.0, one.0, 2);
            assert_eq!((&*list).0.with(Vec::capacity), 0);
            imre_list_reserve_range(list, three.0, zero.0, negative.0, 2);
            assert_eq!((&*list).0.with(Vec::capacity), 6);
            assert_eq!(imre_list_len(list), zero.0);
            imre_list_append(list, huge.0);
            imre_list_reserve_range(list, zero.0, maximum.0, one.0, usize::MAX);
            assert_eq!((&*list).0.with(Vec::capacity), 6);
            assert_eq!(
                Int::borrow(imre_list_borrow_index(list, 0)).big(),
                huge.big()
            );
            drop(Int(imre_list_pop(list)));
            imre_list_reserve_range(list, zero.0, maximum.0, one.0, usize::MAX);
            assert_eq!((&*list).0.with(Vec::capacity), 32_768);
            assert_eq!(imre_list_len(list), zero.0);
            assert_eq!(imre_error_pending(), 0);
            imre_list_release(list);
        }
    }

    #[test]
    fn container_ownership_survives_replacement_and_release() {
        let big = Int::new(BigInt::from(1_u64 << 63));
        let list = imre_list_new();
        let zero = Int::new(BigInt::zero());
        // SAFETY: handles are live; each acquired reference is released once below.
        unsafe {
            imre_list_append(list, big.0);
            let saved = Int(imre_list_get(list, zero.0));
            assert_eq!(imre_list_set(list, zero.0, zero.0), 1);
            imre_list_release(list);
            drop(big);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u64 << 63));
        }
    }

    #[test]
    fn decoded_indices_acquire_before_replacing_the_only_stored_owner() {
        let big = Rc::new(BigInt::from(1_u128 << 100));
        let weak = Rc::downgrade(&big);
        let word = Rc::as_ptr(&big) as usize;
        let list = imre_list_new();
        // SAFETY: the list roots the integer after its original owner is dropped.
        // Decoded zero is a native index; every acquired result is released.
        unsafe {
            imre_list_append(list, word);
            drop(big);
            assert_eq!(imre_list_set_index(list, 1, word), 0);
            clear_index_error();
            assert_eq!(weak.strong_count(), 1);
            let huge = Int::new(BigInt::from(1_u128 << 100));
            assert_eq!(imre_list_set(list, huge.0, word), 0);
            clear_index_error();
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(imre_list_set_index(list, 0, word), 1);
            let saved = Int(imre_list_get_index(list, 0));
            assert_eq!(imre_list_set_index(list, 0, Int::small(7).unwrap().0), 1);
            assert_eq!(Int(imre_list_len(list)).as_small(), Some(1));
            imre_list_release(list);
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
        let list = imre_list_new();
        // SAFETY: the list roots both integers after their original owners are
        // dropped. Borrowed words are consumed before any list mutation; owned
        // results and the list reference are each released exactly once.
        unsafe {
            imre_list_append(list, Rc::as_ptr(&source) as usize);
            imre_list_append(list, Rc::as_ptr(&displaced) as usize);
            drop(source);
            drop(displaced);
            assert_eq!(imre_list_copy_index(list, 2, 0), 0);
            clear_index_error();
            assert_eq!(source_weak.strong_count(), 1);
            assert_eq!(displaced_weak.strong_count(), 1);
            assert_eq!(imre_list_copy_index(list, 1, 2), 0);
            clear_index_error();
            assert_eq!(source_weak.strong_count(), 1);
            assert_eq!(displaced_weak.strong_count(), 1);
            {
                let borrowed = Int::borrow(imre_list_borrow_index(list, 0));
                assert_eq!(borrowed.big().as_ref(), &BigInt::from(1_u128 << 100));
                assert_eq!(source_weak.strong_count(), 1);
            }
            assert_eq!(imre_list_copy_index(list, 0, 0), 1);
            assert_eq!(source_weak.strong_count(), 1);
            assert_eq!(imre_list_copy_index(list, 1, 0), 1);
            assert_eq!(source_weak.strong_count(), 2);
            assert!(displaced_weak.upgrade().is_none());
            assert_eq!(imre_list_set_index(list, 0, Int::small(7).unwrap().0), 1);
            assert_eq!(source_weak.strong_count(), 1);
            {
                let borrowed = Int::borrow(imre_list_borrow_index(list, 1));
                assert_eq!(borrowed.big().as_ref(), &BigInt::from(1_u128 << 100));
                assert_eq!(source_weak.strong_count(), 1);
            }
            let saved = Int(imre_list_get_index(list, 1));
            imre_list_release(list);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u128 << 100));
            assert_eq!(source_weak.strong_count(), 1);
            drop(saved);
        }
        assert!(source_weak.upgrade().is_none());
    }

    #[test]
    fn proven_list_reads_borrow_boundary_values_without_acquiring_owners() {
        let big = Rc::new(BigInt::from(1_u128 << 100));
        let weak = Rc::downgrade(&big);
        let list = imre_list_new();
        // SAFETY: the empty view is never dereferenced. All later reads are
        // in bounds and finish before the list or any element is released.
        unsafe {
            assert!(imre_list_data_readonly(list).is_aligned());
            imre_list_append(list, Int::small(MIN_SMALL).unwrap().0);
            imre_list_append(list, Rc::as_ptr(&big) as usize);
            imre_list_append(list, Int::small(MAX_SMALL).unwrap().0);
            drop(big);
            assert_eq!(
                Int::borrow(imre_list_borrow_index_proven(list, 0)).as_small(),
                Some(MIN_SMALL)
            );
            assert_eq!(
                Int::borrow(imre_list_borrow_index_proven(list, 1))
                    .big()
                    .as_ref(),
                &BigInt::from(1_u128 << 100)
            );
            assert_eq!(weak.strong_count(), 1);
            assert_eq!(
                Int::borrow(imre_list_borrow_index_proven(list, 2)).as_small(),
                Some(MAX_SMALL)
            );
            let words = imre_list_data_readonly(list);
            for index in 0..3 {
                assert_eq!(
                    *words.add(index),
                    imre_list_borrow_index_proven(list, index)
                );
            }
            assert_eq!(weak.strong_count(), 1);
            imre_list_release(list);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn list_error_words_distinguish_numeric_zero_and_allow_recovery() {
        let list = imre_list_new();
        let zero = Int::small(0).unwrap();
        let last = Int::small(-1).unwrap();
        let before_start = Int::small(-2).unwrap();
        let huge = Int::new(BigInt::from(1_u8) << 100_u32);
        // SAFETY: list and index inputs stay rooted. Successful zero results
        // are immediate integers; error words are inspected without adoption,
        // and each pending exception is cleared before the next fallible call.
        unsafe {
            imre_list_append(list, zero.0);
            assert_eq!(imre_list_get(list, last.0), 1);
            assert_eq!(imre_list_get_index(list, 0), 1);
            assert_eq!(imre_list_borrow_index(list, 0), 1);
            assert_eq!(imre_error_pending(), 0);
            assert_eq!(imre_list_get(list, before_start.0), 0);
            clear_index_error();
            assert_eq!(imre_list_get(list, huge.0), 0);
            clear_index_error();
            assert_eq!(imre_list_get_index(list, 1), 0);
            clear_index_error();
            assert_eq!(imre_list_borrow_index(list, 1), 0);
            clear_index_error();
            assert_eq!(imre_list_set(list, last.0, zero.0), 1);
            assert_eq!(imre_list_pop(list), 1);
            assert_eq!(imre_error_pending(), 0);
            assert_eq!(imre_list_pop(list), 0);
            clear_index_error();
            imre_list_append(list, zero.0);
            assert_eq!(imre_list_pop(list), 1);
            assert_eq!(imre_error_pending(), 0);
            imre_list_release(list);
        }
    }
}
