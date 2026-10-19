//! Typed int/string hash tables with insertion-ordered dictionary entries.
//!
//! Keys are monomorphized once per table; values are plain owned words with one
//! shared descriptor. Sets store no value words. Active iterators keep their
//! owner's entry directory stable through a compaction lease, never a Rust borrow.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

use super::exceptions::{Error, Kind, string_repr};
use super::words::{OwnedWord, WordKind};
use super::{Int, Object, Storage, StorageCell, Str, acquire_rc, fail, object};

#[derive(Clone)]
struct IntKey(Int);

impl PartialEq for IntKey {
    fn eq(&self, other: &Self) -> bool {
        match (self.0.as_small(), other.0.as_small()) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.0.big() == other.0.big(),
            _ => false,
        }
    }
}
impl Eq for IntKey {}
impl Hash for IntKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if let Some(value) = self.0.as_small() {
            0_u8.hash(state);
            value.hash(state);
        } else {
            1_u8.hash(state);
            self.0.big().hash(state);
        }
    }
}

/// Only the table's exact immutable scalar key representation implements this.
trait ScalarKey: Clone + Eq + Hash {
    const KIND: u32;
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self>;
    unsafe fn from_owned(word: usize) -> Self;
    fn into_word(self) -> usize;
}
impl ScalarKey for IntKey {
    const KIND: u32 = 2;
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Self(Int(word)))
    }
    unsafe fn from_owned(word: usize) -> Self {
        Self(Int(word))
    }
    fn into_word(self) -> usize {
        self.0.into_raw()
    }
}
impl ScalarKey for Rc<Str> {
    const KIND: u32 = 3;
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        // SAFETY: the caller roots an exact string handle; ManuallyDrop does not consume it.
        ManuallyDrop::new(unsafe { Rc::from_raw(word as *const Str) })
    }
    unsafe fn from_owned(word: usize) -> Self {
        // SAFETY: the caller transfers an independently acquired string owner.
        unsafe { Rc::from_raw(word as *const Str) }
    }
    fn into_word(self) -> usize {
        Rc::into_raw(self) as usize
    }
}

struct Entry<K, V> {
    key: K,
    value: V,
}
struct Table<K, V> {
    index: HashMap<K, usize>,
    entries: Vec<Option<Entry<K, V>>>,
    iterators: Weak<()>,
}

// Every live entry needs at least eight bytes, including key-only set entries.
// This retains the compiler's common 2^60-1 bound for container lengths.
const _: () = assert!(size_of::<Option<Entry<IntKey, ()>>>() >= 8);
const _: () = assert!(size_of::<Option<Entry<Rc<Str>, ()>>>() >= 8);

impl<K: ScalarKey, V: Copy> Table<K, V> {
    fn new() -> Self {
        Self {
            index: HashMap::new(),
            entries: Vec::new(),
            iterators: Weak::new(),
        }
    }

    /// Preserves first-insertion position and returns owners displaced by an update.
    fn insert(&mut self, key: K, value: V) -> Option<(K, V)> {
        if let Some(&position) = self.index.get(&key) {
            let entry = self.entries[position].as_mut().unwrap();
            return Some((key, std::mem::replace(&mut entry.value, value)));
        }
        self.compact();
        self.index
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "mapping allocation failed"));
        self.entries
            .try_reserve(1)
            .unwrap_or_else(|_| fail("MemoryError", "mapping allocation failed"));
        self.index.insert(key.clone(), self.entries.len());
        self.entries.push(Some(Entry { key, value }));
        None
    }

    /// Returns both removed key owners, avoiding any drop during mutable access.
    fn remove(&mut self, key: &K) -> Option<(K, Entry<K, V>)> {
        let (indexed, position) = self.index.remove_entry(key)?;
        let entry = self.entries[position].take().unwrap();
        self.compact();
        Some((indexed, entry))
    }

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

    unsafe fn lookup(&self, word: usize) -> Option<&Entry<K, V>> {
        // SAFETY: the caller roots this table's exact key representation.
        let key = unsafe { K::borrow(word) };
        self.index
            .get(&*key)
            .map(|&position| self.entries[position].as_ref().unwrap())
    }

    fn next(&self, position: usize) -> Option<(usize, OwnedWord, V)> {
        self.entries
            .iter()
            .enumerate()
            .skip(position)
            .find_map(|(position, entry)| {
                entry.as_ref().map(|entry| {
                    let word = entry.key.clone().into_word();
                    // SAFETY: cloning acquired one owner of this exact key kind.
                    let key = unsafe { OwnedWord::from_owned(word, WordKind::new(K::KIND)) };
                    (position + 1, key, entry.value)
                })
            })
    }
}

