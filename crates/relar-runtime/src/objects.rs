//! Owned slots for fixed records, tuples, optional boxes, and richer lists.
//!
//! The compiler supplies each slot's representation kind: 0 = None (word 0),
//! 1 = bool (word 0 or 1), 2 = tagged Int, 3 = String pointer, 4 = ListInt
//! pointer, and 5 = Object pointer. Only kind 5 permits a null pointer, denoting
//! an absent optional value. All non-null pointers must be live handles of the
//! specified kind. The compiler enforces nominal types, homogeneous sequence
//! elements, tuple immutability, and acyclic heap type graphs.
//!
//! Arguments borrow their handles. Reads and pops return one owned reference;
//! writes acquire a reference before replacing an existing owner. Fixed objects
//! and sequences share an opaque allocation type, but their operations check the
//! storage shape. No buffer pointer escapes, and no runtime operation calls user
//! code. A mutable storage borrow ends before a displaced owner is dropped.

use std::cell::RefCell;
use std::rc::Rc;

use num_traits::ToPrimitive;

use super::{Int, List, fail, index};

/// An opaque allocation for either fixed fields or a resizable sequence.
pub struct Object(RefCell<Storage>);

enum Storage {
    Fields(Vec<Option<Slot>>),
    Sequence(Vec<Slot>),
}

/// Owns one value independently of the container from which it was acquired.
#[derive(Clone)]
enum Slot {
    None,
    Bool(bool),
    Int(Int),
    Str(Rc<String>),
    ListInt(Rc<List>),
    Object(Option<Rc<Object>>),
}

impl Slot {
    /// Acquires a borrowed ABI value whose representation was checked by the compiler.
    unsafe fn acquire(word: usize, kind: u32) -> Self {
        match kind {
            0 if word == 0 => Self::None,
            1 if word <= 1 => Self::Bool(word == 1),
            2 if word != 0 => {
                // SAFETY: the caller provides a rooted integer handle for kind 2.
                Self::Int(unsafe { (*Int::borrow(word)).clone() })
            }
            3 => {
                // SAFETY: the caller provides a rooted String handle for kind 3.
                Self::Str(unsafe { acquire_rc(word) })
            }
            4 => {
                // SAFETY: the caller provides a rooted ListInt handle for kind 4.
                Self::ListInt(unsafe { acquire_rc(word) })
            }
            5 if word == 0 => Self::Object(None),
            5 => {
                // SAFETY: the caller provides a rooted Object handle for kind 5.
                Self::Object(Some(unsafe { acquire_rc(word) }))
            }
            _ => fail("SystemError", "invalid runtime slot representation"),
        }
    }

    /// Transfers this slot's reference into the compiler's statically typed word.
    fn into_word(self) -> usize {
        match self {
            Self::None | Self::Object(None) => 0,
            Self::Bool(value) => usize::from(value),
            Self::Int(value) => value.into_raw(),
            Self::Str(value) => Rc::into_raw(value) as usize,
            Self::ListInt(value) => Rc::into_raw(value) as usize,
            Self::Object(Some(value)) => Rc::into_raw(value) as usize,
        }
    }
}

/// Acquires a reference without consuming the caller's live `Rc<T>` handle.
unsafe fn acquire_rc<T>(word: usize) -> Rc<T> {
    if word == 0 {
        fail("TypeError", "unexpected null runtime handle");
    }
    let pointer = word as *const T;
    // SAFETY: the caller supplies a live Rc<T>; the increment creates our reference.
    unsafe { Rc::increment_strong_count(pointer) };
    // SAFETY: this consumes only the reference acquired immediately above.
    unsafe { Rc::from_raw(pointer) }
}

/// Borrows a rooted handle and diagnoses absent optionals before dereferencing.
unsafe fn object<'a>(pointer: *const Object) -> &'a Object {
    if pointer.is_null() {
        fail("TypeError", "cannot access an absent optional value");
    }
    // SAFETY: the caller roots the non-null Object handle for the operation.
    unsafe { &*pointer }
}

