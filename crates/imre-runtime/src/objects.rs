//! Typed container handles with one representation descriptor per homogeneous buffer.
//!
//! Records have separate compact native allocations. Object handles cover sequences,
//! mappings, iterators, exceptions, and resources. All calls follow the parent's
//! single-threaded, nonreentrant ownership contract. Growable buffers stay private
//! except for compiler-proven readonly loop views; displaced owners are dropped
//! only after mutable storage access ends.

use crate::StrOwner;
mod annotations;
mod argparse;
mod args;
mod asyncio;
mod binascii;
pub(crate) use binascii::{base64_decode, urlsafe_encode};
pub(crate) use binascii::{binascii_decode, binascii_encode};
mod boxed_values;
mod builtins;
mod bytes;
pub(crate) use bytes::write_repr as write_bytes_repr;
pub(crate) use bytes::{
    bytes_contains, bytes_contains_int, concat_bytes, repeat_bytes, rsplit_bytes, rstrip_bytes,
};
mod cache_paths;
mod calendar;
pub(crate) use calendar::boxed::{Calendar, CalendarNumber};
mod chainmap;
mod charmaps;
mod codecs;
pub(crate) use codecs::{Codec, CodecInfo};
mod contexts;
mod contextvars;
mod copying;
mod csv;
mod deferred_diff;
mod deque;
mod difflib;
mod errno;
pub(crate) mod exceptions;
mod files;
mod text_file;
pub(crate) use text_file::TextFile;
pub(crate) mod fnmatch;
mod format_cursors;
mod functools;
mod hashlib;
pub(crate) use hashlib::State as HashState;
pub(crate) mod hmac;
mod inspect;
pub(crate) mod interpreter_stdlib;
mod itertools;
pub(crate) use itertools::Repeat;
mod join_buffer;
mod joined_lines;
mod json;
pub(crate) use json::{
    DecodedJson, JsonNumber, decode_json, decode_json_bytes, json_bom_error,
    write_string as write_json_string,
};
mod linecache;
pub(crate) mod locks;
pub(crate) mod logging;
pub(crate) use logging::record::{brace_integer_field, percent_float_field, percent_integer_field};
mod mappings;
mod module_specs;
mod namespaces;
mod os;
mod patchset;
mod paths;
pub(crate) use paths::is_file;
pub(crate) mod posixpath;
mod processes;
mod random;
pub(crate) use random::imre_random_float;
pub(crate) mod readiness;
pub(crate) mod records;
mod regexes;
mod sequences;
mod shutil;
pub(crate) mod socket;
mod source_modules;
pub(crate) mod stdio;
mod stdlib;
pub(crate) use stdlib::comparator::sort_by_less;
pub(crate) use stdlib::format_fixed_float;
mod string_enums;
mod structs;
mod tempdirs;
mod text;
pub(crate) use text::translate;
pub(crate) mod time;
pub(crate) use text::{UrlInput, capitalize, urlquote, urlsplit};
pub(crate) use text::{
    expand_tabs, find_text, ord_byte, ord_char, partition_text, python_isspace, split_parts,
};
mod typing;
pub(crate) use text::{chr_scalar, short_string};
pub(crate) use typing::{OVERLOAD_DUMMY_ERROR, register_overload};
mod unions;
mod values;
mod warnings;
mod words;
mod zipfile;

pub(crate) use errno::{DOC as ERRNO_DOC, platform_entries as errno_entries};
pub(crate) use files::{FileState, errno_error, open_file};

use super::{
    Bytes, Error, Int, Kind, List, MAX_LIST_LEN, StorageCell, Str, decode_index, fail, finish,
    index,
};
use num_traits::ToPrimitive;
use std::rc::Rc;

/// Literal constructors share the same empty and Latin-1 owners as chr and indexing.
pub(crate) fn literal_string(text: &str) -> StrOwner {
    self::text::owned_string(text)
}
use words::{OwnedWord, Words};

// All C ints fit in the immediate representation. Heap integers cannot fit.
fn c_int(word: usize) -> exceptions::Result<libc::c_int> {
    (word & 1 != 0)
        .then_some((word as i64) >> 1)
        .and_then(|n| libc::c_int::try_from(n).ok())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C int",
            )
        })
}

/// An opaque allocation for runtime-managed containers and resource state.
#[repr(transparent)]
pub struct Object(StorageCell<Storage>);

enum Storage {
    Plain,
    WeakProxy(records::weak::WeakRecord, bool),
    FunctoolsPlaceholder {
        class: bool,
    },
    Context(Rc<contextvars::State>),
    ContextVar(Rc<contextvars::Variable>),
    ContextToken(Rc<contextvars::Token>),
    TokenMissing,
    Socket(socket::Socket),
    StrDescriptor(u32),
    Sequence(Words),
    JoinBuffer(allocator_api2::boxed::Box<join_buffer::JoinBuffer>),
    Deque(Box<deque::Deque>),
    DequeIterator(deque::DequeIterator),
    Dictionary(mappings::TableOwner<mappings::Mapping>),
    ChainMap(crate::ObjectOwner),
    FunctionCache(Box<mappings::FunctionCache>),
    Set(mappings::TableOwner<mappings::Set>),
    MappingIterator(mappings::MappingIterator),
    StringIterator(sequences::StringIterator),
    RegexIterator(regexes::FindIter),
    Repeat(Repeat<OwnedWord>),
    IteratorAdapter(Rc<itertools::Adapter>),
    LineIterator(LineIterator),
    BytesIterator(bytes::BytesIterator),
    ByteOutput(bytes::ByteOutput),
    TupleBuffer(Vec<OwnedWord>),
    SimpleNamespace(crate::ObjectOwner),
    SourceModule {
        name: crate::StrOwner,
        filename: crate::StrOwner,
    },
    Hash(allocator_api2::boxed::Box<hashlib::State>),
    Poll(Box<readiness::Poll>),
    NativeModule(errno::Module),
    ModuleNamespace(Rc<errno::Registry>),
    ModuleMember(errno::Member),
    CsvReader(Box<csv::Reader>),
    CodecInfo(codecs::Codec),
    CodecWriterClass(codecs::writer::WriterClass),
    CodecWriter(allocator_api2::boxed::Box<codecs::writer::Writer>),
    Exception(exceptions::Exception),
    ObjectValue(OwnedWord),
    File(files::BoxedFile),
    ZipFile(allocator_api2::boxed::Box<zipfile::Archive>),
    ZipInfo(crate::Shared<zipfile::Info>),
    Popen(Box<processes::Popen>),
    ProcessSelectorClass(bool),
    AsyncProcess(Box<processes::AsyncProcess>),
    AsyncCoroutine(Rc<asyncio::Task>),
    AsyncTask(Rc<asyncio::Task>),
    GeneratorContext(Rc<contexts::Manager>),
    DirectoryContext(tempdirs::DirectoryContext),
    Document(json::readonly::document::Value),
    EventLoop(Rc<asyncio::EventLoop>),
    TaskGroup(Rc<asyncio::Group>),
    Semaphore(Rc<asyncio::Semaphore>),
    Path {
        text: paths::PathText,
        name: std::cell::OnceCell<StrOwner>,
    },
    PathInfo(Box<paths::Info>),
    PurePath(Box<paths::PurePath>),
    PathIterator(Box<paths::PathIterator>),
    PatchSet(patchset::PatchSet),
    PatchedFile(patchset::PatchedFile),
    Hunk(patchset::Hunk),
    RegexPattern(Box<regexes::Pattern>),
    Logger(Rc<logging::Logger>),
    LogManager(Option<logging::manager::Manager>),
    HandlerLock(std::sync::Arc<logging::locks::HandlerLock>),
    TimedFileHandler(Box<logging::timed::Handler>),
    Calendar(allocator_api2::boxed::Box<calendar::Value>),
    LogRecord(Box<logging::record::Record>),
    LogFormatter(Box<logging::formatter::Formatter>),
    RegexMatch(regexes::Match),
    Json(json::readonly::View),
    JsonDecoder(crate::ObjectOwner),
}

/// Borrows a rooted object for one nonreentrant operation.
unsafe fn object<'a>(pointer: *const Object) -> &'a Object {
    if pointer.is_null() {
        fail("TypeError", "cannot access an absent optional value");
    }
    // SAFETY: the caller roots this allocation for the operation.
    unsafe { &*pointer }
}

impl Storage {
    fn sequence_storage(&self) -> &Words {
        match self {
            Self::Sequence(values) => values,
            _ => fail("SystemError", "expected a sequence object"),
        }
    }
    fn sequence(&self) -> Result<&Words, Error> {
        self.sequence_storage().resolved()
    }
    // The caller owns a concrete sequence whose construction excludes a deferred plan.
    fn concrete_sequence_mut(&mut self) -> &mut Words {
        let Self::Sequence(words) = self else {
            fail("SystemError", "expected a sequence object")
        };
        debug_assert!(words.plan().is_none());
        words.forget_order();
        words
    }
    fn sequence_mut(&mut self) -> Result<&mut Words, Error> {
        let values = self.sequence_storage_mut();
        values.resolve_mut()?;
        values.forget_order();
        Ok(values)
    }
    fn sequence_storage_mut(&mut self) -> &mut Words {
        match self {
            Self::Sequence(values) => values,
            _ => fail("SystemError", "expected a sequence object"),
        }
    }
}

/// # Safety
/// `value` is a live Object handle or an absent nullable value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_object_retain(value: *const Object) {
    if !value.is_null() {
        // SAFETY: the caller roots this allocation.
        unsafe { crate::ObjectOwner::increment_strong_count(value) };
    }
}

/// # Safety
/// Transfers one Object reference, or accepts null during nullable/partial cleanup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_object_release(value: *const Object) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live strong reference.
        release_object_owner(unsafe { crate::ObjectOwner::from_raw(value) });
    }
}

/// Give a live Popen's final language owner to subprocess._active before its
/// allocation is destroyed. All other object owners follow the usual Rc drop.
fn release_object_owner(owner: crate::ObjectOwner) {
    if crate::ObjectOwner::strong_count(&owner) == 1 {
        processes::retain_live_owner(&owner);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn imre_seq_new(kind: u32) -> *const Object {
    crate::ObjectOwner::into_raw(Words::new(kind).into_object())
}

/// Remember a mock history's `_CallList` identity across aliases and resets.
/// Copies get the ordinary list representation from their fresh sequence storage.
///
/// # Safety
/// The caller roots a native sequence of recorded mock calls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_mock_call_list(value: *const Object, mark: bool) -> bool {
    // SAFETY: the caller roots the list and only the copied flag escapes the borrow.
    unsafe {
        object(value).0.with(|storage| {
            let flag = &storage.sequence_storage().mock_call_list;
            if mark {
                flag.set(true);
            }
            flag.get()
        })
    }
}

/// Tests string membership with the list and needle rooted for the whole read.
///
/// # Safety
/// The caller supplies a live string and an exact list of initialized string owners.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_list_contains(needle: *const Str, list: *const Object) -> u32 {
    // SAFETY: the storage borrow prevents mutation and none of the compared
    // strings can invoke user code or release its live owner during equality.
    unsafe {
        object(list).0.with(|storage| {
            finish(
                storage
                    .sequence()
                    .map(|words| u32::from(words.contains_string(&*needle))),
                0,
            )
        })
    }
}