enum Tables<V> {
    Int(Table<IntKey, V>),
    Str(Table<Rc<Str>, V>),
}

macro_rules! table {
    ($self:expr, $table:ident => $body:expr) => {
        match $self {
            Tables::Int($table) => $body,
            Tables::Str($table) => $body,
        }
    };
}

impl<V: Copy> Tables<V> {
    fn new(kind: u32) -> Self {
        match kind {
            2 => Self::Int(Table::new()),
            3 => Self::Str(Table::new()),
            _ => fail("SystemError", "mapping keys must be integers or strings"),
        }
    }
    fn key_kind(&self) -> u32 {
        match self {
            Self::Int(_) => 2,
            Self::Str(_) => 3,
        }
    }
    fn len(&self) -> usize {
        table!(self, table => table.index.len())
    }
    #[cfg(test)]
    fn entry_count(&self) -> usize {
        table!(self, table => table.entries.len())
    }
    fn lease(&mut self) -> Rc<()> {
        table!(self, table => table.iterator_lease())
    }
    unsafe fn lookup(&self, word: usize) -> Option<V> {
        // SAFETY: each dispatched table borrows its exact rooted key kind.
        table!(self, table => unsafe { table.lookup(word) }.map(|entry| entry.value))
    }
    fn insert(&mut self, key: OwnedWord, value: V) -> Option<(OwnedWord, V)> {
        fn insert<K: ScalarKey, V: Copy>(
            table: &mut Table<K, V>,
            key: OwnedWord,
            value: V,
        ) -> Option<(OwnedWord, V)> {
            assert_eq!(key.kind.raw(), K::KIND);
            // SAFETY: the key temporary transfers one exact typed owner.
            let key = unsafe { K::from_owned(key.into_word()) };
            table.insert(key, value).map(|(key, value)| {
                // SAFETY: insert transfers the unused incoming key reference.
                (
                    unsafe { OwnedWord::from_owned(key.into_word(), WordKind::new(K::KIND)) },
                    value,
                )
            })
        }
        table!(self, table => insert(table, key, value))
    }
    unsafe fn remove(&mut self, word: usize) -> Option<(OwnedWord, OwnedWord, V)> {
        unsafe fn remove<K: ScalarKey, V: Copy>(
            table: &mut Table<K, V>,
            word: usize,
        ) -> Option<(OwnedWord, OwnedWord, V)> {
            // SAFETY: the caller roots an exact key of this table's kind.
            let key = unsafe { K::borrow(word) };
            table.remove(&key).map(|(indexed, entry)| {
                // SAFETY: the table transferred two independent key owners.
                unsafe {
                    (
                        OwnedWord::from_owned(indexed.into_word(), WordKind::new(K::KIND)),
                        OwnedWord::from_owned(entry.key.into_word(), WordKind::new(K::KIND)),
                        entry.value,
                    )
                }
            })
        }
        // SAFETY: dispatch preserves the exact key kind.
        table!(self, table => unsafe { remove(table, word) })
    }
    fn next(&self, position: usize) -> Option<(usize, OwnedWord, V)> {
        table!(self, table => table.next(position))
    }
}

pub(super) struct Mapping {
    tables: Tables<usize>,
    value_kind: WordKind,
}
pub(super) struct Set {
    tables: Tables<()>,
}