impl Storage {
    fn fields(&self) -> &[Option<Slot>] {
        match self {
            Self::Fields(fields) => fields,
            Self::Sequence(_) => fail("SystemError", "expected a fixed-field object"),
        }
    }

    fn fields_mut(&mut self) -> &mut [Option<Slot>] {
        match self {
            Self::Fields(fields) => fields,
            Self::Sequence(_) => fail("SystemError", "expected a fixed-field object"),
        }
    }

    fn sequence(&self) -> &[Slot] {
        match self {
            Self::Sequence(values) => values,
            Self::Fields(_) => fail("SystemError", "expected a sequence object"),
        }
    }

    fn sequence_mut(&mut self) -> &mut Vec<Slot> {
        match self {
            Self::Sequence(values) => values,
            Self::Fields(_) => fail("SystemError", "expected a sequence object"),
        }
    }
}

/// Allocates fixed fields without inventing values for uninitialized slots.
#[unsafe(no_mangle)]
pub extern "C" fn relar_object_new(field_count: usize) -> *const Object {
    let mut fields = Vec::new();
    fields
        .try_reserve_exact(field_count)
        .unwrap_or_else(|_| fail("MemoryError", "object allocation failed"));
    fields.resize_with(field_count, || None);
    Rc::into_raw(Rc::new(Object(RefCell::new(Storage::Fields(fields)))))
}

/// # Safety
/// `value` must be a live Object handle or a null optional value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_retain(value: *const Object) {
    if !value.is_null() {
        // SAFETY: the caller owns a live Rc<Object> reference.
        unsafe { Rc::increment_strong_count(value) };
    }
}

/// # Safety
/// Consumes one Object reference, or accepts a null optional value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_release(value: *const Object) {
    if !value.is_null() {
        // SAFETY: the caller transfers one live Rc<Object> reference.
        unsafe { Rc::decrement_strong_count(value) };
    }
}

/// Returns one owned field value; its representation remains compiler-checked.
///
/// # Safety
/// `value` must be a rooted fixed object, and the caller must interpret the
/// returned word using the field's declared representation kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_get(value: *const Object, field: usize) -> usize {
    let owned = {
        // SAFETY: the caller roots the object throughout the field read.
        let storage = unsafe { object(value) }.0.borrow();
        storage
            .fields()
            .get(field)
            .unwrap_or_else(|| fail("IndexError", "object field index out of range"))
            .as_ref()
            .unwrap_or_else(|| fail("AttributeError", "object field is uninitialized"))
            .clone()
    };
    owned.into_word()
}

/// # Safety
/// `value` must be a rooted fixed object. `word` must be a borrowed value of
/// `kind`, matching the field's declared type; the write must preserve acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_set(
    value: *const Object,
    field: usize,
    word: usize,
    kind: u32,
) {
    // SAFETY: the compiler checks the incoming value and its representation kind.
    let new = unsafe { Slot::acquire(word, kind) };
    let old = {
        // SAFETY: the caller roots the object throughout the field replacement.
        let mut storage = unsafe { object(value) }.0.borrow_mut();
        storage
            .fields_mut()
            .get_mut(field)
            .unwrap_or_else(|| fail("IndexError", "object field index out of range"))
            .replace(new)
    };
    drop(old);
}

/// # Safety
/// `value` must be a rooted fixed-field object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_object_len(value: *const Object) -> usize {
    // SAFETY: the caller roots the object while its field count is read.
    Int::from_usize(unsafe { object(value) }.0.borrow().fields().len()).into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn relar_seq_new() -> *const Object {
    Rc::into_raw(Rc::new(Object(RefCell::new(Storage::Sequence(Vec::new())))))
}

