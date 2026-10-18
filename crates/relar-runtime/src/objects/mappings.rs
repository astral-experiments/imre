//! Dictionaries and sets with immutable scalar keys and owned typed values.
//!
//! A randomized hash index points into insertion-ordered entries. Deleted entries
//! become tombstones; active iterators hold a lease that prevents compaction.
//! Iterators root their owner and retain only an integer cursor between calls,
//! never a Rust reference or iterator into storage across a Python loop body.
//! Dictionaries expose insertion order; sets deliberately promise no order.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

use super::exceptions::{Error, Kind, string_repr};
use super::{Int, Object, Slot, Storage, StorageCell, Str, acquire_rc, fail, object};

#[derive(Clone, Copy)]
enum KeyKind {
    Int,
    Str,
}

impl KeyKind {
    fn from_representation(kind: u32) -> Self {
        match kind {
            2 => Self::Int,
            3 => Self::Str,
            _ => fail("SystemError", "mapping keys must be integers or strings"),
        }
    }
}

/// Immutable keys compare by scalar value, never by allocation identity.
/// Canonical integer handles make small and arbitrary-precision ranges disjoint.
#[derive(Clone)]
enum Key {
    Int(Int),
    Str(Rc<Str>),
}

impl Key {
    /// Borrows a rooted ABI word without changing its reference count.
    /// The temporary must not outlive that root or be dropped as an owned key.
    unsafe fn borrow(word: usize, kind: KeyKind) -> ManuallyDrop<Self> {
        if word == 0 {
            fail("TypeError", "unexpected null mapping key");
        }
        ManuallyDrop::new(match kind {
            KeyKind::Int => Self::Int(Int(word)),
            KeyKind::Str => {
                // SAFETY: the caller roots this Rc<Str>; ManuallyDrop prevents
                // consumption of its borrowed reference.
                Self::Str(unsafe { Rc::from_raw(word as *const Str) })
            }
        })
    }

    fn into_word(self) -> usize {
        match self {
            Self::Int(value) => value.into_raw(),
            Self::Str(value) => Rc::into_raw(value) as usize,
        }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => match (a.as_small(), b.as_small()) {
                (Some(a), Some(b)) => a == b,
                (None, None) => a.big() == b.big(),
                _ => false,
            },
            (Self::Str(a), Self::Str(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Int(value) => {
                if let Some(value) = value.as_small() {
                    0_u8.hash(state);
                    value.hash(state);
                } else {
                    1_u8.hash(state);
                    value.big().hash(state);
                }
            }
            Self::Str(value) => {
                2_u8.hash(state);
                value.hash(state);
            }
        }
    }
}

struct Entry {
    key: Key,
    value: Slot,
}

// Every live key has an ordered entry. Vec's isize::MAX-byte allocation limit
// therefore gives dictionaries and sets the compiler's shared MAX_LIST_LEN bound.
const _: () = assert!(size_of::<Option<Entry>>() >= 8);

/// Boxed mapping state keeps dictionary metadata from enlarging every Object.
pub(super) struct Mapping {
    key_kind: KeyKind,
    value_kind: u32,
    index: HashMap<Key, usize>,
    entries: Vec<Option<Entry>>,
    iterators: Weak<()>,
}

impl Mapping {
    fn new(key_kind: u32, value_kind: u32) -> Self {
        if value_kind > 5 {
            fail("SystemError", "invalid dictionary value representation");
        }
        Self {
            key_kind: KeyKind::from_representation(key_kind),
            value_kind,
            index: HashMap::new(),
            entries: Vec::new(),
            iterators: Weak::new(),
        }
    }

    fn entry(&self, key: &Key) -> Option<&Entry> {
        self.index
            .get(key)
            .map(|&position| self.entries[position].as_ref().unwrap())
    }