impl Mapping {
    fn new(key_kind: u32, value_kind: u32) -> Self {
        Self {
            tables: Tables::new(key_kind),
            value_kind: WordKind::new(value_kind),
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        table!(&mut self.tables, table => {
            for entry in table.entries.iter_mut().flatten() {
                // SAFETY: each dictionary entry owns one word of the shared kind.
                unsafe { self.value_kind.release(entry.value) };
            }
        });
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
    /// Checks current size and snapshots owners before returning to a loop body.
    /// Exhaustion is permanent; size failures stay sticky after size restoration.
    fn next(&mut self) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, IterationError> {
        if self.exhausted {
            return Ok(None);
        }
        // SAFETY: the iterator and rooted mapping are distinct allocations;
        // snapshot acquisition cannot reenter and no storage reference escapes.
        let result = unsafe {
            self.owner.0.with(|storage| {
                let length = storage.iteration_len(self.kind);
                if length != self.expected_len {
                    return Err(if self.kind == IteratorKind::Set {
                        IterationError::SetSize
                    } else {
                        IterationError::DictionarySize
                    });
                }
                let next = if self.kind == IteratorKind::Set {
                    storage
                        .set()
                        .tables
                        .next(self.position)
                        .map(|(position, key, ())| (position, key, None))
                } else {
                    let mapping = storage.dictionary();
                    mapping
                        .tables
                        .next(self.position)
                        .map(|(position, key, word)| {
                            let value = (self.kind == IteratorKind::DictionaryItems)
                                .then(|| mapping.value_kind.acquire(word));
                            (position, key, value)
                        })
                };
                if next.is_some() && self.kind != IteratorKind::Set && self.remaining == 0 {
                    return Err(IterationError::DictionaryKeys);
                }
                Ok(next)
            })
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if error == IterationError::DictionaryKeys {
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
            Self::Dictionary(value) => value,
            _ => fail("SystemError", "expected a dictionary object"),
        }
    }
    fn dictionary_mut(&mut self) -> &mut Mapping {
        match self {
            Self::Dictionary(value) => value,
            _ => fail("SystemError", "expected a dictionary object"),
        }
    }
    fn set(&self) -> &Set {
        match self {
            Self::Set(value) => value,
            _ => fail("SystemError", "expected a set object"),
        }
    }
    fn set_mut(&mut self) -> &mut Set {
        match self {
            Self::Set(value) => value,
            _ => fail("SystemError", "expected a set object"),
        }
    }
    fn iteration_len(&self, kind: IteratorKind) -> usize {
        if kind == IteratorKind::Set {
            self.set().tables.len()
        } else {
            self.dictionary().tables.len()
        }
    }
    fn iteration_lease(&mut self, kind: IteratorKind) -> Rc<()> {
        if kind == IteratorKind::Set {
            self.set_mut().tables.lease()
        } else {
            self.dictionary_mut().tables.lease()
        }
    }
    fn mapping_iterator_mut(&mut self) -> &mut MappingIterator {
        match self {
            Self::MappingIterator(value) => value,
            _ => fail("SystemError", "expected a mapping iterator"),
        }
    }
}

/// Creates a dictionary with concrete scalar keys and one shared value descriptor.
#[unsafe(no_mangle)]
pub extern "C" fn relar_dict_new(key_kind: u32, value_kind: u32) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Dictionary(
        Box::new(Mapping::new(key_kind, value_kind)),
    )))))
}
/// # Safety
/// `value` is a rooted dictionary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_len(value: *const Object) -> usize {
    // SAFETY: only its copied length escapes.
    Int::from_usize(unsafe { object(value).0.with(|s| s.dictionary().tables.len()) }).into_raw()
}
/// # Safety
/// Both inputs are rooted and `key` has the dictionary's exact key representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_contains(value: *const Object, key: usize) -> u8 {
    // SAFETY: hashing/equality cannot call user code.
    u8::from(unsafe {
        object(value)
            .0
            .with(|s| s.dictionary().tables.lookup(key).is_some())
    })
}
unsafe fn lookup(value: *const Object, key: usize) -> Option<OwnedWord> {
    // SAFETY: only an independently acquired value leaves storage.
    unsafe {
        object(value).0.with(|s| {
            let mapping = s.dictionary();
            mapping
                .tables
                .lookup(key)
                .map(|word| mapping.value_kind.acquire(word))
        })
    }
}
unsafe fn missing_key(value: *const Object, key: usize) -> Error {
    // SAFETY: only copied key-kind metadata leaves storage.
    let kind = unsafe { object(value).0.with(|s| s.dictionary().tables.key_kind()) };
    // SAFETY: the caller roots the exact immutable key kind.
    let message = unsafe {
        match kind {
            2 => Str::from(Int::borrow(key).big().to_string()),
            3 => string_repr(&*(key as *const Str)),
            _ => unreachable!(),
        }
    };
    Error::new(Kind::KeyError, message)
}
/// # Safety
/// Inputs are rooted exact kinds; success transfers a value owner. Check pending
/// before interpreting zero, which is also a valid nullable/scalar value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_get(value: *const Object, key: usize) -> usize {
    // SAFETY: the caller supplies lookup's rooted handles.
    if let Some(owned) = unsafe { lookup(value, key) } {
        return owned.into_word();
    }
    // SAFETY: input roots remain live after storage access ends.
    unsafe { missing_key(value, key) }.returning(0)
}
/// # Safety
/// Root the exact inputs; `out` is an aligned writable word outside runtime
/// storage. Missing leaves it untouched; success transfers one independently owned value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_try_get(
    value: *const Object,
    key: usize,
    out: *mut usize,
) -> u8 {
    // SAFETY: the caller roots the exact inputs.
    let Some(owned) = (unsafe { lookup(value, key) }) else {
        return 0;
    };
    // SAFETY: success permits this independent output slot.
    unsafe { out.write(owned.into_word()) };
    1
}
/// # Safety
/// The eagerly evaluated default has the dictionary's exact value kind and is
/// rooted alongside both other inputs. Success always transfers one owned value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_get_default(
    value: *const Object,
    key: usize,
    default: usize,
) -> usize {
    // SAFETY: all declared input representations remain rooted.
    if let Some(owned) = unsafe { lookup(value, key) } {
        return owned.into_word();
    }
    // SAFETY: only copied representation metadata escapes this rooted read.
    let kind = unsafe { object(value).0.with(|s| s.dictionary().value_kind) };
    // SAFETY: the eager default remains rooted and has this exact value kind.
    unsafe { kind.acquire(default) }.into_word()
}
/// # Safety
/// Inputs match this dictionary's concrete types and preserve heap acyclicity.
/// Acquisition precedes mutation, including replacement of the sole stored owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_set(value: *const Object, key: usize, word: usize) {
    // SAFETY: only copied descriptor metadata leaves storage.
    let (key_kind, value_kind) = unsafe {
        object(value).0.with(|s| {
            let mapping = s.dictionary();
            (WordKind::new(mapping.tables.key_kind()), mapping.value_kind)
        })
    };
    // SAFETY: both arguments are rooted before replacing any old owners.
    let (key, new) = unsafe { (key_kind.acquire(key), value_kind.acquire(word)) };
    // SAFETY: insertion cannot reenter; displaced owners leave storage together.
    let old = unsafe {
        object(value).0.with_mut(|s| {
            s.dictionary_mut()
                .tables
                .insert(key, new.into_word())
                .map(|(key, word)| (key, OwnedWord::from_owned(word, value_kind)))
        })
    };
    drop(old);
}
/// # Safety
/// Root the dictionary and its exact key. Missing sets KeyError; removal drops
/// both key owners and the value only after mutable storage access has ended.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_delete(value: *const Object, key: usize) {
    // SAFETY: lookup/removal cannot reenter; returned values own their references.
    let old = unsafe {
        object(value).0.with_mut(|s| {
            let mapping = s.dictionary_mut();
            mapping
                .tables
                .remove(key)
                .map(|(a, b, word)| (a, b, OwnedWord::from_owned(word, mapping.value_kind)))
        })
    };
    if old.is_none() {
        // SAFETY: the missing input remains rooted after the failed lookup.
        unsafe { missing_key(value, key) }.raise();
    }
    drop(old);
}
/// # Safety
/// Root a dictionary. Mode0 yields keys, mode1 items. Release the internal
/// iterator on every loop exit and never store it in a user container.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_iter(value: *const Object, mode: u32) -> *const Object {
    let kind = match mode {
        0 => IteratorKind::DictionaryKeys,
        1 => IteratorKind::DictionaryItems,
        _ => fail("SystemError", "invalid dictionary iteration mode"),
    };
    // SAFETY: the caller roots this exact container shape.
    unsafe { new_iterator(value, kind) }
}
unsafe fn new_iterator(value: *const Object, kind: IteratorKind) -> *const Object {
    // SAFETY: acquire an independent root before reading mutable cursor metadata.
    let owner = unsafe { acquire_rc::<Object>(value as usize) };
    // SAFETY: the independent owner roots storage; only a lease and size escape.
    let (lease, length) = unsafe {
        owner
            .0
            .with_mut(|s| (s.iteration_lease(kind), s.iteration_len(kind)))
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
#[unsafe(no_mangle)]
pub extern "C" fn relar_set_new(kind: u32) -> *const Object {
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Set(Box::new(
        Set {
            tables: Tables::new(kind),
        },
    ))))))
}
/// # Safety
/// `value` is a rooted set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_len(value: *const Object) -> usize {
    // SAFETY: only the copied length escapes storage.
    Int::from_usize(unsafe { object(value).0.with(|s| s.set().tables.len()) }).into_raw()
}
/// # Safety
/// Root both the set and its exact-kind scalar key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_contains(value: *const Object, key: usize) -> u8 {
    // SAFETY: hashing/equality cannot reenter user code.
    u8::from(unsafe {
        object(value)
            .0
            .with(|s| s.set().tables.lookup(key).is_some())
    })
}
/// # Safety
/// Root the set and exact key, including keys rooted only by this set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_add(value: *const Object, key: usize) {
    // SAFETY: copied metadata leaves storage before acquiring the key.
    let kind = unsafe {
        object(value)
            .0
            .with(|s| WordKind::new(s.set().tables.key_kind()))
    };
    // SAFETY: the caller roots this exact scalar key until acquisition completes.
    let key = unsafe { kind.acquire(key) };
    // SAFETY: unused incoming owners leave mutation before dropping.
    let old = unsafe {
        object(value)
            .0
            .with_mut(|s| s.set_mut().tables.insert(key, ()))
    };
    drop(old);
}
/// # Safety
/// Root the set and exact key. Missing is a no-op; removed owners drop afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_discard(value: *const Object, key: usize) {
    // SAFETY: returned key owners remain independent of their former table slots.
    let old = unsafe { object(value).0.with_mut(|s| s.set_mut().tables.remove(key)) };
    drop(old);
}
/// # Safety
/// Root the set and release this internal cursor on every loop exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_set_iter(value: *const Object) -> *const Object {
    // SAFETY: the caller supplies the exact set shape.
    unsafe { new_iterator(value, IteratorKind::Set) }
}
/// # Safety
/// Root the cursor; output words are aligned, independent, outside runtime storage,
/// and contain no unconsumed owner. Only success writes them; keys/set/string mode
/// ignores value_out and permits null. Size errors install RuntimeError and return0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_mapping_iter_next(
    value: *const Object,
    key_out: *mut usize,
    value_out: *mut usize,
) -> u8 {
    // SAFETY: the rooted iterator snapshots independent owners before returning.
    let next = unsafe {
        object(value).0.with_mut(|storage| match storage {
            Storage::StringIterator(iterator) => Ok(iterator.next()),
            _ => storage.mapping_iterator_mut().next(),
        })
    };
    let next = match next {
        Ok(next) => next,
        Err(error) => return Error::new(Kind::RuntimeError, error.message()).returning(0),
    };
    let Some((key, item)) = next else {
        return 0;
    };
    // SAFETY: the caller provides writable output slots only for the selected mode.
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
        relar_object_release, relar_seq_append, relar_seq_get_index, relar_seq_new,
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

    unsafe fn step(
        iterator: *const Object,
    ) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, IterationError> {
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
            for (kind, stored, default) in [(0, 0, 0), (1, 1, 0), (261, 0, 0)] {
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
            let default = Rc::from_raw(relar_seq_new(2));
            let weak = Rc::downgrade(&default);
            relar_seq_append(Rc::as_ptr(&default), integer(17), 2);
            let result = Rc::from_raw(relar_dict_get_default(
                dictionary,
                integer(0),
                Rc::as_ptr(&default) as usize,
            ) as *const Object);
            drop(default);
            relar_object_release(dictionary);
            assert_eq!(
                Int(relar_seq_get_index(Rc::as_ptr(&result), 0)).as_small(),
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
                object(dictionary)
                    .0
                    .with(|s| s.dictionary().tables.entry_count()),
                179
            );
            relar_object_release(iterator);
            relar_dict_delete(dictionary, integer(0));
            for key in 80..100 {
                relar_dict_delete(dictionary, integer(key));
            }
            assert!(
                object(dictionary)
                    .0
                    .with(|s| s.dictionary().tables.entry_count())
                    < 100
            );
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