/// # Safety
/// The arguments must be rooted sequence and integer handles. The caller must
/// interpret the returned owned word using the sequence's declared element type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_get(value: *const Object, at: usize) -> usize {
    let owned = {
        // SAFETY: the caller roots both handles throughout the element read.
        let (storage, at) = unsafe { (object(value).0.borrow(), Int::borrow(at)) };
        let values = storage.sequence();
        values[index(&at, values.len())].clone()
    };
    owned.into_word()
}

/// # Safety
/// The arguments must be rooted sequence and integer handles. `word` must have
/// representation `kind`, match the element type, and preserve heap acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_set(value: *const Object, at: usize, word: usize, kind: u32) {
    // SAFETY: the compiler checks the incoming element and representation kind.
    let new = unsafe { Slot::acquire(word, kind) };
    let old = {
        // SAFETY: the caller roots the sequence and index for this replacement.
        let (mut storage, at) = unsafe { (object(value).0.borrow_mut(), Int::borrow(at)) };
        let values = storage.sequence_mut();
        let at = index(&at, values.len());
        std::mem::replace(&mut values[at], new)
    };
    drop(old);
}

/// # Safety
/// `value` must be a rooted sequence. `word` must have representation `kind`,
/// match the sequence's element type, and preserve heap acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_append(value: *const Object, word: usize, kind: u32) {
    // SAFETY: the compiler checks the incoming element and representation kind.
    let new = unsafe { Slot::acquire(word, kind) };
    // SAFETY: the caller roots the sequence throughout the append.
    let mut storage = unsafe { object(value) }.0.borrow_mut();
    let values = storage.sequence_mut();
    values
        .try_reserve(1)
        .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
    values.push(new);
}

/// # Safety
/// `value` must be a rooted sequence; the returned owned word has its declared
/// element representation and must be released or transferred by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_pop(value: *const Object) -> usize {
    let owned = {
        // SAFETY: the caller roots the sequence throughout the removal.
        unsafe { object(value) }
            .0
            .borrow_mut()
            .sequence_mut()
            .pop()
            .unwrap_or_else(|| fail("IndexError", "pop from empty list"))
    };
    owned.into_word()
}

/// # Safety
/// `value` must be a rooted sequence object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_len(value: *const Object) -> usize {
    // SAFETY: the caller roots the sequence while its length is read.
    Int::from_usize(unsafe { object(value) }.0.borrow().sequence().len()).into_raw()
}

/// Repeats references to the same elements, preserving Python's shallow aliasing.
///
/// # Safety
/// The arguments must be rooted sequence and integer handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_seq_repeat(value: *const Object, count: usize) -> *const Object {
    // SAFETY: the caller roots the integer count for this operation.
    let count = unsafe { Int::borrow(count) };
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
        return relar_seq_new();
    }
    // SAFETY: the caller roots the sequence while references to its elements are acquired.
    let storage = unsafe { object(value) }.0.borrow();
    let values = storage.sequence();
    let count = count as usize;
    let length = values
        .len()
        .checked_mul(count)
        .unwrap_or_else(|| fail("MemoryError", "list is too large"));
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
    if !values.is_empty() {
        for _ in 0..count {
            result.extend(values.iter().cloned());
        }
    }
    Rc::into_raw(Rc::new(Object(RefCell::new(Storage::Sequence(result)))))
}

/// # Safety
/// Both arguments must be rooted, non-null immutable String handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_concat(a: *const String, b: *const String) -> *const String {
    // SAFETY: both immutable strings remain rooted for this call.
    let (a, b) = unsafe { (&*a, &*b) };
    let length = a
        .len()
        .checked_add(b.len())
        .unwrap_or_else(|| fail("MemoryError", "string is too large"));
    let mut result = String::new();
    result
        .try_reserve_exact(length)
        .unwrap_or_else(|_| fail("MemoryError", "string allocation failed"));
    result.push_str(a);
    result.push_str(b);
    Rc::into_raw(Rc::new(result))
}