/// # Safety
/// Root a sequence and exact canonical Int index. The result owns one value of
/// the sequence's declared kind; pending must be checked before interpreting zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_get(value: *const Object, at: usize) -> usize {
    // SAFETY: the index remains rooted for conversion.
    let at = unsafe { Int::borrow(at) };
    let at = match decode_index(&at) {
        Ok(at) => at,
        Err(error) => return error.returning(0),
    };
    // SAFETY: acquisition cannot reenter and only an owned word escapes.
    let owned = unsafe {
        object(value).0.with(|storage| {
            if let Some(rows) = storage.sequence_storage().rows() {
                return rows.get(index(at, rows.len())?);
            }
            let values = storage.sequence()?;
            Ok(values
                .kind
                .acquire(values.values[index(at, values.values.len())?])
                .into_word())
        })
    };
    finish(owned, 0)
}

/// # Safety
/// `at` is the exact compiler-proven nonnegative native index, not a tagged word.
/// The rooted sequence keeps current bounds checking and returns an owned value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_get_index(value: *const Object, at: usize) -> usize {
    // SAFETY: only an independently acquired value leaves the storage access.
    let owned = unsafe {
        object(value).0.with(|storage| {
            if let Some(rows) = storage.sequence_storage().rows() {
                return rows.get(at);
            }
            let values = storage.sequence()?;
            let word = *values.values.get(at).ok_or_else(Error::index)?;
            Ok(values.kind.acquire(word).into_word())
        })
    };
    finish(owned, 0)
}

/// Transfers a private list slot to its single forward iterator. Unvisited
/// elements are still owned by the normal list destructor on every loop exit.
///
/// # Safety
/// `value` is a rooted, ordinary typed list. No alias may observe a consumed
/// slot and `at` must be a fresh in-bounds native index for this iterator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_take_index_proven(value: *const Object, at: usize) -> usize {
    // SAFETY: the list owns each stored word; zero is the empty destructor state.
    unsafe {
        object(value)
            .0
            .with_mut(|storage| std::mem::take(&mut storage.concrete_sequence_mut().values[at]))
    }
}

/// Borrows one concrete sequence word after its current bounds were proved.
/// Zero remains an ordinary scalar or absent value, never an error sentinel.
///
/// # Safety
/// `value` must be a non-null rooted exact sequence, and `at` a decoded native index
/// strictly below its current length. No mutable storage access may overlap the
/// call. Consume or acquire the returned word with its exact declared type before
/// removing or replacing that element. This does not acquire a reference, and
/// neither a storage reference nor a growable buffer pointer escapes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_borrow_index_proven(value: *const Object, at: usize) -> usize {
    // SAFETY: SequenceGetInBounds supplies a non-null exact sequence and proves
    // its current bounds. Its two emission paths either acquire the result
    // immediately or keep an unchanged caller root. Neither a storage reference
    // nor a pointer into the movable buffer escapes this nonreentrant read.
    unsafe {
        (&*value).0.with(|storage| {
            let Storage::Sequence(values) = storage else {
                // Exact kind is part of the same caller proof as the index.
                std::hint::unreachable_unchecked()
            };
            finish(
                values.resolved().map(|rows| *rows.values.get_unchecked(at)),
                0,
            )
        })
    }
}

/// Exposes an immutable word buffer only for a compiler-proven readonly loop.
/// Empty sequences may return a dangling, aligned pointer that must not be read.
///
/// # Safety
/// `value` is a non-null rooted exact sequence. Keep that same owner alive and
/// prohibit every mutation, reserve, and reentrant access to its storage until
/// the last use of the result. The pointer may exist only as private compiler
/// state, never as a language value or an escaping return/field/container value.
/// Every read must prove its native index below the unchanged current length;
/// consume or acquire each word using the sequence's exact declared element type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_data_readonly(value: *const Object) -> *const usize {
    // SAFETY: this stronger, whole-loop contract deliberately bypasses `with`'s
    // per-call no-buffer-escape rule. Only a raw pointer escapes the short shared
    // storage borrow; the caller keeps its allocation rooted and immutable.
    unsafe {
        let Storage::Sequence(values) = &*(*value).0.0.get() else {
            std::hint::unreachable_unchecked()
        };
        finish(
            values.resolved().map(|rows| rows.values.as_ptr()),
            std::ptr::null(),
        )
    }
}

/// Acquires the replacement before any existing container owner can be displaced.
unsafe fn replacement(value: *const Object, word: usize, kind: u32) -> Result<OwnedWord, Error> {
    // SAFETY: only copied descriptor metadata escapes this rooted storage read.
    let actual = unsafe {
        object(value).0.with(|storage| {
            let values = storage.sequence()?;
            values.check_kind(kind);
            Ok(values.kind)
        })
    }?;
    // SAFETY: the compiler supplies a borrowed word of the exact element type.
    Ok(unsafe { actual.acquire(word) })
}

/// # Safety
/// All handles are rooted; the replacement has the sequence's declared kind and
/// preserves heap acyclicity. Failed indexing leaves the sequence unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_set(value: *const Object, at: usize, word: usize, kind: u32) {
    // SAFETY: the caller roots the exact canonical integer index.
    let at = unsafe { Int::borrow(at) };
    let at = match decode_index(&at) {
        Ok(at) => at,
        Err(error) => return error.raise(),
    };
    // SAFETY: normalize against the current live row count before changing storage.
    if unsafe { structs::set_if_packed(value, |len| index(at, len), word) } {
        return;
    }
    // SAFETY: the caller roots the checked replacement through acquisition.
    let mut new = Some(match unsafe { replacement(value, word, kind) } {
        Ok(owner) => owner,
        Err(error) => return error.raise(),
    });
    // SAFETY: ownership leaves mutable storage before any displaced value drops.
    let old = unsafe {
        object(value).0.with_mut(|storage| {
            let values = storage.sequence_mut()?;
            let at = index(at, values.values.len())?;
            Ok(OwnedWord::from_owned(
                std::mem::replace(&mut values.values[at], new.take().unwrap().into_word()),
                values.kind,
            ))
        })
    };
    drop(new);
    finish(old.map(drop), ());
}

/// # Safety
/// As for set, but `at` is an exact nonnegative native index. Bounds are checked.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_set_index(
    value: *const Object,
    at: usize,
    word: usize,
    kind: u32,
) {
    // SAFETY: the input row is rooted across this immediate copy.
    if unsafe { structs::set_if_packed(value, |_| Ok(at), word) } {
        return;
    }
    // SAFETY: the checked replacement is acquired before replacing its last owner.
    let mut new = Some(match unsafe { replacement(value, word, kind) } {
        Ok(word) => word,
        Err(err) => return err.raise(),
    });
    // SAFETY: no reference escapes; the displaced owner is dropped afterward.
    let old = unsafe {
        object(value).0.with_mut(|storage| {
            let values = storage.sequence_mut()?;
            let slot = values.values.get_mut(at).ok_or_else(Error::index)?;
            Ok(OwnedWord::from_owned(
                std::mem::replace(slot, new.take().unwrap().into_word()),
                values.kind,
            ))
        })
    };
    drop(new);
    finish(old.map(drop), ());
}

/// # Safety
/// The rooted word matches this sequence's element kind and preserves acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_append(value: *const Object, word: usize, kind: u32) -> i32 {
    // SAFETY: the borrowed word stays rooted while the backing buffer moves.
    // Acquiring a word cannot call user code or displace any existing owner.
    let appended = unsafe {
        object(value).0.with_mut(|storage| {
            let words = storage.sequence_storage_mut();
            if kind == 6 && words.kind.raw() == 6 && words.values.is_empty() {
                let layout = records::descriptor(word as *const usize);
                if layout.is_value_struct() {
                    *words = Words::from_rows(structs::Rows::new(layout));
                }
            }
            if let Some(rows) = words.rows_mut() {
                let result = rows.append(rows.payload(word as *const usize));
                words.update_header();
                return result;
            }
            let sequence = storage.sequence_mut()?;
            sequence.check_kind(kind);
            sequence
                .values
                .try_reserve(1)
                .map_err(|_| Error::allocation())?;
            sequence
                .values
                .push(sequence.kind.acquire(word).into_word());
            sequence.update_header();
            Ok(())
        })
    };
    finish(appended.map(|()| 1), 0)
}

/// Publish an empty typed list before releasing the elements of the old buffer.
///
/// # Safety
/// The caller keeps the mutable list rooted for the duration of the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_clear(value: *const Object) {
    // SAFETY: only the detached buffer escapes; no element destructor runs under the borrow.
    let previous = unsafe {
        object(value).0.with_mut(|storage| {
            let Storage::Sequence(words) = storage else {
                unreachable!("a typed list owns sequence storage")
            };
            if let Some(rows) = words.rows_mut() {
                rows.clear();
                words.update_header();
                return None;
            }
            if words.is_sorting_empty() {
                return None;
            }
            // A deferred diff is a list of strings. Clearing it discards its plan
            // without materializing values that no observer can access.
            let kind = if words.plan().is_some() {
                3
            } else {
                words.kind.raw()
            };
            Some(std::mem::replace(words, Words::new(kind)))
        })
    };
    drop(previous);
}

/// Reverse the owned slots in place without acquiring or releasing them.
///
/// # Safety
/// The caller roots the mutable list for the duration of the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_reverse(value: *const Object) {
    // SAFETY: resolving the native sequence and swapping slots cannot invoke Python.
    unsafe {
        object(value).0.with_mut(|storage| {
            finish(
                storage.sequence_mut().map(|words| words.values.reverse()),
                (),
            )
        })
    };
}

/// # Safety
/// The sequence is rooted. Success transfers its removed owner's exact word;
/// check pending before interpreting zero, which may also be a valid element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_pop(value: *const Object) -> usize {
    // SAFETY: only a transferred owner escapes mutable storage.
    let owned = unsafe {
        object(value).0.with_mut(|storage| {
            if let Storage::Sequence(words) = storage
                && let Some(rows) = words.rows_mut()
            {
                let result = rows.pop();
                words.update_header();
                return result;
            }
            let values = storage.sequence_mut()?;
            let word = values
                .values
                .pop()
                .ok_or_else(|| Error::new(Kind::IndexError, "pop from empty list"))?;
            values.update_header();
            Ok(word)
        })
    };
    finish(owned, 0)
}

/// # Safety
/// `value` is a rooted concrete sequence.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_len(value: *const Object) -> usize {
    // SAFETY: only the copied length leaves storage.
    let length = unsafe {
        object(value).0.with(|storage| {
            let words = storage.sequence_storage();
            if let Some(rows) = words.rows() {
                return Ok(rows.len());
            }
            words
                .plan()
                .map_or_else(|| Ok(words.values.len()), deferred_diff::Deferred::len)
        })
    };
    let length = match length {
        Ok(n) => n,
        Err(error) => return error.returning(0),
    };
    debug_assert!(length <= MAX_LIST_LEN);
    (length << 1) | 1
}

/// Repetition preserves the element representation and shares referenced objects.
///
/// # Safety
/// Both sequence and canonical integer count remain rooted throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_seq_repeat(value: *const Object, count: usize) -> *const Object {
    // SAFETY: the caller roots the integer count.
    let count = unsafe { Int::borrow(count) };
    let count = count
        .as_small()
        .map(|value| value as isize)
        .or_else(|| count.big().to_isize());
    let Some(count) = count else {
        return Error::new(Kind::OverflowError, "list repeat count is too large")
            .returning(std::ptr::null());
    };
    // SAFETY: only independent owned words escape; acquisition cannot reenter.
    let result = unsafe {
        object(value).0.with(|storage| {
            let count = usize::try_from(count).unwrap_or(0);
            if let Some(rows) = storage.sequence_storage().rows() {
                return rows.repeat(count);
            }
            let values = storage.sequence()?;
            let length = values
                .values
                .len()
                .checked_mul(count)
                .filter(|&length| length <= MAX_LIST_LEN)
                .ok_or_else(|| Error::new(Kind::MemoryError, "list is too large"))?;
            let mut result = Words::new(values.kind.raw());
            result
                .values
                .try_reserve_exact(length)
                .map_err(|_| Error::allocation())?;
            if !values.values.is_empty() {
                for _ in 0..count {
                    for &word in &values.values {
                        result.values.push(values.kind.acquire(word).into_word());
                    }
                }
            }
            Ok(result)
        })
    };
    finish(
        result.map(|values| crate::ObjectOwner::into_raw(values.into_object())),
        std::ptr::null(),
    )
}