    /// Existing keys retain their position and original key owner. Return the
    /// unused incoming key and displaced value for destruction after storage access.
    fn insert(&mut self, key: Key, value: Slot) -> Option<(Key, Slot)> {
        if let Some(&position) = self.index.get(&key) {
            let entry = self.entries[position].as_mut().unwrap();
            return Some((key, std::mem::replace(&mut entry.value, value)));
        }
        self.compact();
        self.index
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "dictionary allocation failed"));
        self.entries
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "dictionary allocation failed"));
        let previous = self.index.insert(key.clone(), self.entries.len());
        debug_assert!(previous.is_none());
        self.entries.push(Some(Entry { key, value }));
        None
    }

    /// Removes both key owners and the value without dropping them inside the
    /// mutable access. Reinsertion will append a new entry at the end.
    fn remove(&mut self, key: &Key) -> Option<(Key, Entry)> {
        let (indexed_key, position) = self.index.remove_entry(key)?;
        let entry = self.entries[position].take().unwrap();
        self.compact();
        Some((indexed_key, entry))
    }

    /// Compaction moves live owners but drops only empty tombstones. A shared
    /// lease blocks it while an iterator could still use an old entry position.
    fn compact(&mut self) {
        if self.iterators.strong_count() != 0
            || self.entries.len() < 32
            || self.entries.len() - self.index.len() <= self.index.len()
        {
            return;
        }
        let mut position = 0;
        let index = &mut self.index;
        self.entries.retain_mut(|entry| {
            if let Some(entry) = entry {
                *index.get_mut(&entry.key).unwrap() = position;
                position += 1;
                true
            } else {
                false
            }
        });
    }

    fn iterator_lease(&mut self) -> Rc<()> {
        let lease = self.iterators.upgrade().unwrap_or_else(|| Rc::new(()));
        self.iterators = Rc::downgrade(&lease);
        lease
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IterationError {
    DictionarySize,
    DictionaryKeys,
    SetSize,
}

impl IterationError {
    fn message(self) -> &'static str {
        match self {
            Self::DictionarySize => "dictionary changed size during iteration",
            Self::DictionaryKeys => "dictionary keys changed during iteration",
            Self::SetSize => "Set changed size during iteration",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IteratorKind {
    DictionaryKeys,
    DictionaryItems,
    Set,
}

/// The owner and lease are ordinary Rc fields: releasing an iterator never
/// calls back into mapping storage. Each yielded key/value is independently owned.
pub(super) struct MappingIterator {
    owner: Rc<Object>,
    _lease: Rc<()>,
    expected_len: usize,
    position: usize,
    remaining: usize,
    kind: IteratorKind,
    exhausted: bool,
}

impl MappingIterator {
    /// Checks size before scanning current entries, including after the last
    /// yielded item. Exhaustion is permanent; replacing existing values is allowed.
    fn next(&mut self) -> Result<Option<(Key, Option<Slot>)>, IterationError> {
        if self.exhausted {
            return Ok(None);
        }
        // SAFETY: the rooted owner is a distinct mapping allocation, and no
        // storage reference escapes. Key/value clones cannot reenter either cell.
        let result = unsafe {
            self.owner.0.with(|storage| {
                let mapping = storage.iteration_mapping(self.kind);
                if mapping.index.len() != self.expected_len {
                    return Err(if self.kind == IteratorKind::Set {
                        IterationError::SetSize
                    } else {
                        IterationError::DictionarySize
                    });
                }
                for (position, entry) in mapping.entries.iter().enumerate().skip(self.position) {
                    if let Some(entry) = entry {
                        if self.kind != IteratorKind::Set && self.remaining == 0 {
                            return Err(IterationError::DictionaryKeys);
                        }
                        return Ok(Some((
                            position + 1,
                            entry.key.clone(),
                            (self.kind == IteratorKind::DictionaryItems)
                                .then(|| entry.value.clone()),
                        )));
                    }
                }
                Ok(None)
            })
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                // CPython keeps size failures sticky even if a handler restores
                // the size. Keys-changed failures instead exhaust the iterator.
                if matches!(error, IterationError::DictionaryKeys) {
                    self.exhausted = true;
                } else {
                    self.expected_len = usize::MAX;
                }
                return Err(error);
            }
        };
        if let Some((position, key, value)) = result {
            self.position = position;
            self.remaining = self.remaining.saturating_sub(1);
            Ok(Some((key, value)))
        } else {
            self.exhausted = true;
            Ok(None)
        }
    }
}

impl Storage {
    fn dictionary(&self) -> &Mapping {
        match self {
            Self::Dictionary(mapping) => mapping,
            _ => fail("SystemError", "expected a dictionary object"),
        }
    }

    fn dictionary_mut(&mut self) -> &mut Mapping {
        match self {
            Self::Dictionary(mapping) => mapping,
            _ => fail("SystemError", "expected a dictionary object"),
        }
    }

    fn set(&self) -> &Mapping {
        match self {
            Self::Set(mapping) => mapping,
            _ => fail("SystemError", "expected a set object"),
        }
    }

    fn set_mut(&mut self) -> &mut Mapping {
        match self {
            Self::Set(mapping) => mapping,
            _ => fail("SystemError", "expected a set object"),
        }
    }

    fn iteration_mapping(&self, kind: IteratorKind) -> &Mapping {
        if kind == IteratorKind::Set {
            self.set()
        } else {
            self.dictionary()
        }
    }

    fn iteration_mapping_mut(&mut self, kind: IteratorKind) -> &mut Mapping {
        if kind == IteratorKind::Set {
            self.set_mut()
        } else {
            self.dictionary_mut()
        }
    }

    fn mapping_iterator_mut(&mut self) -> &mut MappingIterator {
        match self {
            Self::MappingIterator(iterator) => iterator,
            _ => fail("SystemError", "expected a mapping iterator"),
        }
    }
}

/// Creates a typed dictionary; key kinds are exactly 2 (int) and 3 (str).
/// Values use the ordinary slot representation kinds 0 through 5.
#[unsafe(no_mangle)]
pub extern "C" fn relar_dict_new(key_kind: u32, value_kind: u32) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Dictionary(
        Box::new(Mapping::new(key_kind, value_kind)),
    )))))
}