/// Python also treats the four ASCII information separators as whitespace.
fn python_whitespace(value: char) -> bool {
    value.is_whitespace() || matches!(value, '\u{001c}'..='\u{001f}')
}

/// Splits with Python's no-separator whitespace rules, omitting empty fields.
///
/// # Safety
/// `value` must be a rooted, non-null String handle. The returned Object is an
/// owned sequence of strings and must use the generic sequence ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_str_split_whitespace(value: *const String) -> *const Object {
    // SAFETY: the caller roots the immutable string throughout the split.
    let value = unsafe { &*value };
    let mut result = Vec::new();
    for part in value
        .split(python_whitespace)
        .filter(|part| !part.is_empty())
    {
        result
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "sequence allocation failed"));
        result.push(Slot::Str(Rc::new(part.to_owned())));
    }
    Rc::into_raw(Rc::new(Object(RefCell::new(Storage::Sequence(result)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::BigInt;

    #[test]
    fn fixed_fields_preserve_every_heap_representation_after_the_owner_is_released() {
        let integer = Rc::new(BigInt::from(1_u128 << 100));
        let integer_weak = Rc::downgrade(&integer);
        let text = Rc::new(String::from("owned field"));
        let text_weak = Rc::downgrade(&text);
        // SAFETY: new returns one owned reference, transferred into each Rc.
        let (list, nested) = unsafe {
            (
                Rc::from_raw(super::super::relar_list_new()),
                Rc::from_raw(relar_object_new(0)),
            )
        };
        let list_weak = Rc::downgrade(&list);
        let nested_weak = Rc::downgrade(&nested);
        let record = relar_object_new(7);
        // SAFETY: all handles are rooted, field representations agree, and this graph is acyclic.
        unsafe {
            relar_object_set(record, 0, 0, 0);
            relar_object_set(record, 1, 1, 1);
            relar_object_set(record, 2, Rc::as_ptr(&integer) as usize, 2);
            relar_object_set(record, 3, Rc::as_ptr(&text) as usize, 3);
            relar_object_set(record, 4, Rc::as_ptr(&list) as usize, 4);
            relar_object_set(record, 5, Rc::as_ptr(&nested) as usize, 5);
            relar_object_set(record, 6, 0, 5);
            assert_eq!(relar_object_get(record, 0), 0);
            assert_eq!(relar_object_get(record, 1), 1);
            assert_eq!(relar_object_get(record, 6), 0);
            assert_eq!(Int(relar_object_len(record)).as_small(), Some(7));
            drop((integer, text, list, nested));

            let saved_integer = Int(relar_object_get(record, 2));
            let saved_text = Rc::from_raw(relar_object_get(record, 3) as *const String);
            let saved_list = Rc::from_raw(relar_object_get(record, 4) as *const List);
            let saved_nested = Rc::from_raw(relar_object_get(record, 5) as *const Object);
            relar_object_set(record, 2, 1, 2);
            relar_object_release(record);
            assert_eq!(saved_integer.big().as_ref(), &BigInt::from(1_u128 << 100));
            assert_eq!(saved_text.as_str(), "owned field");
            assert!(saved_list.0.borrow().is_empty());
            assert!(saved_nested.0.borrow().fields().is_empty());
            drop((saved_integer, saved_text, saved_list, saved_nested));
        }
        assert!(integer_weak.upgrade().is_none());
        assert!(text_weak.upgrade().is_none());
        assert!(list_weak.upgrade().is_none());
        assert!(nested_weak.upgrade().is_none());
    }

    #[test]
    fn repeated_sequences_share_elements_and_pop_transfers_ownership() {
        // SAFETY: the constructor returns an owned reference transferred into the Rc.
        let child = unsafe { Rc::from_raw(relar_object_new(1)) };
        let child_weak = Rc::downgrade(&child);
        let original = relar_seq_new();
        let zero = Int::small(0).unwrap();
        let two = Int::small(2).unwrap();
        let last = Int::small(-1).unwrap();
        // SAFETY: every handle is rooted, and sequences contain only acyclic record references.
        unsafe {
            relar_object_set(Rc::as_ptr(&child), 0, Int::small(4).unwrap().0, 2);
            relar_seq_append(original, Rc::as_ptr(&child) as usize, 5);
            let repeated = relar_seq_repeat(original, two.0);
            let popped = Rc::from_raw(relar_seq_pop(original) as *const Object);
            relar_object_release(original);
            drop(child);
            relar_object_set(Rc::as_ptr(&popped), 0, Int::small(9).unwrap().0, 2);
            let alias = Rc::from_raw(relar_seq_get(repeated, last.0) as *const Object);
            assert_eq!(
                Int(relar_object_get(Rc::as_ptr(&alias), 0)).as_small(),
                Some(9)
            );
            assert_eq!(Int(relar_seq_len(repeated)).as_small(), Some(2));
            let replacement = relar_object_new(1);
            relar_object_set(replacement, 0, Int::small(7).unwrap().0, 2);
            relar_seq_set(repeated, zero.0, replacement as usize, 5);
            relar_object_release(replacement);
            relar_object_release(repeated);
            assert_eq!(
                Int(relar_object_get(Rc::as_ptr(&popped), 0)).as_small(),
                Some(9)
            );
            drop((popped, alias));
        }
        assert!(child_weak.upgrade().is_none());
    }

    #[test]
    fn optional_boxes_keep_present_values_alive_and_allow_absent_slots() {
        let text = Rc::new(String::from("optional payload"));
        let weak = Rc::downgrade(&text);
        let present = relar_object_new(1);
        let optionals = relar_seq_new();
        let zero = Int::small(0).unwrap();
        // SAFETY: optionals are null or owned one-field Objects; their payloads are strings.
        unsafe {
            relar_object_set(present, 0, Rc::as_ptr(&text) as usize, 3);
            relar_seq_append(optionals, 0, 5);
            relar_seq_append(optionals, present as usize, 5);
            relar_object_release(present);
            drop(text);
            let absent = relar_seq_get(optionals, zero.0) as *const Object;
            assert!(absent.is_null());
            relar_object_retain(absent);
            relar_object_release(absent);
            let saved = relar_seq_pop(optionals) as *const Object;
            relar_object_release(optionals);
            let payload = Rc::from_raw(relar_object_get(saved, 0) as *const String);
            relar_object_release(saved);
            assert_eq!(payload.as_str(), "optional payload");
            drop(payload);
        }
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn splitting_owns_strings_and_matches_python_whitespace_boundaries() {
        // The complete whitespace set in CPython 3.14's _PyUnicode_IsWhitespace.
        let whitespace = "\u{9}\u{a}\u{b}\u{c}\u{d}\u{1c}\u{1d}\u{1e}\u{1f}\u{20}\u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
        let text = Rc::new(format!("{whitespace}α{whitespace}β\u{200b}γ{whitespace}"));
        let zero = Int::small(0).unwrap();
        let one = Int::small(1).unwrap();
        // SAFETY: the input is rooted; split returns an owned sequence of string references.
        unsafe {
            let parts = relar_str_split_whitespace(Rc::as_ptr(&text));
            drop(text);
            assert_eq!(Int(relar_seq_len(parts)).as_small(), Some(2));
            let first = Rc::from_raw(relar_seq_get(parts, zero.0) as *const String);
            let second = Rc::from_raw(relar_seq_get(parts, one.0) as *const String);
            relar_object_release(parts);
            assert_eq!(first.as_str(), "α");
            assert_eq!(second.as_str(), "β\u{200b}γ");
            let joined = Rc::from_raw(relar_str_concat(Rc::as_ptr(&first), Rc::as_ptr(&second)));
            drop((first, second));
            assert_eq!(joined.as_str(), "αβ\u{200b}γ");
        }
    }
}