/// Concatenate two Python strings after both optional operands have been evaluated.
///
/// # Safety
/// Nonzero words are rooted, immutable strings. Zero represents Python None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_optional_str_concat(left: usize, right: usize) -> *const Str {
    let reason = match (left == 0, right == 0) {
        (true, true) => "unsupported operand type(s) for +: 'NoneType' and 'NoneType'",
        (true, false) => "unsupported operand type(s) for +: 'NoneType' and 'str'",
        (false, true) => "can only concatenate str (not \"NoneType\") to str",
        (false, false) => {
            // SAFETY: both non-null string owners are retained by the caller.
            return unsafe { imre_str_concat(left as *const Str, right as *const Str) };
        }
    };
    Error::new(Kind::TypeError, reason).returning(std::ptr::null())
}

/// # Safety
/// Both arguments must be rooted, non-null immutable string handles.
/// Allocation failure raises MemoryError and returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_concat(a: *const Str, b: *const Str) -> *const Str {
    // SAFETY: both immutable strings remain rooted for this call.
    let (left, right) = unsafe { (&*a, &*b) };
    if left.is_empty() || right.is_empty() {
        let retained = if left.is_empty() { b } else { a };
        // SAFETY: the caller roots the original immutable string handle.
        return StrOwner::into_raw(unsafe { crate::StrOwner::acquire_word(retained as usize) });
    }
    let Some(mut result) = concatenated_length(left.len(), [right.len()])
        .and_then(|length| Str::try_with_capacity(length).ok())
    else {
        return Error::allocation().returning(std::ptr::null());
    };
    result.push_str(left);
    result.push_str(right);
    finish(
        StrOwner::try_new(result)
            .map(StrOwner::into_raw)
            .map_err(|_| Error::allocation()),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots the string and tagged integer until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_repeat(value: *const Str, count: usize) -> *const Str {
    // SAFETY: both arguments remain rooted for this non-reentrant operation.
    let (text, count) = unsafe { (&*value, Int::borrow(count)) };
    let result = repeat_str(text, &count).map(|result| StrOwner::into_raw(StrOwner::new(result)));
    finish(result, std::ptr::null())
}

/// Repeats UTF-8 text with the native sequence length and allocation checks.
pub(crate) fn repeat_str(text: &str, count: &Int) -> Result<Str, Error> {
    let count = bytes::native_size(count)?.max(0) as usize;
    if text.is_empty() || count == 0 {
        return Ok(Str::default());
    }
    let length = text
        .len()
        .checked_mul(count)
        .filter(|length| *length <= isize::MAX as usize)
        .ok_or_else(|| Error::new(Kind::OverflowError, "repeated string is too long"))?;
    let mut result =
        Str::try_with_capacity(length).map_err(|_| Error::new(Kind::MemoryError, ""))?;
    for _ in 0..count {
        result.push_str(text);
    }
    Ok(result)
}

/// One borrowed concatenation fragment. `usize::MAX` identifies a rooted Str
/// handle; other lengths identify immutable, valid UTF-8 bytes, including
/// compiler literals and caller-owned formatting scratch. A consuming builder
/// also roots fragments borrowed directly from its transferred left operand.
#[repr(C)]
pub struct StringPart {
    data: *const u8,
    length: usize,
}

const _: () = assert!(size_of::<StringPart>() == 16);

impl StringPart {
    /// # Safety
    /// This fragment's handle or byte span remains readable and immutable for
    /// the borrow. Nonempty spans contain valid UTF-8; empty spans may be null.
    unsafe fn as_str(&self) -> &str {
        if self.length == usize::MAX {
            // SAFETY: the sentinel selects a rooted, correctly aligned Str handle.
            unsafe { &*self.data.cast::<Str>() }.as_str()
        } else if self.length == 0 {
            ""
        } else {
            // SAFETY: the caller supplies this immutable, valid UTF-8 span.
            unsafe {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.data, self.length))
            }
        }
    }
}

/// Writes one indexed Unicode scalar as a borrowed fragment without allocating.
/// Returns one on success; zero installs StringGet's error and leaves both
/// outputs untouched. The fragment remains valid after releasing both inputs.
///
/// # Safety
/// `value` and the canonical integer `at` remain rooted for this call. `buffer`
/// points to four writable bytes and `part` to one aligned writable descriptor;
/// both are disjoint from each other and from every input. Enter with no pending
/// error and check the status before reading either output. Keep the written
/// span immutable until its concatenation finishes; neither input is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_get_part(
    value: *const Str,
    at: usize,
    buffer: *mut u8,
    part: *mut StringPart,
) -> u32 {
    // SAFETY: both immutable inputs remain rooted throughout Unicode indexing.
    let (value, at) = unsafe { (&*value, Int::borrow(at)) };
    let character = match sequences::string_character(value, &at) {
        Ok(character) => character,
        Err(error) => return error.returning(0),
    };
    // SAFETY: the caller provides four disjoint writable bytes, enough for every
    // Unicode scalar. No output is touched before all recoverable checks succeed.
    let buffer_bytes = unsafe { std::slice::from_raw_parts_mut(buffer, 4) };
    let length = character.encode_utf8(buffer_bytes).len();
    // SAFETY: the writable descriptor borrows the caller's initialized UTF-8 span.
    unsafe {
        part.write(StringPart {
            data: buffer,
            length,
        });
    }
    1
}

/// Formats one integer at its expression position without allocating for small
/// values. Null means the fragment borrows `buffer`; otherwise the result owns
/// the fallback string referenced by `part`. A pending digit-limit error means `part`
/// is uninitialized. No outcome consumes `value`.
///
/// # Safety
/// `value` is a rooted canonical integer for this call. `buffer` points to 20
/// writable bytes, and `part` to one aligned writable descriptor; they do not
/// overlap each other or any borrowed input. Keep the written span immutable,
/// or the returned string rooted, until the fragment's concatenation finishes.
/// The returned nullable owner must be released once. Allocation failure is fatal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_int_str_part(
    value: usize,
    buffer: *mut u8,
    part: *mut StringPart,
) -> *const Str {
    // SAFETY: the caller roots this integer throughout formatting.
    let borrowed = unsafe { Int::borrow(value) };
    if let Some(value) = borrowed.as_small() {
        let mut formatted = itoa::Buffer::new();
        let formatted = formatted.format(value);
        // SAFETY: every i64 decimal spelling fits 20 bytes. The caller's scratch
        // and descriptor are writable and disjoint from this local formatter.
        unsafe {
            std::ptr::copy_nonoverlapping(formatted.as_ptr(), buffer, formatted.len());
            part.write(StringPart {
                data: buffer,
                length: formatted.len(),
            });
        }
        std::ptr::null()
    } else {
        // SAFETY: the input is still rooted; this conversion returns an independent
        // string owner and performs arbitrary-precision formatting exactly once.
        let result = unsafe { super::imre_int_str(value) };
        if result.is_null() {
            return result;
        }
        // SAFETY: the caller provides a writable descriptor, and the returned
        // owner keeps its immutable Str allocation alive for concatenation.
        unsafe {
            part.write(StringPart {
                data: result.cast(),
                length: usize::MAX,
            });
        }
        result
    }
}

/// Joins already evaluated fragments into one owned string without intermediate
/// strings or a temporary fragment vector. Capacity is checked before copying.
///
/// # Safety
/// `parts` is an aligned, readable array of `count` initialized descriptors,
/// or null when `count` is zero. Each handle remains rooted and each byte span
/// remains immutable and valid UTF-8 for this call. The array and spans obey Rust
/// slice size limits. Allocation failure raises MemoryError and returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_concat_parts(
    parts: *const StringPart,
    count: usize,
) -> *const Str {
    // SAFETY: this entry point has the builder's same fragment-lifetime contract.
    finish(
        unsafe { concat_parts(parts, count) }.map(StrOwner::into_raw),
        std::ptr::null(),
    )
}

/// Appends already evaluated fragments, reusing uniquely owned string storage.
/// Shared strings and suffixes borrowing the left operand use a fresh result;
/// the consumed owner keeps every such borrow alive until copying finishes.
/// CompactString's reservation grows geometrically across repeated appends.
///
/// # Safety
/// `left` transfers exactly one live, non-null StrOwner owner. `parts` obeys the
/// borrowing builder's descriptor and UTF-8 requirements, and its array is
/// disjoint from `left` and its storage. A suffix may borrow `left` directly,
/// through its handle or initialized bytes in its character storage, without
/// acquiring another owner. Other inputs stay rooted, immutable, and disjoint
/// from the transferred Str object's representation throughout the call.
/// On success the returned handle owns one reference. On allocation failure,
/// returns null with MemoryError pending and restores the original left owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_concat_parts_owned(
    left: *const Str,
    parts: *const StringPart,
    count: usize,
) -> *const Str {
    // SAFETY: the caller transfers this exact, live Rc owner once.
    let mut owner = unsafe { StrOwner::from_raw(left) };
    let parts = if count == 0 {
        &[]
    } else {
        // SAFETY: the caller provides this initialized, disjoint descriptor array.
        unsafe { std::slice::from_raw_parts(parts, count) }
    };
    // SAFETY: the caller and the consumed owner root all fragments through the call.
    let Some((length, shared)) = (unsafe { concat_size(owner.len(), parts) }) else {
        return failed_append(owner);
    };
    if let Some(shared) = shared {
        return StrOwner::into_raw(shared);
    }
    if length == owner.len() {
        return StrOwner::into_raw(owner);
    }

    // Raw byte fragments need not acquire an Rc, so strong uniqueness alone is
    // insufficient. Include spare capacity: initialized spans there can also be
    // invalidated by reallocation. No borrowed view survives the mutable access.
    let start = owner.as_ptr() as usize;
    let end = start.saturating_add(owner.capacity());
    let borrows_left = parts.iter().any(|part| {
        if part.length == usize::MAX {
            part.data == left.cast()
        } else {
            let part_start = part.data as usize;
            part.length != 0 && part_start < end && start < part_start.saturating_add(part.length)
        }
    });
    if !borrows_left && let Some(value) = StrOwner::get_mut(&mut owner) {
        if value.try_reserve(length - value.len()).is_err() {
            return failed_append(owner);
        }
        {
            // SAFETY: reservation covers the complete suffix, and alias screening
            // excludes every borrowed source from the storage being mutated. The
            // spare slice begins after the initialized prefix, which stays intact.
            let output = unsafe { value.spare_capacity_mut() }
                .as_mut_ptr()
                .cast::<u8>();
            let mut offset = 0;
            for part in parts {
                // SAFETY: all inputs remain rooted, immutable, and disjoint.
                let text = unsafe { part.as_str() };
                // SAFETY: the checked suffix length bounds every copy. As in the
                // fresh builder, the final inline tag byte is overwritten only
                // with the final byte of complete, initialized UTF-8. Do not read
                // or move the string until its length is published below.
                unsafe {
                    std::ptr::copy_nonoverlapping(text.as_ptr(), output.add(offset), text.len());
                }
                offset += text.len();
            }
        }
        // SAFETY: the original prefix and complete UTF-8 suffix are initialized
        // within the reserved allocation, including a completely full inline one.
        unsafe { value.set_len(length) };
        return StrOwner::into_raw(owner);
    }

    let Ok(mut result) = Str::try_with_capacity(length) else {
        return failed_append(owner);
    };
    result.push_str(&owner);
    for part in parts {
        // SAFETY: all inputs, including self-borrows, remain rooted and unchanged.
        result.push_str(unsafe { part.as_str() });
    }
    finish_shared_append(owner, StrOwner::try_new(result))
}