/// # Safety
/// `value` must be a rooted dictionary. Returns an owned canonical integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_len(value: *const Object) -> usize {
    // SAFETY: only a copied length escapes this nonreentrant storage access.
    let length = unsafe {
        object(value)
            .0
            .with(|storage| storage.dictionary().index.len())
    };
    Int::from_usize(length).into_raw()
}

/// # Safety
/// `value` and `key` must be rooted handles matching the dictionary's key kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_contains(value: *const Object, key: usize) -> u8 {
    // SAFETY: the immutable key remains rooted; hashing cannot call user code.
    u8::from(unsafe {
        object(value).0.with(|storage| {
            let mapping = storage.dictionary();
            let key = Key::borrow(key, mapping.key_kind);
            mapping.index.contains_key(&*key)
        })
    })
}

/// Acquires a found value before leaving storage. Missing and a stored None are
/// distinct, allowing LLVM to flatten Optional values without nested boxes.
unsafe fn lookup(value: *const Object, key: usize) -> Option<Slot> {
    // SAFETY: the caller roots the dictionary/key, and cloning cannot reenter.
    unsafe {
        object(value).0.with(|storage| {
            let mapping = storage.dictionary();
            let key = Key::borrow(key, mapping.key_kind);
            mapping.entry(&key).map(|entry| entry.value.clone())
        })
    }
}

/// Formats the actual missing scalar key after dictionary storage access ends.
unsafe fn missing_key(value: *const Object, key: usize) -> Error {
    // SAFETY: the caller roots the dictionary; only immutable kind metadata escapes.
    let kind = unsafe {
        object(value)
            .0
            .with(|storage| storage.dictionary().key_kind)
    };
    // SAFETY: the caller roots a key of the dictionary's exact declared kind.
    let key = unsafe { Key::borrow(key, kind) };
    let message = match &*key {
        Key::Int(value) => Str::from(value.big().to_string()),
        Key::Str(value) => string_repr(value),
    };
    Error::new(Kind::KeyError, message)
}

/// # Safety
/// The dictionary and exact-kind key must be rooted. The returned word owns its
/// declared value type. A missing key sets KeyError; check pending before using
/// or acquiring the returned word, which is uninterpreted zero on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_get(value: *const Object, key: usize) -> usize {
    // SAFETY: the caller satisfies lookup's handle contract.
    if let Some(owned) = unsafe { lookup(value, key) } {
        return owned.into_word();
    }
    // SAFETY: both borrowed handles remain rooted after lookup returns.
    unsafe { missing_key(value, key) }.returning(0)
}

/// Writes one owned value only when the key exists; missing leaves `out` untouched.
///
/// # Safety
/// Dictionary/key handles must be rooted and correctly typed. On success, `out`
/// must point to writable, aligned usize storage outside runtime allocations.
/// The caller must consume each owned result before reusing that output slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_try_get(
    value: *const Object,
    key: usize,
    out: *mut usize,
) -> u8 {
    // SAFETY: the caller satisfies lookup's handle contract.
    let Some(owned) = (unsafe { lookup(value, key) }) else {
        return 0;
    };
    // SAFETY: the caller supplies an independent writable word slot on success.
    unsafe { out.write(owned.into_word()) };
    1
}

/// # Safety
/// The dictionary, key, and default are rooted values of the dictionary's exact
/// key/value kinds. The default has already been evaluated by the caller.
/// Returns one owned value, whether found or acquired from the default.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_get_default(
    value: *const Object,
    key: usize,
    default: usize,
) -> usize {
    // SAFETY: the caller satisfies lookup's handle contract.
    if let Some(owned) = unsafe { lookup(value, key) } {
        return owned.into_word();
    }
    // SAFETY: only the immutable representation kind escapes the storage read.
    let kind = unsafe {
        object(value)
            .0
            .with(|storage| storage.dictionary().value_kind)
    };
    // SAFETY: the caller roots a default of the dictionary's declared value kind.
    unsafe { Slot::acquire(default, kind) }.into_word()
}