/// Appends a compiler-pooled literal, retaining its root for an empty prefix.
///
/// # Safety
/// The caller transfers one left owner. The root points to a live Str handle
/// containing exactly the immutable, program-lifetime UTF-8 span in data.
/// A null result restores the original left owner with MemoryError pending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_append_literal_owned(
    left: *const Str,
    root: *const *const Str,
    data: *const u8,
    length: usize,
) -> *const Str {
    if length == 0 {
        return left;
    }
    // SAFETY: the caller transfers this exact owner. Static bytes cannot
    // overlap a writable heap buffer.
    let mut owner = unsafe { StrOwner::from_raw(left) };
    // SAFETY: the nonempty static span is disjoint from writable heap storage.
    if unsafe { append_reserved(&mut owner, data, length) } {
        return StrOwner::into_raw(owner);
    }
    // SAFETY: the program owns the literal root throughout the call.
    let literal = unsafe { root.read() };
    let part = StringPart {
        data: literal.cast(),
        length: usize::MAX,
    };
    // SAFETY: the descriptor stays live and the left owner transfers.
    unsafe { imre_str_concat_parts_owned(StrOwner::into_raw(owner), &part, 1) }
}

/// Appends one disjoint UTF-8 span, retaining unique string storage and growing
/// it geometrically only when the existing capacity cannot hold the suffix.
///
/// # Safety
/// `left` transfers exactly one live, non-null StrOwner owner. `data` points to
/// `length` immutable valid UTF-8 bytes disjoint from both the Str object and
/// its character allocation, including spare capacity. The span obeys Rust
/// slice size limits and remains readable until return; it may be null only
/// when `length` is zero. On success the returned handle owns one reference.
/// On allocation failure, returns null and restores the original left owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_append_utf8_owned(
    left: *const Str,
    data: *const u8,
    length: usize,
) -> *const Str {
    if length == 0 {
        return left;
    }
    // SAFETY: the caller transfers this exact live owner, including on copying.
    let mut owner = unsafe { StrOwner::from_raw(left) };
    // SAFETY: the caller supplies a disjoint nonempty span.
    if unsafe { append_reserved(&mut owner, data, length) } {
        return StrOwner::into_raw(owner);
    }
    // SAFETY: the unchanged owner and nonempty disjoint UTF-8 span are transferred.
    unsafe { append_utf8_slow(owner, data, length) }
}

/// # Safety
/// The readable nonempty UTF-8 span must be disjoint from a writable heap buffer.
#[inline]
unsafe fn append_reserved(owner: &mut StrOwner, data: *const u8, length: usize) -> bool {
    if let Some(value) = StrOwner::get_mut(owner)
        && value.is_heap_allocated()
    {
        let initial = value.len();
        if length <= value.capacity() - initial {
            // SAFETY: the capacity bound proves the total fits both usize and
            // the live allocation. The complete suffix is disjoint and valid
            // UTF-8; its heap destination cannot overwrite the Str's inline tag.
            unsafe {
                let output = value.as_mut_ptr().add(initial);
                std::ptr::copy_nonoverlapping(data, output, length);
                value.set_len(initial + length);
            }
            return true;
        }
    }
    false
}

/// Keeps inline/static storage, growth, and shared-owner copying outside the
/// common append into a uniquely owned, already reserved heap buffer.
///
/// # Safety
/// `length` is nonzero and the span obeys `imre_str_append_utf8_owned`'s contract.
#[cold]
#[inline(never)]
unsafe fn append_utf8_slow(mut owner: StrOwner, data: *const u8, length: usize) -> *const Str {
    let Some(total) = concatenated_length(owner.len(), [length]) else {
        return failed_append(owner);
    };
    if let Some(value) = StrOwner::get_mut(&mut owner) {
        if value.capacity() < total && value.try_reserve(length).is_err() {
            return failed_append(owner);
        }
        // SAFETY: capacity covers the checked total and the caller guarantees
        // a disjoint initialized suffix. Do not inspect or move the string after
        // copying its final inline tag byte until the complete length is set.
        unsafe {
            let output = value.spare_capacity_mut().as_mut_ptr().cast::<u8>();
            std::ptr::copy_nonoverlapping(data, output, length);
            value.set_len(total);
        }
        StrOwner::into_raw(owner)
    } else {
        // SAFETY: the validated nonempty UTF-8 span remains readable and disjoint.
        let suffix =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(data, length)) };
        let Ok(mut result) = Str::try_with_capacity(total) else {
            return failed_append(owner);
        };
        result.push_str(&owner);
        result.push_str(suffix);
        finish_shared_append(owner, StrOwner::try_new(result))
    }
}

fn finish_shared_append(
    owner: StrOwner,
    result: Result<StrOwner, allocator_api2::alloc::AllocError>,
) -> *const Str {
    match result {
        Ok(result) => StrOwner::into_raw(result),
        Err(_) => failed_append(owner),
    }
}

/// Give the unmodified reference back to the caller's existing slot.
#[cold]
fn failed_append(owner: StrOwner) -> *const Str {
    let _ = StrOwner::into_raw(owner);
    Error::allocation().returning(std::ptr::null())
}

/// Checks both arithmetic overflow and Rust's maximum contiguous allocation size.
fn concatenated_length(initial: usize, lengths: impl IntoIterator<Item = usize>) -> Option<usize> {
    lengths
        .into_iter()
        .try_fold(initial, usize::checked_add)
        .filter(|&length| length <= isize::MAX as usize)
}

/// Returns a retained owner when its bytes make up the entire result.
///
/// # Safety
/// All fragments obey the rooted handle and borrowed UTF-8 span contracts.
unsafe fn concat_size(initial: usize, parts: &[StringPart]) -> Option<(usize, Option<StrOwner>)> {
    let mut handle = None;
    let length = concatenated_length(
        initial,
        parts.iter().map(|part| {
            // SAFETY: the caller roots every handle and byte span through the call.
            let size = unsafe { part.as_str() }.len();
            if initial == 0 && size != 0 && part.length == usize::MAX {
                handle = Some((part.data, size));
            }
            size
        }),
    )?;
    let shared = handle.filter(|&(_, size)| size == length).map(|(data, _)| {
        // SAFETY: the sentinel selected a live Str handle which the caller roots.
        unsafe { crate::StrOwner::acquire_word(data as usize) }
    });
    Some((length, shared))
}

/// Builds a complete concatenation before publishing it or accessing a writer.
///
/// # Safety
/// The descriptors, handles, and spans obey `imre_str_concat_parts`'s contract.
unsafe fn concat_parts(parts: *const StringPart, count: usize) -> Result<StrOwner, Error> {
    let parts = if count == 0 {
        &[]
    } else {
        // SAFETY: the compiler supplies a readable descriptor array of this length.
        unsafe { std::slice::from_raw_parts(parts, count) }
    };
    // SAFETY: all descriptor sources remain rooted through this call.
    let (length, shared) = unsafe { concat_size(0, parts) }.ok_or_else(Error::allocation)?;
    if let Some(shared) = shared {
        return Ok(shared);
    }
    if length == 0 {
        return text::try_owned_string("");
    }
    let mut result = Str::try_with_capacity(length).map_err(|_| Error::allocation())?;
    {
        // SAFETY: the fresh result has capacity for the checked total. We fill
        // its spare bytes without reading or moving it, then publish length once.
        // At the inline capacity, the final byte also holds the tag: sequential
        // prefixes overwrite it only with the final byte of complete valid UTF-8.
        let output = unsafe { result.spare_capacity_mut() }
            .as_mut_ptr()
            .cast::<u8>();
        let mut offset = 0;
        for part in parts {
            // SAFETY: no immutable input has changed since the capacity pass.
            let text = unsafe { part.as_str() };
            // SAFETY: the checked total bounds every prefix, and the fresh result
            // cannot overlap any borrowed input. No uninitialized byte is read.
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), output.add(offset), text.len());
            }
            offset += text.len();
        }
    }
    // SAFETY: exactly `length` bytes were initialized within capacity; joining
    // complete UTF-8 fragments preserves UTF-8, including the full-inline case.
    unsafe { result.set_len(length) };
    StrOwner::try_new(result).map_err(|_| Error::allocation())
}

/// Python also treats the four ASCII information separators as whitespace.
pub(crate) fn python_whitespace(value: char) -> bool {
    value.is_whitespace() || matches!(value, '\u{001c}'..='\u{001f}')
}

/// Removes a trailing set of Unicode scalars, or Python whitespace for null.
///
/// # Safety
/// `value` roots a live string and `chars` is either another rooted string or
/// null. The returned string owns an independent reference, including no-ops.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_rstrip(value: *const Str, chars: *const Str) -> *const Str {
    // SAFETY: both immutable strings remain rooted throughout this call.
    let (source, chars) = unsafe { (&*value, chars.as_ref()) };
    let stripped = source.trim_end_matches(|character: char| {
        chars.map_or_else(
            || python_whitespace(character),
            |chars| chars.contains(character),
        )
    });
    if stripped.len() == source.len() {
        // SAFETY: acquiring an independent reference preserves the caller's owner.
        unsafe { StrOwner::increment_strong_count(value) };
        value
    } else {
        StrOwner::into_raw(StrOwner::new(Str::new(stripped)))
    }
}

/// Splits with Python's no-separator whitespace rules, omitting empty fields.
///
/// # Safety
/// `value` must be a rooted, non-null string handle. The returned Object is an
/// owned sequence of strings and must use the generic sequence ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_split_whitespace(value: *const Str) -> *const Object {
    // SAFETY: the caller roots the immutable string throughout the split.
    let value = unsafe { std::mem::ManuallyDrop::new(StrOwner::from_raw(value)) };
    let mut result = Words::new(3);
    for part in value
        .split(python_whitespace)
        .filter(|part| !part.is_empty())
    {
        text::push_part(&mut result, &value, part);
    }
    crate::ObjectOwner::into_raw(result.into_object())
}

/// Bytes in 0x20..=0x7f contain no Python line break. Reducing fixed-size
/// blocks lets LLVM check long ASCII lines without branching on each byte.
fn plain_ascii_line(bytes: &[u8]) -> bool {
    let (blocks, tail) = bytes.as_chunks::<32>();
    blocks.iter().all(|block| {
        block
            .iter()
            .fold(0_u8, |bits, &byte| bits | byte | byte.wrapping_sub(32))
            & 0x80
            == 0
    }) && tail.iter().all(|byte| (32..128).contains(byte))
}

/// Python line offsets computed once per delimiter, with the common physical
/// ASCII-line and ordinary CR/LF inputs staying on byte-oriented paths.
struct LineOffsets {
    start: usize,
    boundaries: Boundaries,
}

enum Boundaries {
    Single(usize),
    CrLf,
    Unicode,
}

impl LineOffsets {
    fn new(value: &str) -> Self {
        let single = value
            .strip_suffix("\r\n")
            .or_else(|| value.strip_suffix(['\r', '\n']))
            .unwrap_or(value);
        let bytes = value.as_bytes();
        let boundaries = if plain_ascii_line(single.as_bytes()) {
            Boundaries::Single(single.len())
        } else if memchr::memchr3(0x0b, 0x0c, 0x1c, bytes).is_none()
            && memchr::memchr2(0x1d, 0x1e, bytes).is_none()
            && !memchr::memchr2_iter(0xc2, 0xe2, bytes).any(|at| {
                matches!(
                    &bytes[at..],
                    [0xc2, 0x85, ..] | [0xe2, 0x80, 0xa8 | 0xa9, ..]
                )
            })
        {
            Boundaries::CrLf
        } else {
            Boundaries::Unicode
        };
        Self {
            start: 0,
            boundaries,
        }
    }

    fn next(&mut self, value: &str) -> Option<std::ops::Range<usize>> {
        let start = self.start;
        if start == value.len() {
            return None;
        }
        let remaining = &value[start..];
        let boundary = match self.boundaries {
            Boundaries::Single(end) => {
                self.start = value.len();
                return Some(start..end);
            }
            Boundaries::CrLf => {
                memchr::memchr2(b'\r', b'\n', remaining.as_bytes()).map(|at| (at, 1))
            }
            Boundaries::Unicode => remaining.char_indices().find_map(|(at, c)| {
                matches!(
                    c,
                    '\n' | '\r'
                        | '\x0b'
                        | '\x0c'
                        | '\x1c'
                        | '\x1d'
                        | '\x1e'
                        | '\u{85}'
                        | '\u{2028}'
                        | '\u{2029}'
                )
                .then_some((at, c.len_utf8()))
            }),
        };
        let Some((offset, length)) = boundary else {
            self.start = value.len();
            return Some(start..self.start);
        };
        let end = start + offset;
        self.start = end + length;
        if value.as_bytes()[end] == b'\r' && value.as_bytes().get(self.start) == Some(&b'\n') {
            self.start += 1;
        }
        Some(start..end)
    }
}

struct LineIterator {
    owner: StrOwner,
    offsets: LineOffsets,
}

fn owned_line(source: &StrOwner, range: std::ops::Range<usize>) -> Result<StrOwner, Error> {
    if range == (0..source.len()) {
        Ok(StrOwner::clone(source))
    } else {
        text::try_owned_string(&source[range])
    }
}

impl LineIterator {
    fn next(&mut self) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, Error> {
        let Some(range) = self.offsets.next(&self.owner) else {
            return Ok(None);
        };
        let line = owned_line(&self.owner, range)?;
        // SAFETY: the new string transfers one owner of the exact element kind.
        let line = unsafe {
            OwnedWord::from_owned(StrOwner::into_raw(line) as usize, words::WordKind::new(3))
        };
        Ok(Some((line, None)))
    }
}

/// Creates a line cursor for an immediately iterated split. The source is read
/// before the loop, and each step returns an independently owned Python string.
///
/// # Safety
/// The caller roots a live string handle, and releases the iterator on every exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_splitlines_iter(value: *const Str) -> *const Object {
    // SAFETY: acquire a strong reference while the caller still roots the input.
    let owner: StrOwner = unsafe { crate::StrOwner::acquire_word(value as usize) };
    let offsets = LineOffsets::new(&owner);
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::LineIterator(LineIterator { owner, offsets }),
    ))))
}

/// Splits Python line boundaries without retaining separators. CRLF is one
/// boundary; a final boundary creates no extra item, and U+001F is not a boundary.
///
/// # Safety
/// `value` must be rooted. Returns an owned string sequence or null on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_splitlines(value: *const Str) -> *const Object {
    // SAFETY: the receiver stays rooted through the default collector.
    unsafe { split_string_lines::<false>(value) }
}

/// Splits at the same boundaries as the no-argument and fused paths. The
/// caller truth-tests the flag before entering the collector.
///
/// # Safety
/// The immutable receiver stays rooted throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_splitlines_options(
    value: *const Str,
    keep: u32,
) -> *const Object {
    // SAFETY: both paths use the same rooted source and return a fresh list.
    unsafe {
        if keep == 0 {
            imre_str_splitlines(value)
        } else {
            split_string_lines::<true>(value)
        }
    }
}

/// Reuse the collector and its ownership logic for either boundary policy.
/// The caller roots `value` until the list owns every returned slice.
unsafe fn split_string_lines<const KEEP: bool>(value: *const Str) -> *const Object {
    // SAFETY: the caller roots the immutable string throughout the split.
    let value = unsafe { std::mem::ManuallyDrop::new(StrOwner::from_raw(value)) };
    if !KEEP {
        match deferred_diff::Deferred::split_lines(&value) {
            Ok(Some(lines)) => return lines,
            Ok(None) => {}
            Err(error) => return finish(Err(error), std::ptr::null()),
        }
    }
    finish(
        split_lines(&value, |mut range| {
            // The shared scanner has already found the boundary. Its first
            // scalar supplies the width; only CRLF has two scalars. This
            // branch disappears from the legacy no-argument specialization.
            if KEEP && let Some(ending) = value[range.end..].chars().next() {
                range.end += ending.len_utf8();
                if ending == '\r' && value.as_bytes().get(range.end) == Some(&b'\n') {
                    range.end += 1;
                }
            }
            owned_line(&value, range)
        }),
        std::ptr::null(),
    )
}

/// Visit line fields using the same scanner as native str.splitlines.
pub(crate) fn visit_line_fields<'a>(
    value: &'a str,
    keep_ends: bool,
    mut visit: impl FnMut(&'a str),
) {
    let mut offsets = LineOffsets::new(value);
    while let Some(mut range) = offsets.next(value) {
        if keep_ends {
            range.end = offsets.start;
        }
        visit(&value[range]);
    }
}