/// Replaces a value without moving its key, or appends a newly inserted key.
///
/// # Safety
/// All handles must be rooted with the dictionary's exact key/value types.
/// The replacement must preserve the compiler's acyclic heap type graph.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_set(value: *const Object, key: usize, word: usize) {
    // SAFETY: only immutable kind metadata escapes this rooted storage read.
    let (key_kind, value_kind) = unsafe {
        object(value).0.with(|storage| {
            let mapping = storage.dictionary();
            (mapping.key_kind, mapping.value_kind)
        })
    };
    // SAFETY: acquire both arguments before mutation, including self-assignment.
    let (key, new) = unsafe {
        (
            (*Key::borrow(key, key_kind)).clone(),
            Slot::acquire(word, value_kind),
        )
    };
    // SAFETY: insertion cannot reenter. Displaced owners are returned for later drop.
    let old = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.dictionary_mut().insert(key, new))
    };
    drop(old);
}

/// # Safety
/// The dictionary and exact-kind key must be rooted. Missing keys set KeyError;
/// removed key/value owners are dropped after storage access.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_delete(value: *const Object, key: usize) {
    // SAFETY: lookup/removal cannot reenter; removed owners escape the access.
    let old = unsafe {
        object(value).0.with_mut(|storage| {
            let mapping = storage.dictionary_mut();
            let key = Key::borrow(key, mapping.key_kind);
            mapping.remove(&key)
        })
    };
    if old.is_none() {
        // SAFETY: the unchanged dictionary and missing input key remain rooted.
        unsafe { missing_key(value, key) }.raise();
    }
    drop(old);
}

/// Creates an internal iterator: mode 0 yields keys; mode 1 yields key/value pairs.
/// The compiler releases it with the ordinary Object ABI on every loop exit.
///
/// # Safety
/// `value` must be a rooted dictionary. Iterators are internal values and must
/// never be inserted into user containers, which would invalidate acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_iter(value: *const Object, mode: u32) -> *const Object {
    let kind = match mode {
        0 => IteratorKind::DictionaryKeys,
        1 => IteratorKind::DictionaryItems,
        _ => fail("SystemError", "invalid dictionary iteration mode"),
    };
    // SAFETY: the caller roots a dictionary and the mode was checked above.
    unsafe { new_iterator(value, kind) }
}

/// Acquires a rooted mapping owner and a compaction lease for this exact mode.
unsafe fn new_iterator(value: *const Object, kind: IteratorKind) -> *const Object {
    // SAFETY: the caller roots the mapping; the new iterator acquires its owner.
    let owner = unsafe { acquire_rc::<Object>(value as usize) };
    // SAFETY: only the independent lease and copied size escape; no callbacks run.
    let (lease, length) = unsafe {
        owner.0.with_mut(|storage| {
            let mapping = storage.iteration_mapping_mut(kind);
            (mapping.iterator_lease(), mapping.index.len())
        })
    };
    let iterator = MappingIterator {
        owner,
        _lease: lease,
        expected_len: length,
        position: 0,
        remaining: length,
        kind,
        exhausted: false,
    };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::MappingIterator(
        iterator,
    )))))
}

/// Creates a typed set; key kinds are exactly 2 (int) and 3 (str).
/// Its iteration order is unspecified even though the storage is shared with dicts.
#[unsafe(no_mangle)]
pub extern "C" fn relar_set_new(key_kind: u32) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Set(Box::new(
        Mapping::new(key_kind, 0),
    ))))))
}

/// # Safety
/// `value` must be a rooted set. Returns an owned canonical integer word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_len(value: *const Object) -> usize {
    // SAFETY: only a copied length escapes this nonreentrant storage access.
    let length = unsafe { object(value).0.with(|storage| storage.set().index.len()) };
    Int::from_usize(length).into_raw()
}

/// # Safety
/// `value` and `key` must be rooted handles matching the set's exact key kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_contains(value: *const Object, key: usize) -> u8 {
    // SAFETY: the immutable key remains rooted; hashing cannot call user code.
    u8::from(unsafe {
        object(value).0.with(|storage| {
            let mapping = storage.set();
            let key = Key::borrow(key, mapping.key_kind);
            mapping.index.contains_key(&*key)
        })
    })
}

/// # Safety
/// The set and exact-kind key must be rooted. Duplicate adds leave size unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_add(value: *const Object, key: usize) {
    // SAFETY: only immutable kind metadata escapes the rooted storage read.
    let kind = unsafe { object(value).0.with(|storage| storage.set().key_kind) };
    // SAFETY: acquire before mutation, including a key rooted only by this set.
    let key = unsafe { (*Key::borrow(key, kind)).clone() };
    // SAFETY: insertion cannot reenter, and displaced owners escape the access.
    let old = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.set_mut().insert(key, Slot::None))
    };
    drop(old);
}