fn split_lines(
    value: &str,
    own: impl Fn(std::ops::Range<usize>) -> Result<StrOwner, Error>,
) -> Result<*const Object, Error> {
    let mut offsets = LineOffsets::new(value);
    let mut result = Words::new(3);
    while let Some(range) = offsets.next(value) {
        result
            .values
            .try_reserve(1)
            .map_err(|_| Error::allocation())?;
        result.values.push(StrOwner::into_raw(own(range)?) as usize);
    }
    Ok(crate::ObjectOwner::into_raw(result.into_object()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::Shared;

    #[test]
    fn partial_split_releases_earlier_string_owners() {
        let first = StrOwner::new(Str::new("first"));
        let result = split_lines("first\nsecond", |range| {
            if range.start == 0 {
                Ok(first.clone())
            } else {
                Err(Error::allocation())
            }
        });
        assert!(result.is_err());
        assert_eq!(StrOwner::strong_count(&first), 1);
    }

    #[test]
    fn concatenation_retains_the_only_nonempty_string_handle() {
        let text = StrOwner::new(Str::new("a heap-backed concatenation operand"));
        let empty = StrOwner::new(Str::new(""));
        let parts = [
            StringPart {
                data: StrOwner::as_ptr(&empty).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: StrOwner::as_ptr(&text).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: StrOwner::as_ptr(&empty).cast(),
                length: usize::MAX,
            },
        ];
        // SAFETY: both input owners root all three descriptors through each call.
        unsafe {
            for result in [
                imre_str_concat(StrOwner::as_ptr(&text), StrOwner::as_ptr(&empty)),
                imre_str_concat(StrOwner::as_ptr(&empty), StrOwner::as_ptr(&text)),
                imre_str_concat_parts(parts.as_ptr(), parts.len()),
            ] {
                assert!(StrOwner::ptr_eq(&StrOwner::from_raw(result), &text));
            }
        }
        assert_eq!(StrOwner::strong_count(&text), 1);

        let literal = "static bytes that exceed inline capacity";
        // SAFETY: the literal has static storage. Each call transfers one owner.
        let handle = unsafe { crate::imre_str_static_literal(literal.as_ptr(), literal.len()) };
        let part = StringPart {
            data: handle.cast(),
            length: usize::MAX,
        };
        let empty = StrOwner::into_raw(StrOwner::new(Str::default()));
        // SAFETY: the descriptor and its source remain live, and the empty owner transfers.
        let result = unsafe { StrOwner::from_raw(imre_str_concat_parts_owned(empty, &part, 1)) };
        // SAFETY: recover the constructor's original owner exactly once.
        let original = unsafe { StrOwner::from_raw(handle) };
        assert!(StrOwner::ptr_eq(&result, &original));
        assert_eq!(result.as_static_str(), Some(literal));
    }

    #[test]
    fn concatenation_initializes_inline_tag_and_heap_boundaries() {
        for (length, ending) in [
            (0, ""),
            (23, "é"),
            (24, "z"),
            (24, "\0"),
            (24, "🦀"),
            (25, "é"),
            (25, "🦀"),
        ] {
            let padding = "a".repeat(length - ending.len());
            let split = padding.len() / 2;
            let owned = StrOwner::new(Str::new(&padding[split..]));
            let parts = [
                StringPart {
                    data: padding.as_ptr(),
                    length: split,
                },
                StringPart {
                    data: std::ptr::null(),
                    length: 0,
                },
                StringPart {
                    data: StrOwner::as_ptr(&owned).cast(),
                    length: usize::MAX,
                },
                StringPart {
                    data: ending.as_ptr(),
                    length: ending.len(),
                },
                StringPart {
                    data: std::ptr::null(),
                    length: 0,
                },
            ];
            // SAFETY: the ASCII padding, complete UTF-8 ending, and owned handle
            // remain immutable; null descriptors have zero length.
            let Ok(mut result) = (unsafe { concat_parts(parts.as_ptr(), parts.len()) }) else {
                panic!("test fragments must fit");
            };
            let source = StrOwner::into_raw(StrOwner::new(Str::new(&padding[..split])));
            let suffix = &parts[1..];
            // SAFETY: source transfers its only owner, and the remaining suffix
            // fragments are disjoint. Empty trailing parts also exercise reading
            // descriptors after the last full-inline byte overwrites its tag.
            let mut reused = unsafe {
                StrOwner::from_raw(imre_str_concat_parts_owned(
                    source,
                    suffix.as_ptr(),
                    suffix.len(),
                ))
            };
            assert_eq!(StrOwner::as_ptr(&reused), source);
            drop(owned);
            let expected = format!("{padding}{ending}");
            assert_eq!(result.len(), length);
            assert_eq!(result.as_str(), expected);
            assert_eq!(result.is_heap_allocated(), length > 24);
            assert_eq!(reused.as_str(), expected);
            assert_eq!(reused.is_heap_allocated(), length > 24);
            StrOwner::make_mut(&mut result).push_str("!");
            StrOwner::get_mut(&mut reused).unwrap().push_str("!");
            assert_eq!(result.as_str(), format!("{expected}!"));
            assert_eq!(reused.as_str(), result.as_str());
        }
    }

    #[test]
    fn integer_parts_outlive_inputs_and_preserve_fallback_ownership() {
        use num_bigint::BigInt;
        use std::mem::MaybeUninit;

        for number in [
            BigInt::from(crate::MIN_SMALL),
            BigInt::from(crate::MAX_SMALL),
            BigInt::from(0),
            BigInt::from(-1),
            BigInt::from(-1001),
            BigInt::from(17),
            BigInt::from(crate::MIN_SMALL) - 1,
            BigInt::from(crate::MAX_SMALL) + 1,
            BigInt::from(1) << 400,
            -(BigInt::from(1) << 400_u32),
        ] {
            let expected = number.to_string();
            let integer = Int::new(number);
            let small = integer.as_small().is_some();
            let weak_integer = if small {
                None
            } else {
                // SAFETY: the integer owns this exact BigInt allocation. The
                // temporary acquired reference is released before formatting.
                let owner = unsafe { Shared::<BigInt>::acquire_word(integer.0) };
                Some(Shared::downgrade(&owner))
            };
            let mut buffer = [0xcc_u8; 22];
            let scratch = buffer[1..21].as_mut_ptr();
            let mut part = MaybeUninit::<StringPart>::uninit();
            // SAFETY: the integer is rooted, and these disjoint caller allocations
            // provide 20 writable bytes and one aligned descriptor.
            let owner = unsafe { imre_int_str_part(integer.0, scratch, part.as_mut_ptr()) };
            // SAFETY: both successful helper paths initialize the descriptor.
            let part = unsafe { part.assume_init() };
            assert_eq!(owner.is_null(), small);
            assert_eq!((buffer[0], buffer[21]), (0xcc, 0xcc));
            let owner = if small {
                assert_eq!(part.data, scratch.cast_const());
                assert_eq!(part.length, expected.len());
                assert_eq!(
                    std::str::from_utf8(&buffer[1..1 + part.length]).unwrap(),
                    expected
                );
                assert!(buffer[1 + part.length..21].iter().all(|byte| *byte == 0xcc));
                None
            } else {
                assert_eq!(part.data, owner.cast());
                assert_eq!(part.length, usize::MAX);
                assert!(buffer.iter().all(|byte| *byte == 0xcc));
                assert_eq!(weak_integer.as_ref().unwrap().strong_count(), 1);
                // SAFETY: the helper transfers exactly one non-null Str owner.
                Some(unsafe { StrOwner::from_raw(owner) })
            };
            let weak_string = owner.as_ref().map(StrOwner::downgrade);
            drop(integer);
            if let Some(weak) = weak_integer {
                assert!(weak.upgrade().is_none());
            }
            let prefix = "é\0[";
            let suffix = "]🦀";
            let parts = [
                StringPart {
                    data: prefix.as_ptr(),
                    length: prefix.len(),
                },
                part,
                StringPart {
                    data: suffix.as_ptr(),
                    length: suffix.len(),
                },
            ];
            // SAFETY: caller scratch, literals, and the optional fallback owner
            // remain immutable and live; the original integer is already gone.
            let result =
                unsafe { StrOwner::from_raw(imre_str_concat_parts(parts.as_ptr(), parts.len())) };
            drop(owner);
            if let Some(weak) = weak_string {
                assert!(weak.upgrade().is_none());
            }
            assert_eq!(result.as_str(), format!("{prefix}{expected}{suffix}"));
        }
    }

    #[test]
    fn indexed_string_parts_preserve_unicode_scratch_bounds_and_input_lifetimes() {
        use std::mem::MaybeUninit;

        for (position, expected) in ["a", "é", "\0", "e", "\u{301}", "🦀"]
            .into_iter()
            .enumerate()
        {
            for at in [position as i64, position as i64 - 6] {
                let source = StrOwner::new(Str::new("aé\0e\u{301}🦀"));
                let weak = StrOwner::downgrade(&source);
                let at = Int::small(at).unwrap();
                let mut buffer = [0xcc_u8; 6];
                let scratch = buffer[1..5].as_mut_ptr();
                let mut part = MaybeUninit::<StringPart>::uninit();
                // SAFETY: rooted inputs and disjoint outputs provide a four-byte
                // scratch span and one descriptor; status precedes all output reads.
                let status = unsafe {
                    imre_str_get_part(StrOwner::as_ptr(&source), at.0, scratch, part.as_mut_ptr())
                };
                assert_eq!(status, 1);
                assert_eq!(exceptions::imre_error_pending(), 0);
                // SAFETY: successful status initializes the entire descriptor.
                let part = unsafe { part.assume_init() };
                assert_eq!(part.data, scratch.cast_const());
                assert_eq!(part.length, expected.len());
                assert_eq!(buffer[0], 0xcc);
                assert!(
                    buffer[1 + expected.len()..]
                        .iter()
                        .all(|byte| *byte == 0xcc)
                );
                drop((source, at));
                assert!(weak.upgrade().is_none());
                // SAFETY: the descriptor borrows caller scratch, so neither
                // released input is needed to concatenate its initialized bytes.
                let result = unsafe { StrOwner::from_raw(imre_str_concat_parts(&part, 1)) };
                assert_eq!(result.as_str(), expected);
            }
        }
    }

    #[test]
    fn indexed_string_part_errors_preserve_outputs_and_match_string_get() {
        use num_bigint::BigInt;

        let bounds = "string index out of range";
        let overflow = "cannot fit 'int' into an index-sized integer";
        for (text, at, expected) in [
            ("aé\0e\u{301}🦀", BigInt::from(6), bounds),
            ("aé\0e\u{301}🦀", BigInt::from(-7), bounds),
            ("", BigInt::from(0), bounds),
            ("", BigInt::from(-1), bounds),
            ("a", BigInt::from(isize::MIN), bounds),
            ("a", BigInt::from(isize::MAX), bounds),
            ("a", BigInt::from(1) << 400_u32, overflow),
            ("a", -(BigInt::from(1) << 400_u32), overflow),
        ] {
            let source = StrOwner::new(Str::new(text));
            let at = Int::new(at);
            let mut buffer = [0xcc_u8; 6];
            let mut part = StringPart {
                data: "unchanged".as_ptr(),
                length: "unchanged".len(),
            };
            // SAFETY: all inputs and writable outputs satisfy the normal ABI,
            // despite the language-level index being outside the accepted range.
            let status = unsafe {
                imre_str_get_part(
                    StrOwner::as_ptr(&source),
                    at.0,
                    buffer[1..5].as_mut_ptr(),
                    &mut part,
                )
            };
            assert_eq!(status, 0);
            assert_eq!(buffer, [0xcc; 6]);
            assert_eq!(part.data, "unchanged".as_ptr());
            assert_eq!(part.length, "unchanged".len());
            assert_eq!(StrOwner::strong_count(&source), 1);
            assert_eq!(exceptions::imre_error_pending(), 1);
            // SAFETY: taking pending transfers one exception owner; retrieving
            // its message returns one independent string owner, both adopted once.
            unsafe {
                let error = crate::ObjectOwner::from_raw(exceptions::imre_error_take());
                assert_eq!(
                    exceptions::imre_exception_matches(
                        crate::ObjectOwner::as_ptr(&error),
                        Kind::IndexError as u32
                    ),
                    1,
                );
                let message = StrOwner::from_raw(exceptions::imre_exception_message(
                    crate::ObjectOwner::as_ptr(&error),
                ));
                assert_eq!(message.as_str(), expected);
            }
            assert_eq!(exceptions::imre_error_pending(), 0);
        }
    }

    #[test]
    fn concatenation_parts_borrow_aliases_and_preserve_utf8_and_nuls() {
        let source = StrOwner::new(Str::new("α\0🦀"));
        let alias = source.clone();
        let weak = StrOwner::downgrade(&source);
        let prefix = "literalé\0:";
        let suffix = ":a suffix exceeding inline string capacity";
        let parts = [
            StringPart {
                data: prefix.as_ptr(),
                length: prefix.len(),
            },
            StringPart {
                data: StrOwner::as_ptr(&source).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: std::ptr::null(),
                length: 0,
            },
            StringPart {
                data: StrOwner::as_ptr(&alias).cast(),
                length: usize::MAX,
            },
            StringPart {
                data: suffix.as_ptr(),
                length: suffix.len(),
            },
        ];
        // SAFETY: both aliases and all literal spans outlive the descriptor array's
        // use; the call returns one independently owned concatenated string.
        let result =
            unsafe { StrOwner::from_raw(imre_str_concat_parts(parts.as_ptr(), parts.len())) };
        assert_eq!(StrOwner::strong_count(&source), 2);
        assert_eq!(source.as_str(), "α\0🦀");
        drop((source, alias));
        assert!(weak.upgrade().is_none());
        assert_eq!(
            result.as_str(),
            "literalé\0:α\0🦀α\0🦀:a suffix exceeding inline string capacity"
        );
        // SAFETY: zero count permits a null array, and empty literal spans permit
        // null data; each returned reference is adopted and released once.
        unsafe {
            assert!(StrOwner::from_raw(imre_str_concat_parts(std::ptr::null(), 0)).is_empty());
            assert!(StrOwner::from_raw(imre_str_concat_parts(&parts[2], 1)).is_empty());
        }
    }

    #[test]
    fn owned_concatenation_reuses_unique_owner_and_reserved_heap_storage() {
        let mut text = Str::with_capacity(256);
        text.push_str("a prefix exceeding inline string capacity");
        let source = StrOwner::new(text);
        let storage = source.as_ptr();
        let capacity = source.capacity();
        let suffix = StrOwner::new(Str::new("🦀"));
        let literal = "é\0";
        let parts = [
            StringPart {
                data: literal.as_ptr(),
                length: literal.len(),
            },
            StringPart {
                data: StrOwner::as_ptr(&suffix).cast(),
                length: usize::MAX,
            },
        ];
        let source = StrOwner::into_raw(source);
        // SAFETY: source transfers its only owner; suffix and literal are disjoint
        // rooted inputs, and the returned owner is adopted exactly once.
        let result = unsafe {
            StrOwner::from_raw(imre_str_concat_parts_owned(
                source,
                parts.as_ptr(),
                parts.len(),
            ))
        };
        assert_eq!(StrOwner::as_ptr(&result), source);
        assert_eq!(result.as_ptr(), storage);
        assert_eq!(result.capacity(), capacity);
        assert_eq!(StrOwner::strong_count(&result), 1);
        assert_eq!(StrOwner::strong_count(&suffix), 1);
        assert_eq!(
            result.as_str(),
            "a prefix exceeding inline string capacityé\0🦀"
        );
    }

    #[test]
    fn owned_concatenation_grows_geometrically_across_inline_and_heap_storage() {
        let mut source = StrOwner::new(Str::new(""));
        let original = StrOwner::as_ptr(&source);
        let literal = "é\0";
        let part = StringPart {
            data: literal.as_ptr(),
            length: literal.len(),
        };
        let mut growths = 0;
        for length in (3..=3072).step_by(3) {
            let capacity = source.capacity();
            // SAFETY: this disjoint literal outlives every call, which consumes
            // the sole owner and returns exactly one replacement owner.
            source = unsafe {
                StrOwner::from_raw(imre_str_concat_parts_owned(
                    StrOwner::into_raw(source),
                    &part,
                    1,
                ))
            };
            assert_eq!(StrOwner::as_ptr(&source), original);
            assert_eq!(source.len(), length);
            assert_eq!(source.is_heap_allocated(), length > 24);
            growths += usize::from(source.capacity() != capacity);
        }
        assert!(
            growths < 32,
            "capacity grew {growths} times for 1024 appends"
        );
        assert_eq!(source.as_str(), "é\0".repeat(1024));
    }

    #[test]
    fn refused_shared_append_owner_restores_the_original_handle() {
        let alias = StrOwner::new(Str::new("shared"));
        let original = StrOwner::into_raw(alias.clone());
        // SAFETY: the call consumes the slot's owner and restores it on failure.
        let result = finish_shared_append(
            unsafe { StrOwner::from_raw(original) },
            Err(allocator_api2::alloc::AllocError),
        );
        assert!(result.is_null());
        assert_eq!(exceptions::imre_error_pending(), 1);
        // SAFETY: the slot still owns one reference and take transfers the exception.
        unsafe {
            let restored = StrOwner::from_raw(original);
            assert!(StrOwner::ptr_eq(&restored, &alias));
            imre_object_release(exceptions::imre_error_take());
        }
        assert_eq!(StrOwner::strong_count(&alias), 1);
    }

    #[test]
    fn failed_append_restores_the_callers_reference() {
        let original = StrOwner::new(Str::from("preserved"));
        let alias = StrOwner::clone(&original);
        let pointer = StrOwner::as_ptr(&original);
        assert!(failed_append(original).is_null());
        assert_eq!(StrOwner::strong_count(&alias), 2);

        // SAFETY: failure returned the transferred reference to its original
        // pointer, and taking the pending exception transfers one owner.
        unsafe {
            let restored = StrOwner::from_raw(pointer);
            assert!(StrOwner::ptr_eq(&restored, &alias));
            assert_eq!(restored.as_str(), "preserved");
            let error = crate::ObjectOwner::from_raw(exceptions::imre_error_take());
            assert_eq!(
                exceptions::imre_exception_matches(
                    crate::ObjectOwner::as_ptr(&error),
                    Kind::MemoryError as u32
                ),
                1
            );
        }
    }

    #[test]
    fn owned_concatenation_preserves_shared_alias_and_consumes_one_owner() {
        for count in [1, 2] {
            let alias = StrOwner::new(Str::new("α\0🦀"));
            let weak = StrOwner::downgrade(&alias);
            let source = StrOwner::into_raw(alias.clone());
            let parts = [
                StringPart {
                    data: ":".as_ptr(),
                    length: 1,
                },
                StringPart {
                    data: source.cast(),
                    length: usize::MAX,
                },
            ];
            // SAFETY: source transfers one of the two references, and alias roots
            // any self-borrowed suffix. Disjoint-only suffixes must also copy.
            let result = unsafe {
                StrOwner::from_raw(imre_str_concat_parts_owned(source, parts.as_ptr(), count))
            };
            assert_ne!(StrOwner::as_ptr(&result), source);
            assert_eq!(StrOwner::strong_count(&alias), 1);
            assert_eq!(alias.as_str(), "α\0🦀");
            drop(alias);
            assert!(weak.upgrade().is_none());
            assert_eq!(
                result.as_str(),
                if count == 1 {
                    "α\0🦀:"
                } else {
                    "α\0🦀:α\0🦀"
                }
            );
        }
    }

    #[test]
    fn owned_concatenation_roots_unowned_self_handles_and_byte_spans() {
        for text in ["α\0🦀", "α\0🦀 with a suffix exceeding inline capacity"] {
            for handle in [false, true] {
                let source = StrOwner::new(Str::new(text));
                let part = if handle {
                    StringPart {
                        data: StrOwner::as_ptr(&source).cast(),
                        length: usize::MAX,
                    }
                } else {
                    StringPart {
                        data: source.as_ptr(),
                        length: source.len(),
                    }
                };
                let source = StrOwner::into_raw(source);
                // SAFETY: the consumed sole owner roots its directly borrowed
                // handle/span. No extra Rc or Weak masks the aliasing case.
                let result =
                    unsafe { StrOwner::from_raw(imre_str_concat_parts_owned(source, &part, 1)) };
                assert_ne!(StrOwner::as_ptr(&result), source);
                assert_eq!(result.as_str(), text.repeat(2));
            }
        }

        let mut text = Str::new("a".repeat(64));
        let storage = text.as_ptr();
        text.truncate(8);
        assert_eq!(text.as_ptr(), storage);
        let source = StrOwner::into_raw(StrOwner::new(text));
        let part = StringPart {
            // SAFETY: truncation leaves this initialized heap allocation in place.
            data: unsafe { storage.add(32) },
            length: 16,
        };
        // SAFETY: the source owner roots initialized UTF-8 bytes in spare capacity.
        let result = unsafe { StrOwner::from_raw(imre_str_concat_parts_owned(source, &part, 1)) };
        assert_ne!(StrOwner::as_ptr(&result), source);
        assert_eq!(result.as_str(), "a".repeat(24));
    }

    #[test]
    fn owned_empty_suffix_preserves_handle_and_shared_ownership() {
        let alias = StrOwner::new(Str::new("unchanged"));
        for count in [0, 1] {
            let empty = StringPart {
                data: std::ptr::null(),
                length: 0,
            };
            let parts = if count == 0 { std::ptr::null() } else { &empty };
            let source = StrOwner::into_raw(alias.clone());
            // SAFETY: a null array is allowed only for zero count; a zero-length
            // byte span may be null. Exactly one source owner is transferred.
            let result =
                unsafe { StrOwner::from_raw(imre_str_concat_parts_owned(source, parts, count)) };
            assert_eq!(StrOwner::as_ptr(&result), source);
            assert_eq!(StrOwner::strong_count(&alias), 2);
            assert_eq!(result.as_str(), "unchanged");
        }
        assert_eq!(StrOwner::strong_count(&alias), 1);
    }

    #[test]
    fn owned_utf8_append_reuses_unique_owner_and_reserved_heap_storage() {
        let mut text = Str::with_capacity(256);
        text.push_str("a prefix exceeding inline string capacity");
        let source = StrOwner::new(text);
        let storage = source.as_ptr();
        let capacity = source.capacity();
        let source = StrOwner::into_raw(source);
        let suffix = "é\0🦀";
        // SAFETY: the literal is disjoint, and the only source owner is consumed.
        let result = unsafe {
            StrOwner::from_raw(imre_str_append_utf8_owned(
                source,
                suffix.as_ptr(),
                suffix.len(),
            ))
        };
        assert_eq!(StrOwner::as_ptr(&result), source);
        assert_eq!(result.as_ptr(), storage);
        assert_eq!(result.capacity(), capacity);
        assert_eq!(StrOwner::strong_count(&result), 1);
        assert_eq!(
            result.as_str(),
            "a prefix exceeding inline string capacityé\0🦀"
        );
    }

    #[test]
    fn owned_utf8_append_preserves_shared_and_weak_aliases() {
        let heap_prefix = "a shared prefix exceeding inline string capacity α\0🦀";
        for (text, shared) in [
            ("α\0🦀", false),
            ("α\0🦀", true),
            (heap_prefix, false),
            (heap_prefix, true),
        ] {
            let source = StrOwner::new(Str::new(text));
            let alias = shared.then(|| source.clone());
            let weak = StrOwner::downgrade(&source);
            let source = StrOwner::into_raw(source);
            let suffix = "é!";
            // SAFETY: the literal is disjoint and exactly one owner is transferred.
            let result = unsafe {
                StrOwner::from_raw(imre_str_append_utf8_owned(
                    source,
                    suffix.as_ptr(),
                    suffix.len(),
                ))
            };
            assert_ne!(StrOwner::as_ptr(&result), source);
            assert_eq!(StrOwner::strong_count(&result), 1);
            if let Some(alias) = alias {
                assert_eq!(StrOwner::strong_count(&alias), 1);
                assert_eq!(alias.as_str(), text);
                drop(alias);
            }
            assert!(weak.upgrade().is_none());
            assert_eq!(result.as_str(), format!("{text}é!"));
        }
    }

    #[test]
    fn owned_utf8_append_initializes_inline_tag_and_heap_boundaries() {
        for (length, ending) in [
            (0, ""),
            (23, "é"),
            (24, "z"),
            (24, "\0"),
            (24, "é"),
            (24, "🦀"),
            (25, "é"),
            (25, "🦀"),
        ] {
            let padding = "a".repeat(length - ending.len());
            let source = StrOwner::into_raw(StrOwner::new(Str::new(&padding)));
            // SAFETY: the complete literal suffix is disjoint, including when it
            // fills the final inline tag byte. Exactly one source owner moves.
            let mut result = unsafe {
                StrOwner::from_raw(imre_str_append_utf8_owned(
                    source,
                    ending.as_ptr(),
                    ending.len(),
                ))
            };
            assert_eq!(StrOwner::as_ptr(&result), source);
            assert_eq!(result.as_str(), format!("{padding}{ending}"));
            assert_eq!(result.len(), length);
            assert_eq!(result.is_heap_allocated(), length > 24);
            StrOwner::get_mut(&mut result).unwrap().push_str("!");
            assert_eq!(result.as_str(), format!("{padding}{ending}!"));
        }

        let literal = "a static prefix exceeding inline string capacity";
        let source = StrOwner::new(Str::const_new(literal));
        assert_eq!(source.as_ptr(), literal.as_ptr());
        assert!(!source.is_heap_allocated());
        let source = StrOwner::into_raw(source);
        let suffix = "é\0🦀";
        // SAFETY: both static spans are disjoint, and the only source owner is
        // transferred. Mutation must first copy the read-only static storage.
        let result = unsafe {
            StrOwner::from_raw(imre_str_append_utf8_owned(
                source,
                suffix.as_ptr(),
                suffix.len(),
            ))
        };
        assert_eq!(StrOwner::as_ptr(&result), source);
        assert_ne!(result.as_ptr(), literal.as_ptr());
        assert!(result.is_heap_allocated());
        assert_eq!(result.as_str(), format!("{literal}{suffix}"));
    }

    #[test]
    fn owned_utf8_empty_append_preserves_shared_owner_without_reading_null() {
        let alias = StrOwner::new(Str::new("unchanged α\0🦀"));
        let source = StrOwner::into_raw(alias.clone());
        // SAFETY: an empty suffix may be null; one of two owners is transferred.
        let result =
            unsafe { StrOwner::from_raw(imre_str_append_utf8_owned(source, std::ptr::null(), 0)) };
        assert_eq!(StrOwner::as_ptr(&result), source);
        assert_eq!(StrOwner::strong_count(&alias), 2);
        assert_eq!(result.as_str(), "unchanged α\0🦀");
        drop(result);
        assert_eq!(StrOwner::strong_count(&alias), 1);
    }

    #[test]
    fn concatenation_length_rejects_allocation_and_arithmetic_overflow() {
        assert_eq!(concatenated_length(0, []), Some(0));
        assert_eq!(concatenated_length(20, [1, 2]), Some(23));
        assert_eq!(
            concatenated_length(isize::MAX as usize, [0]),
            Some(isize::MAX as usize)
        );
        assert_eq!(concatenated_length(isize::MAX as usize, [1]), None);
        assert_eq!(concatenated_length(0, [isize::MAX as usize, 1]), None);
        assert_eq!(concatenated_length(usize::MAX, [1]), None);
    }

    #[test]
    fn sequence_words_preserve_niches_and_owned_snapshots() {
        let text = StrOwner::new(Str::from("optional payload"));
        let weak = StrOwner::downgrade(&text);
        let word = StrOwner::as_ptr(&text) as usize;
        // SAFETY: exact sequence kinds describe every scalar or pointer word;
        // nullable strings use null directly without any wrapper allocation.
        unsafe {
            let optional = imre_seq_new(259);
            imre_seq_append(optional, 0, 259);
            imre_seq_append(optional, word, 259);
            drop(text);
            imre_seq_set_index(optional, 1, word, 259);
            let saved = StrOwner::from_raw(imre_seq_get_index(optional, 1) as *const Str);
            imre_seq_set_index(optional, 1, 0, 259);
            assert_eq!(imre_seq_pop(optional), 0);
            assert_eq!(imre_seq_get_index(optional, 0), 0);
            imre_object_release(optional);
            assert_eq!(saved.as_str(), "optional payload");
            drop(saved);

            for (kind, values) in [
                (257, vec![0, 1, 2]),
                (258, vec![0, 1, Int::small(-1).unwrap().0]),
            ] {
                let sequence = imre_seq_new(kind);
                for &value in &values {
                    imre_seq_append(sequence, value, kind);
                }
                let empty = imre_seq_repeat(sequence, Int::small(-1).unwrap().0);
                assert_eq!(Int(imre_seq_len(empty)).as_small(), Some(0));
                assert_eq!(
                    object(empty).0.with(|s| s
                        .sequence()
                        .ok()
                        .expect("fixture sequence")
                        .kind
                        .raw()),
                    kind
                );
                imre_object_release(empty);
                for (index, &value) in values.iter().enumerate() {
                    assert_eq!(imre_seq_get_index(sequence, index), value);
                }
                imre_object_release(sequence);
            }
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn proven_sequence_reads_borrow_exact_scalar_and_optional_words() {
        use num_bigint::BigInt;

        let text = StrOwner::new(Str::from("borrowed sequence word"));
        let big = Shared::new(BigInt::from(1_u128 << 100));
        let float = Rc::new(-0.0_f64);
        let text_weak = StrOwner::downgrade(&text);
        let big_weak = Shared::downgrade(&big);
        let float_weak = Rc::downgrade(&float);
        let cases = [
            (0, vec![0]),
            (1, vec![0, 1]),
            (257, vec![0, 1, 2]),
            (258, vec![0, 1, Shared::as_ptr(&big) as usize]),
            (259, vec![0, StrOwner::as_ptr(&text) as usize]),
            (263, vec![0, Rc::as_ptr(&float) as usize]),
            (
                7,
                vec![
                    0,
                    (-0.0_f64).to_bits() as usize,
                    f64::NAN.to_bits() as usize,
                ],
            ),
        ];
        for (kind, words) in cases {
            // SAFETY: each descriptor matches its rooted values. All accesses
            // are in bounds and their borrowed words expire before mutation.
            unsafe {
                let sequence = imre_seq_new(kind);
                for &word in &words {
                    imre_seq_append(sequence, word, kind);
                }
                let view = imre_seq_data_readonly(sequence);
                for (index, &word) in words.iter().enumerate() {
                    assert_eq!(imre_seq_borrow_index_proven(sequence, index), word);
                    assert_eq!(view.add(index).read(), word);
                }
                imre_object_release(sequence);
            }
        }
        assert_eq!(StrOwner::strong_count(&text), 1);
        assert_eq!(Shared::strong_count(&big), 1);
        assert_eq!(Rc::strong_count(&float), 1);
        drop((text, big, float));
        assert!(text_weak.upgrade().is_none());
        assert!(big_weak.upgrade().is_none());
        assert!(float_weak.upgrade().is_none());
        assert_eq!(exceptions::imre_error_pending(), 0);
    }

    #[test]
    fn readonly_sequence_views_keep_empty_and_owned_element_lifetimes() {
        let text = StrOwner::new(Str::from("rooted until the loop finishes"));
        let weak = StrOwner::downgrade(&text);
        // SAFETY: views are never read out of bounds and their last uses precede
        // any storage mutation or owner release. Acquired words outlive the view.
        unsafe {
            let sequence = imre_seq_new(3);
            let empty = imre_seq_data_readonly(sequence);
            assert!(!empty.is_null());
            assert_eq!(empty as usize % std::mem::align_of::<usize>(), 0);
            assert_eq!(imre_seq_len(sequence), 1); // Tagged zero; no load.
            imre_seq_append(sequence, StrOwner::as_ptr(&text) as usize, 3);
            let view = imre_seq_data_readonly(sequence);
            assert_eq!(StrOwner::strong_count(&text), 2);
            let word = view.read();
            let saved = crate::StrOwner::acquire_word(word);
            assert_eq!(StrOwner::strong_count(&text), 3);
            imre_object_release(sequence);
            drop(text);
            assert_eq!(saved.as_str(), "rooted until the loop finishes");
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(exceptions::imre_error_pending(), 0);
    }

    #[test]
    fn proven_sequence_record_is_acquired_before_its_source_owner_is_removed() {
        use records::{RecordDescriptor, imre_record_new, imre_record_release, imre_record_retain};

        let kinds = [3];
        let descriptor = RecordDescriptor {
            length: 1,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let text = StrOwner::new(Str::from("retained before mutation"));
        let weak = StrOwner::downgrade(&text);
        // SAFETY: immutable metadata outlives the record. The borrowed read is
        // immediately acquired, matching compiler lowering, before removing
        // the source owner. Every independent record owner is released once.
        unsafe {
            let record = imre_record_new(&descriptor);
            record.cast::<*const Str>().write(StrOwner::into_raw(text));
            let sequence = imre_seq_new(6);
            imre_seq_append(sequence, record as usize, 6);
            imre_record_release(record);
            let saved = imre_seq_borrow_index_proven(sequence, 0) as *const usize;
            imre_record_retain(saved);
            imre_record_release(imre_seq_pop(sequence) as *const usize);
            imre_object_release(sequence);
            assert_eq!(
                (&*saved.cast::<*const Str>().read()).as_str(),
                "retained before mutation"
            );
            imre_record_release(saved);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(exceptions::imre_error_pending(), 0);
    }

    #[test]
    fn repeated_record_sequences_share_mutations_and_transfer_pop_owners() {
        use records::{RecordDescriptor, imre_record_new, imre_record_release};
        let kinds = [2, 3];
        let descriptor = RecordDescriptor {
            length: 2,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        let text = StrOwner::new(Str::from("nested record root"));
        let weak = StrOwner::downgrade(&text);
        // SAFETY: metadata outlives all records and each copied record reference
        // is independently rooted until released; field writes transfer ownership.
        unsafe {
            let child = imre_record_new(&descriptor);
            child.write(Int::small(4).unwrap().into_raw());
            child
                .add(1)
                .cast::<*const Str>()
                .write(StrOwner::into_raw(text));
            let original = imre_seq_new(6);
            imre_seq_append(original, child as usize, 6);
            imre_record_release(child);
            let repeated = imre_seq_repeat(original, Int::small(2).unwrap().0);
            let popped = imre_seq_pop(original) as *mut usize;
            imre_object_release(original);
            popped.write(Int::small(9).unwrap().into_raw());
            let alias = imre_seq_get(repeated, Int::small(-1).unwrap().0) as *const usize;
            assert_eq!(Int(alias.read()).as_small(), Some(9));
            imre_object_release(repeated);
            imre_record_release(popped);
            assert!(weak.upgrade().is_some());
            imre_record_release(alias);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn optional_bigint_sequence_replacement_releases_only_displaced_owners() {
        use num_bigint::BigInt;
        let integer = Shared::new(BigInt::from(1_u128 << 120));
        let weak = Shared::downgrade(&integer);
        let word = Shared::as_ptr(&integer) as usize;
        // SAFETY: nullable Int word0 and canonical nonzero words are distinct;
        // every returned BigInt owner is adopted exactly once.
        unsafe {
            let values = imre_seq_new(258);
            imre_seq_append(values, word, 258);
            drop(integer);
            imre_seq_set_index(values, 0, word, 258);
            let saved = Int(imre_seq_get_index(values, 0));
            imre_seq_set_index(values, 0, 0, 258);
            imre_object_release(values);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u128 << 120));
            drop(saved);
        }
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn one_complete_line_retains_the_input_in_both_split_paths() {
        let source = StrOwner::new(Str::from("line".repeat(1024)));
        let mut cursor = LineIterator {
            offsets: LineOffsets::new(&source),
            owner: StrOwner::clone(&source),
        };
        let Ok(Some((word, _))) = cursor.next() else {
            panic!("expected an owned line");
        };
        // SAFETY: the iterator transfers a Str owner, and source roots the eager split.
        unsafe {
            let streamed = StrOwner::from_raw(word.into_word() as *const Str);
            let parts = imre_str_splitlines(StrOwner::as_ptr(&source));
            let listed =
                StrOwner::from_raw(imre_seq_get(parts, Int::small(0).unwrap().0) as *const Str);
            imre_object_release(parts);
            assert!(StrOwner::ptr_eq(&source, &listed));
            assert!(StrOwner::ptr_eq(&source, &streamed));
        }
    }

    #[test]
    fn splitlines_distinguishes_every_python_boundary_and_retains_interior_empty_lines() {
        for (source, expected) in [
            ("", vec![]),
            ("\n", vec![""]),
            ("\r\n\r\n", vec!["", ""]),
            (
                "a\nb\rc\r\nd\x0be\x0cf\x1cg\x1dh\x1ei\u{85}j\u{2028}k\u{2029}",
                vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"],
            ),
            ("é\x1f🦀\nlast", vec!["é\x1f🦀", "last"]),
        ] {
            let source = StrOwner::new(Str::new(source));
            // SAFETY: the source is rooted; the returned sequence transfers one
            // owner, and inspection copies strings without retaining storage refs.
            let actual = unsafe {
                let result =
                    crate::ObjectOwner::from_raw(imre_str_splitlines(StrOwner::as_ptr(&source)));
                result.0.with(|storage| {
                    storage
                        .sequence()
                        .ok()
                        .expect("fixture sequence")
                        .values
                        .iter()
                        .map(|&word| (&*(word as *const Str)).to_string())
                        .collect::<Vec<_>>()
                })
            };
            assert_eq!(actual, expected);
            // SAFETY: the cursor retains its input; output strings are owned
            // even after advancing the cursor, releasing it and dropping the input.
            let lines = unsafe {
                let cursor = imre_str_splitlines_iter(StrOwner::as_ptr(&source));
                drop(source);
                let mut result = Vec::new();
                let mut word = 0;
                while mappings::imre_mapping_iter_next(cursor, &mut word, std::ptr::null_mut()) != 0
                {
                    result.push(StrOwner::from_raw(word as *const Str));
                }
                imre_object_release(cursor);
                result
            };
            assert_eq!(
                lines.iter().map(|line| line.as_str()).collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn repeated_short_split_fields_share_independent_owners() {
        for value in ["", "a", "\0", "\u{80}", "ÿ"] {
            let left = text::owned_string(value);
            let right = text::owned_string(value);
            assert!(StrOwner::ptr_eq(&left, &right));
            drop(left);
            assert_eq!(right.as_str(), value);
        }
        for value in ["aa", "Ā"] {
            assert!(!StrOwner::ptr_eq(
                &text::owned_string(value),
                &text::owned_string(value)
            ));
        }
    }

    #[test]
    fn whitespace_split_retains_a_complete_field() {
        let source = StrOwner::new(Str::from("word".repeat(1024)));
        // SAFETY: source roots the call, and both ABI results transfer one owner.
        unsafe {
            let parts = imre_str_split_whitespace(StrOwner::as_ptr(&source));
            let word =
                StrOwner::from_raw(imre_seq_get(parts, Int::small(0).unwrap().0) as *const Str);
            imre_object_release(parts);
            assert!(StrOwner::ptr_eq(&word, &source));
            drop(source);
            assert_eq!(word.len(), 4096);
        }
    }

    #[test]
    fn splitting_owns_strings_and_matches_python_whitespace_boundaries() {
        // The complete whitespace set in CPython 3.14's _PyUnicode_IsWhitespace.
        let whitespace = "\u{9}\u{a}\u{b}\u{c}\u{d}\u{1c}\u{1d}\u{1e}\u{1f}\u{20}\u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
        let text = StrOwner::new(Str::from(format!(
            "{whitespace}α{whitespace}β\u{200b}γ{whitespace}"
        )));
        let zero = Int::small(0).unwrap();
        let one = Int::small(1).unwrap();
        // SAFETY: the input is rooted; split returns an owned sequence of string references.
        unsafe {
            let parts = imre_str_split_whitespace(StrOwner::as_ptr(&text));
            drop(text);
            assert_eq!(Int(imre_seq_len(parts)).as_small(), Some(2));
            let first = StrOwner::from_raw(imre_seq_get(parts, zero.0) as *const Str);
            let second = StrOwner::from_raw(imre_seq_get(parts, one.0) as *const Str);
            imre_object_release(parts);
            assert_eq!(first.as_str(), "α");
            assert_eq!(second.as_str(), "β\u{200b}γ");
            let joined = StrOwner::from_raw(imre_str_concat(
                StrOwner::as_ptr(&first),
                StrOwner::as_ptr(&second),
            ));
            drop((first, second));
            assert_eq!(joined.as_str(), "αβ\u{200b}γ");
        }
    }
}