/// # Safety
/// The set and exact-kind key must be rooted. A missing key is a no-op;
/// removed owners are dropped after mutable storage access ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_discard(value: *const Object, key: usize) {
    // SAFETY: lookup/removal cannot reenter; removed owners escape the access.
    let old = unsafe {
        object(value).0.with_mut(|storage| {
            let mapping = storage.set_mut();
            let key = Key::borrow(key, mapping.key_kind);
            mapping.remove(&key)
        })
    };
    drop(old);
}

/// Creates an internal iterator yielding owned keys in unspecified order.
///
/// # Safety
/// `value` must be a rooted set. The compiler must release the iterator on every
/// loop exit and never insert it into user containers, preserving acyclicity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_iter(value: *const Object) -> *const Object {
    // SAFETY: the caller roots the exact storage shape required by this mode.
    unsafe { new_iterator(value, IteratorKind::Set) }
}

/// Writes independently owned key/value words for one step, returning 1, or
/// leaves outputs untouched and returns 0 after permanent exhaustion.
///
/// # Safety
/// `value` must be a rooted internal mapping iterator. On success `key_out`, and
/// `value_out` for items mode, must be distinct writable aligned usize slots
/// outside runtime allocations. Keys-only/set modes ignore `value_out` and accept
/// null. Prior owned outputs must be consumed before the slots are reused.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_mapping_iter_next(
    value: *const Object,
    key_out: *mut usize,
    value_out: *mut usize,
) -> u8 {
    // SAFETY: the iterator owns its distinct container; only acquired values
    // escape, and mutable iterator access ends before output ownership transfers.
    let next = unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.mapping_iterator_mut().next())
    };
    let next = match next {
        Ok(next) => next,
        Err(error) => return Error::new(Kind::RuntimeError, error.message()).returning(0),
    };
    let Some((key, item)) = next else {
        return 0;
    };
    // SAFETY: the caller supplies independent writable word slots for this mode.
    unsafe {
        key_out.write(key.into_word());
        if let Some(item) = item {
            value_out.write(item.into_word());
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{
        relar_object_get, relar_object_new, relar_object_release, relar_object_set,
    };
    use num_bigint::BigInt;

    fn integer(value: i64) -> usize {
        Int::small(value).unwrap().into_raw()
    }

    unsafe fn collect_keys(iterator: *const Object) -> Vec<i64> {
        let mut values = Vec::new();
        let mut word = 0;
        // SAFETY: the caller roots a keys iterator for integer keys; every
        // successful output is consumed before its stack slot is reused.
        unsafe {
            while relar_mapping_iter_next(iterator, &mut word, std::ptr::null_mut()) != 0 {
                values.push(Int(word).as_small().unwrap());
            }
        }
        values
    }

    unsafe fn step(iterator: *const Object) -> Result<Option<(Key, Option<Slot>)>, IterationError> {
        // SAFETY: the caller roots the iterator; the result contains only owned values.
        unsafe {
            object(iterator)
                .0
                .with_mut(|storage| storage.mapping_iterator_mut().next())
        }
    }

    #[test]
    fn keys_use_scalar_equality_across_allocations_and_unicode_forms() {
        let dictionary = relar_dict_new(2, 2);
        let values = [
            BigInt::from(crate::MIN_SMALL),
            BigInt::from(-1),
            BigInt::from(0),
            BigInt::from(crate::MAX_SMALL),
            BigInt::from(1_u128 << 100),
            -BigInt::from(1_u128 << 100),
        ];
        // SAFETY: each key remains rooted through its call and each owned result
        // is consumed with its declared representation.
        unsafe {
            for (index, value) in values.iter().enumerate() {
                let key = Int::new(value.clone());
                relar_dict_set(dictionary, key.0, integer(index as i64));
            }
            for (index, value) in values.iter().enumerate() {
                let equal_key = Int::new(value.clone());
                assert_eq!(relar_dict_contains(dictionary, equal_key.0), 1);
                assert_eq!(
                    Int(relar_dict_get(dictionary, equal_key.0)).as_small(),
                    Some(index as i64)
                );
            }
            assert_eq!(Int(relar_dict_len(dictionary)).as_small(), Some(6));
            relar_object_release(dictionary);
        }

        let dictionary = relar_dict_new(3, 2);
        let texts = ["é", "e\u{301}", "name\0suffix", "😺"];
        // SAFETY: independent Rc strings have equal values without sharing an
        // allocation; exact UTF-8 equality does not normalize Unicode forms.
        unsafe {
            for (index, text) in texts.iter().enumerate() {
                let key = Rc::new(Str::from(*text));
                relar_dict_set(dictionary, Rc::as_ptr(&key) as usize, integer(index as i64));
            }
            for (index, text) in texts.iter().enumerate() {
                let key = Rc::new(Str::from(*text));
                assert_eq!(
                    Int(relar_dict_get(dictionary, Rc::as_ptr(&key) as usize)).as_small(),
                    Some(index as i64)
                );
            }
            assert_eq!(Int(relar_dict_len(dictionary)).as_small(), Some(4));
            relar_object_release(dictionary);
        }
    }

    #[test]
    fn replacement_acquires_sole_owners_and_reclaims_deleted_keys() {
        let key = Rc::new(BigInt::from(1_u128 << 120));
        let value = Rc::new(BigInt::from(1_u128 << 100));
        let key_weak = Rc::downgrade(&key);
        let value_weak = Rc::downgrade(&value);
        let (key_word, value_word) = (Rc::as_ptr(&key) as usize, Rc::as_ptr(&value) as usize);
        let dictionary = relar_dict_new(2, 2);
        // SAFETY: the dictionary roots its existing key/value during self-assignment;
        // the equal deletion key is independently owned. Snapshots are consumed once.
        unsafe {
            relar_dict_set(dictionary, key_word, value_word);
            drop((key, value));
            relar_dict_set(dictionary, key_word, value_word);
            assert_eq!(value_weak.strong_count(), 1);
            let saved = Int(relar_dict_get(dictionary, key_word));
            relar_dict_set(dictionary, key_word, integer(7));
            let equal_key = Int::new(BigInt::from(1_u128 << 120));
            relar_dict_delete(dictionary, equal_key.0);
            assert!(key_weak.upgrade().is_none());
            relar_object_release(dictionary);
            assert_eq!(saved.big().as_ref(), &BigInt::from(1_u128 << 100));
            drop(saved);
        }
        assert!(value_weak.upgrade().is_none());
    }

    #[test]
    fn optional_and_scalar_out_words_distinguish_missing_from_none() {
        // SAFETY: each dictionary has the supplied primitive value representation;
        // output slots are independent stack words and contain no owned old value.
        unsafe {
            for (kind, stored, default) in [(0, 0, 0), (1, 1, 0), (5, 0, 0)] {
                let dictionary = relar_dict_new(2, kind);
                relar_dict_set(dictionary, integer(0), stored);
                let mut out = 73;
                assert_eq!(relar_dict_try_get(dictionary, integer(0), &mut out), 1);
                assert_eq!(out, stored);
                out = 73;
                assert_eq!(relar_dict_try_get(dictionary, integer(1), &mut out), 0);
                assert_eq!(out, 73);
                assert_eq!(
                    relar_dict_get_default(dictionary, integer(1), default),
                    default
                );
                relar_object_release(dictionary);
            }
        }

        let dictionary = relar_dict_new(2, 5);
        // SAFETY: the new field object is owned by this Rc and contains one Int;
        // the default result acquires ownership independently of that Rc.
        unsafe {
            let default = Rc::from_raw(relar_object_new(1));
            let weak = Rc::downgrade(&default);
            relar_object_set(Rc::as_ptr(&default), 0, integer(17), 2);
            let result = Rc::from_raw(relar_dict_get_default(
                dictionary,
                integer(0),
                Rc::as_ptr(&default) as usize,
            ) as *const Object);
            drop(default);
            relar_object_release(dictionary);
            assert_eq!(
                Int(relar_object_get(Rc::as_ptr(&result), 0)).as_small(),
                Some(17)
            );
            drop(result);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn items_return_owned_snapshots_and_observe_later_value_updates() {
        // SAFETY: each constructor reference is transferred exactly once to Rc.
        let dictionary = unsafe { Rc::from_raw(relar_dict_new(2, 3)) };
        let dictionary_weak = Rc::downgrade(&dictionary);
        let original = Rc::new(Str::from("original"));
        let replacement = Rc::new(Str::from("replacement"));
        let original_weak = Rc::downgrade(&original);
        let replacement_weak = Rc::downgrade(&replacement);
        let pointer = Rc::as_ptr(&dictionary);
        // SAFETY: item outputs are independent stack slots. The iterator roots
        // its owner after the external dictionary reference is released.
        unsafe {
            for key in [0, 1] {
                relar_dict_set(pointer, integer(key), Rc::as_ptr(&original) as usize);
            }
            let iterator = relar_dict_iter(pointer, 1);
            let (mut key, mut value) = (0, 0);
            assert_eq!(relar_mapping_iter_next(iterator, &mut key, &mut value), 1);
            assert_eq!(Int(key).as_small(), Some(0));
            let first = Rc::from_raw(value as *const Str);
            for key in [0, 1] {
                relar_dict_set(pointer, integer(key), Rc::as_ptr(&replacement) as usize);
            }
            drop((original, replacement, dictionary));
            assert!(dictionary_weak.upgrade().is_some());
            assert_eq!(relar_mapping_iter_next(iterator, &mut key, &mut value), 1);
            assert_eq!(Int(key).as_small(), Some(1));
            let second = Rc::from_raw(value as *const Str);
            relar_object_release(iterator);
            assert!(dictionary_weak.upgrade().is_none());
            assert_eq!(first.as_str(), "original");
            assert_eq!(second.as_str(), "replacement");
            drop((first, second));
        }
        assert!(original_weak.upgrade().is_none());
        assert!(replacement_weak.upgrade().is_none());
    }

    #[test]
    fn active_iterator_positions_survive_deletion_reinsertion_and_compaction() {
        let dictionary = relar_dict_new(2, 2);
        // SAFETY: all keys/values are immediate Int words. Iterators and each
        // yielded key are consumed once; storage inspection returns copied lengths.
        unsafe {
            for key in 0..100 {
                relar_dict_set(dictionary, integer(key), integer(key));
            }
            let iterator = relar_dict_iter(dictionary, 0);
            let mut first = 0;
            assert_eq!(
                relar_mapping_iter_next(iterator, &mut first, std::ptr::null_mut()),
                1
            );
            assert_eq!(Int(first).as_small(), Some(0));
            for key in 1..80 {
                relar_dict_delete(dictionary, integer(key));
            }
            for key in 100..179 {
                relar_dict_set(dictionary, integer(key), integer(key));
            }
            assert_eq!(
                collect_keys(iterator),
                (80..100).chain(100..179).collect::<Vec<_>>()
            );
            assert_eq!(
                object(dictionary).0.with(|s| s.dictionary().entries.len()),
                179
            );
            relar_object_release(iterator);
            relar_dict_delete(dictionary, integer(0));
            for key in 80..100 {
                relar_dict_delete(dictionary, integer(key));
            }
            assert!(object(dictionary).0.with(|s| s.dictionary().entries.len()) < 100);
            relar_dict_set(dictionary, integer(120), integer(999));
            let iterator = relar_dict_iter(dictionary, 0);
            assert_eq!(collect_keys(iterator), (100..179).collect::<Vec<_>>());
            relar_object_release(iterator);
            relar_dict_delete(dictionary, integer(120));
            relar_dict_set(dictionary, integer(120), integer(1000));
            let iterator = relar_dict_iter(dictionary, 0);
            assert_eq!(
                collect_keys(iterator),
                (100..179)
                    .filter(|&key| key != 120)
                    .chain([120])
                    .collect::<Vec<_>>()
            );
            relar_object_release(iterator);
            relar_object_release(dictionary);
        }
    }

    #[test]
    fn iterator_mutation_errors_and_permanent_exhaustion_are_distinct() {
        let dictionary = relar_dict_new(2, 2);
        // SAFETY: every iterator remains rooted during inspection; step returns
        // owned values and exposes errors without invoking process-level fail.
        unsafe {
            let empty = relar_dict_iter(dictionary, 0);
            assert!(step(empty).unwrap().is_none());
            relar_dict_set(dictionary, integer(0), integer(0));
            assert!(step(empty).unwrap().is_none());
            relar_object_release(empty);

            let resized = relar_dict_iter(dictionary, 0);
            assert!(step(resized).unwrap().is_some());
            relar_dict_set(dictionary, integer(1), integer(1));
            assert!(matches!(step(resized), Err(IterationError::DictionarySize)));
            relar_object_release(resized);

            let changed = relar_dict_iter(dictionary, 0);
            assert_eq!(
                Int(step(changed).unwrap().unwrap().0.into_word()).as_small(),
                Some(0)
            );
            relar_dict_delete(dictionary, integer(0));
            relar_dict_set(dictionary, integer(2), integer(2));
            assert_eq!(
                Int(step(changed).unwrap().unwrap().0.into_word()).as_small(),
                Some(1)
            );
            assert!(matches!(step(changed), Err(IterationError::DictionaryKeys)));
            relar_object_release(changed);
            relar_object_release(dictionary);
        }
    }

    #[test]
    fn sets_deduplicate_integer_values_and_own_yielded_snapshots() {
        let original = Rc::new(BigInt::from(1_u128 << 120));
        let equal = Rc::new(BigInt::from(1_u128 << 120));
        let original_weak = Rc::downgrade(&original);
        let equal_weak = Rc::downgrade(&equal);
        let set = relar_set_new(2);
        // SAFETY: all keys are canonical rooted Int words. Every yielded owner
        // is transferred into Int before its independent output slot is reused.
        unsafe {
            relar_set_add(set, Rc::as_ptr(&original) as usize);
            relar_set_add(set, Rc::as_ptr(&equal) as usize);
            assert_eq!(Int(relar_set_len(set)).as_small(), Some(1));
            drop(equal);
            assert!(equal_weak.upgrade().is_none());
            drop(original);
            for key in [crate::MIN_SMALL, -1, 0, crate::MAX_SMALL] {
                relar_set_add(set, integer(key));
            }
            relar_set_discard(set, integer(7));
            assert_eq!(Int(relar_set_len(set)).as_small(), Some(5));
            let iterator = relar_set_iter(set);
            let mut snapshots = Vec::new();
            let mut word = 0;
            while relar_mapping_iter_next(iterator, &mut word, std::ptr::null_mut()) != 0 {
                snapshots.push(Int(word));
            }
            relar_object_release(iterator);
            let expected = Int::new(BigInt::from(1_u128 << 120));
            assert_eq!(relar_set_contains(set, expected.0), 1);
            relar_set_discard(set, expected.0);
            assert_eq!(relar_set_contains(set, expected.0), 0);
            relar_object_release(set);
            assert!(original_weak.upgrade().is_some());
            let mut actual: Vec<_> = snapshots
                .iter()
                .map(|value| value.big().as_ref().clone())
                .collect();
            actual.sort();
            assert_eq!(
                actual,
                [
                    BigInt::from(crate::MIN_SMALL),
                    BigInt::from(-1),
                    BigInt::from(0),
                    BigInt::from(crate::MAX_SMALL),
                    BigInt::from(1_u128 << 120),
                ]
            );
            drop(snapshots);
        }
        assert!(original_weak.upgrade().is_none());
    }

    #[test]
    fn sets_keep_exact_string_keys_and_reclaim_owner_after_iteration() {
        // SAFETY: the constructor's owned reference is transferred once to Rc.
        let set = unsafe { Rc::from_raw(relar_set_new(3)) };
        let set_weak = Rc::downgrade(&set);
        let pointer = Rc::as_ptr(&set);
        let texts = ["é", "e\u{301}", "name\0suffix", "😺"];
        let mut keys_weak = Vec::new();
        // SAFETY: each string is rooted through insertion. The iterator roots the
        // set after its external owner is dropped, and outputs acquire their own Rc.
        unsafe {
            for text in texts {
                let key = Rc::new(Str::from(text));
                keys_weak.push(Rc::downgrade(&key));
                relar_set_add(pointer, Rc::as_ptr(&key) as usize);
                let duplicate = Rc::new(Str::from(text));
                relar_set_add(pointer, Rc::as_ptr(&duplicate) as usize);
            }
            assert_eq!(Int(relar_set_len(pointer)).as_small(), Some(4));
            let iterator = relar_set_iter(pointer);
            drop(set);
            assert!(set_weak.upgrade().is_some());
            let mut snapshots = Vec::new();
            let mut word = 0;
            while relar_mapping_iter_next(iterator, &mut word, std::ptr::null_mut()) != 0 {
                snapshots.push(Rc::from_raw(word as *const Str));
            }
            relar_object_release(iterator);
            assert!(set_weak.upgrade().is_none());
            let mut actual: Vec<_> = snapshots.iter().map(|key| key.as_str()).collect();
            let mut expected = texts.to_vec();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected);
            assert!(keys_weak.iter().all(|key| key.upgrade().is_some()));
            drop(snapshots);
        }
        assert!(keys_weak.iter().all(|key| key.upgrade().is_none()));
    }

    #[test]
    fn set_iteration_checks_size_but_ignores_duplicate_adds_and_missing_discards() {
        let set = relar_set_new(2);
        // SAFETY: the set and iterators remain rooted; all values are immediate
        // integers and step returns only independent owners or an error.
        unsafe {
            let empty = relar_set_iter(set);
            assert!(step(empty).unwrap().is_none());
            relar_set_add(set, integer(0));
            assert!(step(empty).unwrap().is_none());
            relar_object_release(empty);

            let resized = relar_set_iter(set);
            relar_set_add(set, integer(0));
            relar_set_discard(set, integer(1));
            assert!(step(resized).unwrap().is_some());
            relar_set_add(set, integer(1));
            assert!(matches!(step(resized), Err(IterationError::SetSize)));
            relar_object_release(resized);

            let same_size = relar_set_iter(set);
            let yielded = step(same_size).unwrap().unwrap().0;
            relar_set_discard(set, yielded.into_word());
            relar_set_add(set, integer(2));
            // Sets have no dictionary-style remaining-key check or ordering
            // guarantee when deletion/insertion restores the original size.
            while step(same_size).unwrap().is_some() {}
            relar_object_release(same_size);
            relar_object_release(set);
        }
    }
}
