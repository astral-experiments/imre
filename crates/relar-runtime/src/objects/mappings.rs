//! Typed immutable-key hash tables with insertion-ordered dictionary entries.
//!
//! Keys are monomorphized once per table; values are plain owned words with one
//! shared descriptor. Sets store no value words. Each iterator owns a cursor
//! that the table can adjust when it removes vacant entries.

use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::sync::LazyLock;

mod sorted_rows;

use hashbrown::HashTable;
use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

use super::exceptions::{Error, Kind};
use super::words::{NULLABLE, OwnedWord, WordKind};
use super::{Bytes, Int, List, Object, Storage, StorageCell, Str, acquire_rc, fail, object};

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

/// A key roots the first inserted immutable path, and hashes its normalized
/// POSIX spelling without allocating or accessing the filesystem.
#[derive(Clone)]
struct PathKey(Rc<Object>);

impl PartialEq for PathKey {
    fn eq(&self, other: &Self) -> bool {
        // SAFETY: both keys root exact immutable path objects.
        unsafe { super::paths::relar_path_equal(Rc::as_ptr(&self.0), Rc::as_ptr(&other.0)) != 0 }
    }
}
impl Eq for PathKey {}
impl Hash for PathKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // SAFETY: the key roots immutable path storage for this nonreentrant hash.
        unsafe { self.0.0.with(|path| path.path().hash(state)) };
    }
}

/// An immutable tuple owns its concrete record without copying its fields into
/// a second allocation. Lookup borrows that owner; hashing cannot call user code.
struct TupleKey(*const usize);

impl Clone for TupleKey {
    fn clone(&self) -> Self {
        // SAFETY: this key roots one immutable, initialized tuple record.
        unsafe { super::records::relar_record_retain(self.0) };
        Self(self.0)
    }
}
impl Drop for TupleKey {
    fn drop(&mut self) {
        // SAFETY: every nonborrowed key owns one independently acquired record.
        unsafe { super::records::relar_record_release(self.0) };
    }
}
impl PartialEq for TupleKey {
    fn eq(&self, other: &Self) -> bool {
        // SAFETY: both keys root immutable tuples with no float or mutable fields.
        unsafe { super::values::equal(self.0 as usize, other.0 as usize, WordKind::new(6)) }
    }
}
impl Eq for TupleKey {}
impl Hash for TupleKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // SAFETY: checked tuple keys contain only immutable scalar/tuple fields.
        unsafe { hash_tuple(self.0, state) };
    }
}

/// # Safety
/// The rooted tuple has only exact immutable scalar, optional, or tuple fields.
unsafe fn hash_tuple<H: Hasher>(value: *const usize, state: &mut H) {
    let mut pending = Vec::new();
    let mut current = (value, 0);
    'tuples: loop {
        let (value, position) = current;
        // SAFETY: the root owns every tuple in the traversal and its descriptor.
        let layout = unsafe { super::records::descriptor(value) };
        if position == 0 {
            // Tuple equality requires the same descriptor. Hash it once per tuple.
            std::ptr::from_ref(layout).hash(state);
        }
        for index in position..layout.length {
            // SAFETY: the static layout covers each initialized concrete field.
            unsafe {
                let kind = WordKind::new(*layout.kinds.add(index));
                let word = kind.read_record_field(value.add(index));
                let raw = kind.raw();
                if raw & NULLABLE != 0 {
                    (word != 0).hash(state);
                    if word == 0 {
                        continue;
                    }
                }
                match raw & !NULLABLE {
                    0 | 1 | 10 => word.hash(state),
                    2 => IntKey::borrow(word).hash(state),
                    3 => (&*(word as *const Str)).hash(state),
                    5 => PathKey::borrow(word).hash(state),
                    6 => {
                        if index + 1 < layout.length {
                            pending.push((value, index + 1));
                        }
                        current = (word as *const usize, 0);
                        continue 'tuples;
                    }
                    8 => (&*(word as *const Bytes)).hash(state),
                    _ => unreachable!("the checker excludes mutable and float tuple keys"),
                }
            }
        }
        let Some(next) = pending.pop() else { return };
        current = next;
    }
}

/// Only the table's exact immutable key representation implements this.
trait ScalarKey: Clone + Eq + Hash {
    const KIND: u32;
    fn word(&self) -> usize;
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self>;
    unsafe fn from_owned(word: usize) -> Self;
    fn into_word(self) -> usize;
}
// Enum tags are immutable scalar keys within their statically checked family.
impl ScalarKey for usize {
    const KIND: u32 = 10;
    fn word(&self) -> usize {
        *self
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        ManuallyDrop::new(word)
    }
    unsafe fn from_owned(word: usize) -> Self {
        word
    }
    fn into_word(self) -> usize {
        self
    }
}
impl ScalarKey for IntKey {
    const KIND: u32 = 2;
    fn word(&self) -> usize {
        self.0.0
    }
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
    fn word(&self) -> usize {
        Rc::as_ptr(self) as usize
    }
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
impl ScalarKey for Rc<Bytes> {
    const KIND: u32 = 8;
    fn word(&self) -> usize {
        Rc::as_ptr(self) as usize
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        // SAFETY: the caller roots an exact bytes handle; ManuallyDrop does not consume it.
        ManuallyDrop::new(unsafe { Rc::from_raw(word as *const Bytes) })
    }
    unsafe fn from_owned(word: usize) -> Self {
        // SAFETY: the caller transfers an independently acquired bytes owner.
        unsafe { Rc::from_raw(word as *const Bytes) }
    }
    fn into_word(self) -> usize {
        Rc::into_raw(self) as usize
    }
}

impl ScalarKey for PathKey {
    const KIND: u32 = 5;
    fn word(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        // SAFETY: the caller roots an exact path; the borrowed Rc is never dropped.
        ManuallyDrop::new(Self(unsafe { Rc::from_raw(word as *const Object) }))
    }
    unsafe fn from_owned(word: usize) -> Self {
        // SAFETY: the caller transfers one independently acquired path owner.
        Self(unsafe { Rc::from_raw(word as *const Object) })
    }
    fn into_word(self) -> usize {
        Rc::into_raw(self.0) as usize
    }
}

impl ScalarKey for TupleKey {
    const KIND: u32 = 6;
    fn word(&self) -> usize {
        self.0 as usize
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        ManuallyDrop::new(Self(word as *const usize))
    }
    unsafe fn from_owned(word: usize) -> Self {
        Self(word as *const usize)
    }
    fn into_word(self) -> usize {
        let word = self.word();
        std::mem::forget(self);
        word
    }
}

// All tables share a randomized SipHash key, like dictionaries in one Python
// process. Immutable entries can pass their hash to another table or a resize.
static HASHER: LazyLock<RandomState> = LazyLock::new(RandomState::new);

struct Entry<K, V> {
    hash: u64,
    key: K,
    value: V,
}
struct Table<K, V> {
    index: HashTable<usize>,
    entries: Vec<Option<Entry<K, V>>>,
    iterators: Vec<Weak<Cell<usize>>>,
}

// Every live entry needs at least eight bytes, including key-only set entries.
// This retains the compiler's common 2^60-1 bound for container lengths.
const _: () = assert!(size_of::<Option<Entry<IntKey, ()>>>() >= 8);
const _: () = assert!(size_of::<Option<Entry<Rc<Str>, ()>>>() >= 8);

impl<K: ScalarKey, V: Copy> Table<K, V> {
    fn new() -> Self {
        Self {
            index: HashTable::new(),
            entries: Vec::new(),
            iterators: Vec::new(),
        }
    }

    /// The fingerprint rejects most bucket candidates before comparing key values.
    fn position<Q: Eq + ?Sized>(&self, hash: u64, key: &Q) -> Option<usize>
    where
        K: std::borrow::Borrow<Q>,
    {
        self.index
            .find(hash, |&position| {
                let entry = self.entries[position].as_ref().unwrap();
                entry.hash == hash && entry.key.borrow() == key
            })
            .copied()
    }

    fn reserve(&mut self, additional: usize) {
        let entries = &self.entries;
        self.index
            .try_reserve(additional, |&i| entries[i].as_ref().unwrap().hash)
            .unwrap_or_else(|_| fail("MemoryError", "mapping allocation failed"));
        self.entries
            .try_reserve(additional)
            .unwrap_or_else(|_| fail("MemoryError", "mapping allocation failed"));
    }

    fn insert_absent(&mut self, hash: u64, key: K, value: V) {
        self.compact();
        self.reserve(1);
        let position = self.entries.len();
        let entries = &self.entries;
        self.index
            .insert_unique(hash, position, |&i| entries[i].as_ref().unwrap().hash);
        self.entries.push(Some(Entry { hash, key, value }));
    }

    /// Preserves first-insertion position and returns owners displaced by an update.
    fn insert(&mut self, key: K, value: V) -> Option<(K, V)> {
        let hash = HASHER.hash_one(&key);
        if let Some(position) = self.position(hash, &key) {
            let entry = self.entries[position].as_mut().unwrap();
            return Some((key, std::mem::replace(&mut entry.value, value)));
        }
        self.insert_absent(hash, key, value);
        None
    }

    /// Transfers the removed key and value so they can drop outside the mutation.
    fn remove(&mut self, key: &K) -> Option<Entry<K, V>> {
        let hash = HASHER.hash_one(key);
        let slot = self
            .index
            .find_entry(hash, |&position| {
                let entry = self.entries[position].as_ref().unwrap();
                entry.hash == hash && &entry.key == key
            })
            .ok()?;
        let (position, _) = slot.remove();
        let entry = self.entries[position].take().unwrap();
        self.compact();
        Some(entry)
    }

    fn compact(&mut self) {
        if self.entries.len() < 32 || self.entries.len() - self.index.len() <= self.index.len() {
            return;
        }
        // Each cursor points just past the last entry it examined. After
        // compaction only the occupied slots in that prefix remain before it.
        self.iterators.retain(|iterator| {
            let Some(position) = iterator.upgrade() else {
                return false;
            };
            position.set(self.entries[..position.get()].iter().flatten().count());
            true
        });
        self.entries.retain(Option::is_some);
        self.index.clear();
        let entries = &self.entries;
        for (position, entry) in entries.iter().enumerate() {
            self.index
                .insert_unique(entry.as_ref().unwrap().hash, position, |&i| {
                    entries[i].as_ref().unwrap().hash
                });
        }
    }

    /// A key view already has the immutable hash needed by the result set.
    fn copy_keys(&self, result: &mut Table<K, ()>) {
        result.reserve(self.index.len().saturating_sub(result.index.len()));
        for entry in self.entries.iter().flatten() {
            if result.position(entry.hash, &entry.key).is_none() {
                result.insert_absent(entry.hash, entry.key.clone(), ());
            }
        }
    }

    fn iterator_position(&mut self) -> Rc<Cell<usize>> {
        self.iterators
            .retain(|iterator| iterator.strong_count() != 0);
        let position = Rc::new(Cell::new(0));
        self.iterators.push(Rc::downgrade(&position));
        position
    }

    unsafe fn lookup(&self, word: usize) -> Option<&Entry<K, V>> {
        // SAFETY: the caller roots this table's exact key representation.
        let key = unsafe { K::borrow(word) };
        self.position(HASHER.hash_one(&*key), &*key)
            .map(|position| self.entries[position].as_ref().unwrap())
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

impl<K: ScalarKey> Table<K, usize> {
    /// Hash a subscription once, including when the sealed list factory inserts.
    /// The table and the returned expression each acquire their own list owner.
    unsafe fn subscript(
        &mut self,
        word: usize,
        kind: WordKind,
        factory: Option<WordKind>,
    ) -> Option<OwnedWord> {
        // SAFETY: the incoming key remains rooted for this whole operation.
        let key = unsafe { K::borrow(word) };
        let hash = HASHER.hash_one(&*key);
        if let Some(position) = self.position(hash, &*key) {
            let value = self.entries[position].as_ref().unwrap().value;
            // SAFETY: the table roots the stored value of the declared kind.
            return Some(unsafe { kind.acquire(value) });
        }
        let element = factory?;
        let list = if element.raw() == 2 {
            crate::relar_list_new() as usize
        } else {
            super::relar_seq_new(element.raw()) as usize
        };
        // SAFETY: the factory transfers its fresh list to the table. The caller
        // owns the acquired result independently of any subsequent replacement.
        let result = unsafe { kind.acquire(list) };
        self.insert_absent(hash, (*key).clone(), list);
        Some(result)
    }
}

enum Tables<V> {
    Int(Table<IntKey, V>),
    Enum(Table<usize, V>),
    Str(Table<Rc<Str>, V>),
    Path(Table<PathKey, V>),
    Bytes(Table<Rc<Bytes>, V>),
    Tuple(Table<TupleKey, V>),
}

macro_rules! table {
    ($self:expr, $table:ident => $body:expr) => {
        match $self {
            Tables::Int($table) => $body,
            Tables::Enum($table) => $body,
            Tables::Str($table) => $body,
            Tables::Path($table) => $body,
            Tables::Bytes($table) => $body,
            Tables::Tuple($table) => $body,
        }
    };
}

impl<V: Copy> Tables<V> {
    /// Visits borrowed key/value words in insertion order without acquiring keys.
    /// The predicate may inspect other immutable acyclic containers, not mutate
    /// this table or retain a storage reference after the visit.
    fn all(&self, mut predicate: impl FnMut(usize, V) -> bool) -> bool {
        table!(self, table => {
            table.entries.iter().flatten().all(|entry| predicate(entry.key.word(), entry.value))
        })
    }

    fn new(kind: u32) -> Self {
        match kind {
            2 => Self::Int(Table::new()),
            10 => Self::Enum(Table::new()),
            3 => Self::Str(Table::new()),
            5 => Self::Path(Table::new()),
            6 => Self::Tuple(Table::new()),
            8 => Self::Bytes(Table::new()),
            _ => fail("SystemError", "unsupported immutable mapping key kind"),
        }
    }
    fn key_kind(&self) -> u32 {
        match self {
            Self::Int(_) => 2,
            Self::Enum(_) => 10,
            Self::Str(_) => 3,
            Self::Path(_) => 5,
            Self::Bytes(_) => 8,
            Self::Tuple(_) => 6,
        }
    }
    fn copy_keys(&self, result: &mut Tables<()>) {
        match (self, result) {
            (Self::Int(table), Tables::Int(result)) => table.copy_keys(result),
            (Self::Enum(table), Tables::Enum(result)) => table.copy_keys(result),
            (Self::Str(table), Tables::Str(result)) => table.copy_keys(result),
            (Self::Path(table), Tables::Path(result)) => table.copy_keys(result),
            (Self::Bytes(table), Tables::Bytes(result)) => table.copy_keys(result),
            (Self::Tuple(table), Tables::Tuple(result)) => table.copy_keys(result),
            _ => unreachable!("key union requires the same key kind"),
        }
    }
    fn len(&self) -> usize {
        table!(self, table => table.index.len())
    }
    #[cfg(test)]
    fn entry_count(&self) -> usize {
        table!(self, table => table.entries.len())
    }
    fn iterator_position(&mut self) -> Rc<Cell<usize>> {
        table!(self, table => table.iterator_position())
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
    unsafe fn remove(&mut self, word: usize) -> Option<(OwnedWord, V)> {
        unsafe fn remove<K: ScalarKey, V: Copy>(
            table: &mut Table<K, V>,
            word: usize,
        ) -> Option<(OwnedWord, V)> {
            // SAFETY: the caller roots an exact key of this table's kind.
            let key = unsafe { K::borrow(word) };
            table.remove(&key).map(|entry| {
                // SAFETY: the table transferred the stored key owner.
                unsafe {
                    (
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

/// Borrowed canonical integer words; zero denotes a missing ASCII key. Keeping
/// the heap-value count lets gathers prove that copying words acquires no owners.
struct AsciiIntCache {
    values: [usize; 128],
    heap_values: usize,
}

impl AsciiIntCache {
    fn new() -> Self {
        Self {
            values: [0; 128],
            heap_values: 0,
        }
    }

    /// Maintains the small-value proof across insertion, replacement, and deletion.
    fn set(&mut self, character: u8, word: usize) {
        let old = std::mem::replace(&mut self.values[usize::from(character)], word);
        self.heap_values -= usize::from(old != 0 && old & 1 == 0);
        self.heap_values += usize::from(word != 0 && word & 1 == 0);
    }
}

pub(super) struct Mapping {
    tables: Tables<usize>,
    value_kind: WordKind,
    /// A sealed list factory is invoked only by a missing subscription.
    default_list_element: Option<WordKind>,
    /// Present only after a string-to-integer gather. Words borrow current table
    /// values; ASCII mutation updates the corresponding slot before old owners drop.
    ascii_int: Option<Box<AsciiIntCache>>,
}
pub(super) struct Set {
    tables: Tables<()>,
}

impl Mapping {
    /// Borrows entries in Python insertion order. The caller must keep the
    /// enclosing storage borrow live and cannot mutate or invoke user code.
    pub(super) fn string_entries(&self) -> impl Iterator<Item = (&Str, usize)> {
        let Tables::Str(table) = &self.tables else {
            fail("SystemError", "expected a string-key dictionary");
        };
        table
            .entries
            .iter()
            .flatten()
            .map(|entry| (&*entry.key, entry.value))
    }

    /// Transfers two exact owners only for a new key. Duplicate inputs are
    /// dropped together, preserving existing entries during strict decoding.
    pub(super) fn insert_unique(&mut self, key: OwnedWord, value: OwnedWord) -> bool {
        assert_eq!(key.kind.raw(), self.tables.key_kind());
        assert_eq!(value.kind, self.value_kind);
        // SAFETY: the temporary key roots this table's exact immutable key kind.
        if unsafe { self.tables.lookup(key.word) }.is_some() {
            return false;
        }
        let displaced = self.tables.insert(key, value.into_word());
        debug_assert!(displaced.is_none());
        true
    }

    /// Copies exact string entries in insertion order before a process launch.
    /// No borrowed table storage or language owner crosses the fork boundary.
    pub(super) fn string_pairs(&self) -> Vec<(Str, Str)> {
        if self.tables.key_kind() != 3 || self.value_kind.raw() != 3 {
            fail("SystemError", "expected a string-to-string dictionary");
        }
        let mut pairs = Vec::with_capacity(self.tables.len());
        self.tables.all(|key, value| {
            // SAFETY: the checked exact dictionary roots both immutable strings.
            unsafe {
                pairs.push((
                    (*(key as *const Str)).clone(),
                    (*(value as *const Str)).clone(),
                ));
            }
            true
        });
        pairs
    }

    /// # Safety
    /// Both tables and all values are rooted and immutable for this traversal.
    pub(super) unsafe fn compare_entry(
        &self,
        other: &Self,
        position: usize,
    ) -> super::values::Comparison {
        use super::values::Comparison;
        if self.tables.key_kind() != other.tables.key_kind()
            || self.value_kind != other.value_kind
            || self.tables.len() != other.tables.len()
        {
            return Comparison::Done(false);
        }
        let Some((next, key, left)) = self.tables.next(position) else {
            return Comparison::Done(true);
        };
        // SAFETY: table kinds match and both tables retain their entries.
        match unsafe { other.tables.lookup(key.word) } {
            Some(right) => Comparison::Child {
                left,
                right,
                kind: self.value_kind,
                resume: Some(next),
            },
            None => Comparison::Done(false),
        }
    }

    /// Copies the next entry while the original table roots both words.
    pub(super) fn repr_entry(
        &self,
        output: &mut String,
        position: usize,
    ) -> Option<super::values::ReprEntry> {
        if position == 0 {
            if self.default_list_element.is_some() {
                output.push_str("defaultdict(<class 'list'>, ");
            }
            output.push('{');
        }
        let Some((next, key, value)) = self.tables.next(position) else {
            output.push('}');
            if self.default_list_element.is_some() {
                output.push(')');
            }
            return None;
        };
        if position != 0 {
            output.push_str(", ");
        }
        Some(super::values::ReprEntry {
            next,
            key: (key.word, key.kind),
            value: Some((value, self.value_kind)),
        })
    }

    pub(super) fn new(key_kind: u32, value_kind: u32) -> Self {
        Self {
            tables: Tables::new(key_kind),
            value_kind: WordKind::new(value_kind),
            default_list_element: None,
            ascii_int: None,
        }
    }

    /// Updates only an already-built cache. Other dictionary representations
    /// neither allocate a cache nor reinterpret their keys as strings.
    ///
    /// # Safety
    /// `key` is a rooted key of this mapping's exact kind. When the cache exists,
    /// `word` is a rooted canonical integer word, or zero after deletion.
    unsafe fn update_ascii_int(&mut self, key: usize, word: usize) {
        if let Some(cache) = &mut self.ascii_int {
            // SAFETY: only exact dict[str, int] gather creates this cache.
            let key = unsafe { &*(key as *const Str) };
            if let [character] = key.as_bytes() {
                // A one-byte valid UTF-8 string is necessarily ASCII.
                cache.set(*character, word);
            }
        }
    }
}

impl Set {
    /// # Safety
    /// Both sets remain rooted and immutable for the entire comparison.
    pub(super) unsafe fn equal(&self, other: &Self) -> bool {
        self.tables.key_kind() == other.tables.key_kind()
            && self.tables.len() == other.tables.len()
            && self.tables.all(|key, ()| {
                // SAFETY: matching table kinds borrow this rooted scalar key.
                unsafe { other.tables.lookup(key).is_some() }
            })
    }

    /// Copies the next key while the original set roots it.
    pub(super) fn repr_entry(
        &self,
        output: &mut String,
        position: usize,
    ) -> Option<super::values::ReprEntry> {
        if position == 0 {
            if self.tables.len() == 0 {
                output.push_str("set()");
                return None;
            }
            output.push('{');
        }
        let Some((next, key, ())) = self.tables.next(position) else {
            output.push('}');
            return None;
        };
        if position != 0 {
            output.push_str(", ");
        }
        Some(super::values::ReprEntry {
            next,
            key: (key.word, key.kind),
            value: None,
        })
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
    expected_len: usize,
    position: Rc<Cell<usize>>,
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
                        .next(self.position.get())
                        .map(|(position, key, ())| (position, key, None))
                } else {
                    let mapping = storage.dictionary();
                    mapping
                        .tables
                        .next(self.position.get())
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
            self.position.set(position);
            self.remaining = self.remaining.saturating_sub(1);
            Ok(Some((key, value)))
        } else {
            self.exhausted = true;
            Ok(None)
        }
    }
}

impl Storage {
    pub(super) fn dictionary(&self) -> &Mapping {
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
    fn iteration_position(&mut self, kind: IteratorKind) -> Rc<Cell<usize>> {
        if kind == IteratorKind::Set {
            self.set_mut().tables.iterator_position()
        } else {
            self.dictionary_mut().tables.iterator_position()
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
/// Attaches the exact element descriptor to a native `defaultdict(list)`.
#[unsafe(no_mangle)]
pub extern "C" fn relar_default_list_dict_new(key_kind: u32, element_kind: u32) -> *const Object {
    let mut mapping = Mapping::new(key_kind, if element_kind == 2 { 4 } else { 5 });
    mapping.default_list_element = Some(WordKind::new(element_kind));
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Dictionary(
        Box::new(mapping),
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
pub unsafe extern "C" fn relar_dict_contains(value: *const Object, key: usize) -> u32 {
    // SAFETY: hashing/equality cannot call user code.
    u32::from(unsafe {
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
    // SAFETY: the caller roots the missing key until we acquire its exact kind.
    Error::key(unsafe { WordKind::new(kind).acquire(key) })
}
/// # Safety
/// Inputs are rooted exact kinds; success transfers a value owner. Check pending
/// before interpreting zero, which is also a valid nullable/scalar value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_get(value: *const Object, key: usize) -> usize {
    // SAFETY: this native subscription only hashes immutable keys and constructs
    // an empty builtin list, so no storage borrow crosses a user callback.
    let owned = unsafe {
        object(value).0.with_mut(|storage| {
        let mapping = storage.dictionary_mut();
        table!(&mut mapping.tables, table => table.subscript(key, mapping.value_kind, mapping.default_list_element))
    })
    };
    if let Some(owned) = owned {
        return owned.into_word();
    }
    // SAFETY: input roots remain live after storage access ends.
    unsafe { missing_key(value, key) }.returning(0)
}

/// Gathers a dictionary's integer values in Unicode code-point order without
/// allocating a string owner for every key. A lazily built ASCII cache borrows
/// current dictionary values and is maintained by every mapping mutation.
///
/// # Safety
/// `text` is a rooted string and `value` is a rooted exact `dict[str, int]`.
/// The compiler proves the comprehension contains only these lookups, with no
/// callbacks or intervening mutations. Success transfers one list owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_gather_str_int(
    text: *const Str,
    value: *const Object,
) -> *const List {
    // SAFETY: the exact dictionary and immutable string remain rooted.
    unsafe { gather_str_int(&*text, value) }
}

/// Gathers directly from compiler-validated UTF-8 bytes without copying a string.
///
/// # Safety
/// `text` is rooted immutable bytes that already passed `relar_bytes_validate_utf8`
/// at the source decode expression. `value` is a rooted exact `dict[str, int]`,
/// with the same mutation-free comprehension proof as `relar_dict_gather_str_int`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_gather_bytes_int(
    text: *const Bytes,
    value: *const Object,
) -> *const List {
    // SAFETY: the compiler validates these exact immutable bytes before entry.
    let text = unsafe { std::str::from_utf8_unchecked(&*text) };
    // SAFETY: all borrowed contents and the exact dictionary remain rooted.
    unsafe { gather_str_int(text, value) }
}

/// Shares lookup, cache maintenance, and ownership between text and decoded bytes.
///
/// # Safety
/// `value` is a rooted exact `dict[str, int]`; no callbacks or mapping mutation
/// can occur during the gather. The borrowed text remains immutable throughout.
unsafe fn gather_str_int(text: &str, value: *const Object) -> *const List {
    // SAFETY: exact immutable keys, integer acquisition, and buffer allocation
    // cannot reenter. Only the internal lookup cache changes, and no borrowed
    // key/value or table address escapes this access.
    let result = unsafe {
        object(value).0.with_mut(|storage| {
            let mapping = storage.dictionary_mut();
            let Tables::Str(table) = &mapping.tables else {
                fail(
                    "SystemError",
                    "string gather requires string dictionary keys",
                );
            };
            if mapping.value_kind != WordKind::new(2) {
                fail(
                    "SystemError",
                    "string gather requires integer dictionary values",
                );
            }
            if mapping.ascii_int.is_none() {
                let mut ascii = Box::new(AsciiIntCache::new());
                for entry in table.entries.iter().flatten() {
                    if let [character] = entry.key.as_bytes() {
                        ascii.set(*character, entry.value);
                    }
                }
                mapping.ascii_int = Some(ascii);
            }
            let ascii = mapping.ascii_int.as_ref().unwrap();
            let ascii_small = ascii.heap_values == 0 && text.is_ascii();
            let length = if ascii_small {
                text.len()
            } else {
                text.chars().count()
            };
            let mut values = crate::take_list_buffer(length);
            values
                .try_reserve_exact(length)
                .unwrap_or_else(|_| fail("MemoryError", "list allocation failed"));
            if ascii_small {
                for (slot, &character) in values.spare_capacity_mut()[..length]
                    .iter_mut()
                    .zip(text.as_bytes())
                {
                    // SAFETY: ascii_small proved this immutable text is ASCII,
                    // so every byte indexes within the 128-entry lookup cache.
                    let word = *ascii.values.get_unchecked(usize::from(character));
                    if word == 0 {
                        let mut key = Str::new("");
                        key.push(char::from(character));
                        return Err(Error::string_key(key));
                    }
                    slot.write(Int(word));
                }
                // SAFETY: the equal-length zip initialized every slot. The
                // cache proves all words are small integers, so an earlier
                // missing key leaves no heap owners in unpublished slots.
                values.set_len(length);
                return Ok(values);
            }
            for character in text.chars() {
                let word = if character.is_ascii() {
                    ascii.values[character as usize]
                } else {
                    let mut key = Str::new("");
                    key.push(character);
                    table
                        .position(HASHER.hash_one(&key), &key)
                        .map_or(0, |position| {
                            table.entries[position].as_ref().unwrap().value
                        })
                };
                // Canonical Int words are nonzero, including the integer zero.
                if word == 0 {
                    let mut key = Str::new("");
                    key.push(character);
                    return Err(Error::string_key(key));
                }
                // SAFETY: this exact integer remains rooted by the dictionary;
                // the copied owner belongs to the fresh result or its cleanup.
                values.push((*Int::borrow(word)).clone());
            }
            Ok(values)
        })
    };
    match result {
        Ok(values) => crate::new_list(values),
        Err(error) => error.returning(std::ptr::null()),
    }
}
/// # Safety
/// Root the exact inputs; `out` is an aligned writable word outside runtime
/// storage. Missing leaves it untouched; success transfers one independently owned value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_try_get(
    value: *const Object,
    key: usize,
    out: *mut usize,
) -> u32 {
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
            let mapping = s.dictionary_mut();
            // Both incoming owners remain rooted while the cache and table are
            // updated together; insertion cannot reenter a dictionary lookup.
            mapping.update_ascii_int(key.word, new.word);
            mapping
                .tables
                .insert(key, new.into_word())
                .map(|(key, word)| (key, OwnedWord::from_owned(word, value_kind)))
        })
    };
    drop(old);
}
/// # Safety
/// Root the dictionary and its exact key. Missing sets KeyError; removal drops
/// the key and value owners only after mutable storage access has ended.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_dict_delete(value: *const Object, key: usize) {
    // SAFETY: lookup/removal cannot reenter; returned values own their references.
    let old = unsafe {
        object(value).0.with_mut(|s| {
            let mapping = s.dictionary_mut();
            mapping.update_ascii_int(key, 0);
            mapping
                .tables
                .remove(key)
                .map(|(key, word)| (key, OwnedWord::from_owned(word, mapping.value_kind)))
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
    // SAFETY: the independent owner roots storage; only an owned cursor and size escape.
    let (position, length) = unsafe {
        owner
            .0
            .with_mut(|s| (s.iteration_position(kind), s.iteration_len(kind)))
    };
    let iterator = MappingIterator {
        owner,
        expected_len: length,
        position,
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

/// Reads distinct string keys directly from their source indexes and sorts the
/// resulting list. Each dictionary already supplies unique keys and their hashes.
///
/// # Safety
/// The array contains two to four rooted dictionaries with exact string keys.
/// No callback or mutation occurs until every result owner has been acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_sorted_string_keys(
    sources: *const *const Object,
    count: usize,
) -> *const Object {
    fn strings(storage: &Storage) -> &Table<Rc<Str>, usize> {
        let Tables::Str(table) = &storage.dictionary().tables else {
            fail("SystemError", "expected a dictionary with string keys")
        };
        table
    }
    // SAFETY: every array element is an immutable, rooted dictionary handle.
    let sources = unsafe { std::slice::from_raw_parts(sources, count) };
    let mut result = super::Words::new(3);
    for (index, &source) in sources.iter().enumerate() {
        let previous = &sources[..index];
        if previous.contains(&source) {
            continue;
        }
        // SAFETY: duplicate handles are skipped, so each nested read is of a
        // distinct immutable table. Only acquired string owners escape the reads.
        let gathered = unsafe {
            object(source).0.with(|storage| {
                let table = strings(storage);
                result
                    .values
                    .try_reserve(table.index.len().saturating_sub(result.values.len()))
                    .ok()?;
                for entry in table.entries.iter().flatten() {
                    if !previous.iter().any(|&other| {
                        object(other).0.with(|storage| {
                            strings(storage).position(entry.hash, &entry.key).is_some()
                        })
                    }) {
                        // Disjoint inputs can exceed the initial reservation.
                        result.values.try_reserve(1).ok()?;
                        result.values.push(entry.key.clone().into_word());
                    }
                }
                Some(())
            })
        };
        if gathered.is_none() {
            super::exceptions::sequence_allocation_failed();
            return std::ptr::null();
        }
    }
    let list = result.into_object();
    // SAFETY: the list has not escaped and owns each distinct immutable key.
    unsafe {
        list.0.with_mut(|storage| {
            let Storage::Sequence(words) = storage else {
                unreachable!()
            };
            super::stdlib::sort_strings(&mut words.values, super::stdlib::StringSort::Distinct);
        });
    }
    Rc::into_raw(list)
}

/// Creates a fresh set from two evaluated key views or sets. A direct view
/// borrows its dictionary here, so effects in the other operand remain visible.
///
/// # Safety
/// Both inputs are rooted dictionaries or sets with the same immutable key kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_key_union(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    let key_kind = |storage: &Storage| match storage {
        Storage::Dictionary(dict) => dict.tables.key_kind(),
        Storage::Set(set) => set.tables.key_kind(),
        _ => fail("SystemError", "expected dictionary keys or a set"),
    };
    // SAFETY: the exact key descriptor is copied from the rooted left operand.
    let kind = unsafe { object(left).0.with(key_kind) };
    let mut result = Tables::new(kind);
    for source in [left, right] {
        // SAFETY: the destination is private and distinct from both inputs. All
        // hash operations use sealed immutable keys and cannot call user code.
        unsafe {
            object(source).0.with(|storage| {
                match storage {
                    Storage::Dictionary(dict) => dict.tables.copy_keys(&mut result),
                    Storage::Set(set) => set.tables.copy_keys(&mut result),
                    _ => unreachable!("the checker proves both key sources"),
                };
            })
        };
    }
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Set(Box::new(
        Set { tables: result },
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
pub unsafe extern "C" fn relar_set_contains(value: *const Object, key: usize) -> u32 {
    // SAFETY: hashing/equality cannot reenter user code.
    u32::from(unsafe {
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
) -> u32 {
    // SAFETY: the rooted iterator snapshots independent owners before returning.
    let next = unsafe {
        object(value).0.with_mut(|storage| match storage {
            Storage::StringIterator(iterator) => Ok(iterator.next()),
            Storage::LineIterator(iterator) => Ok(iterator.next()),
            Storage::BytesIterator(iterator) => Ok(iterator.next()),
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
    use crate::objects::exceptions::{
        relar_error_pending, relar_error_take, relar_exception_matches, relar_exception_message,
    };
    use crate::objects::{
        relar_object_release, relar_seq_append, relar_seq_get_index, relar_seq_new,
    };
    use num_bigint::BigInt;

    fn integer(value: i64) -> usize {
        Int::small(value).unwrap().into_raw()
    }

    #[test]
    fn deeply_nested_tuple_keys_hash_every_field_in_order() {
        use crate::objects::records::{
            RecordDescriptor, RecordSemantics, relar_record_new, relar_record_release,
        };
        use std::hash::DefaultHasher;

        let semantics = RecordSemantics {
            kind: 1,
            flags: 3,
            name: std::ptr::null(),
            name_len: 0,
            qualified_name: std::ptr::null(),
            qualified_name_len: 0,
            fields: std::ptr::null(),
        };
        let kinds = [6 | NULLABLE, 2];
        let layout = RecordDescriptor {
            length: 2,
            kinds: kinds.as_ptr(),
            semantics: &semantics,
        };
        let mut expected = DefaultHasher::new();
        let mut observed = DefaultHasher::new();
        // SAFETY: each tuple transfers its child owner to the next tuple. The
        // leaf's nullable child is zero, and the metadata outlives the whole key.
        unsafe {
            let mut key = relar_record_new(&layout);
            key.add(1).write(integer(7));
            for i in 0..20_000 {
                let parent = relar_record_new(&layout);
                parent.write(key as usize);
                parent.add(1).write(integer(i));
                key = parent;
                std::ptr::from_ref(&layout).hash(&mut expected);
                true.hash(&mut expected);
            }
            std::ptr::from_ref(&layout).hash(&mut expected);
            false.hash(&mut expected);
            IntKey::borrow(integer(7)).hash(&mut expected);
            for i in 0..20_000 {
                IntKey::borrow(integer(i)).hash(&mut expected);
            }
            hash_tuple(key, &mut observed);
            relar_record_release(key);
        }
        assert_eq!(observed.finish(), expected.finish());
    }

    #[test]
    fn cached_hash_collisions_preserve_keys_across_growth_and_compaction() {
        // Force every key into one full-hash collision, including during transfer
        // between independently allocated tables. Keys still compare by value.
        let mut source = Table::<usize, ()>::new();
        let mut result = Table::<usize, ()>::new();
        for key in 1..90 {
            source.insert_absent(42, key, ());
        }
        source.copy_keys(&mut result);
        source.copy_keys(&mut result);
        assert_eq!(result.index.len(), 89);
        for key in 1..70 {
            let position = source.position(42, &key).unwrap();
            let slot = source.index.find_entry(42, |&i| i == position).unwrap();
            slot.remove();
            source.entries[position].take();
        }
        source.compact();
        for (index, key) in (70..90).enumerate() {
            assert_eq!(source.position(42, &key), Some(index));
            assert!(result.position(42, &key).is_some());
        }
        assert!(source.position(42, &69).is_none());
    }

    #[test]
    fn key_union_keeps_the_original_path_and_subscription_keeps_its_list() {
        // SAFETY: both inputs and every returned owner have their exact ABI type.
        unsafe {
            let [original, equal] = ["./a/", "a"].map(|spelling| {
                let text = Rc::new(Str::new(spelling));
                Rc::from_raw(super::super::paths::relar_path_new(Rc::as_ptr(&text)))
            });
            let weak = Rc::downgrade(&original);
            let mapping = Rc::from_raw(relar_default_list_dict_new(5, 3));
            let dict = Rc::as_ptr(&mapping);
            let list =
                Rc::from_raw(relar_dict_get(dict, Rc::as_ptr(&original) as usize) as *const Object);
            let again =
                Rc::from_raw(relar_dict_get(dict, Rc::as_ptr(&equal) as usize) as *const Object);
            assert!(Rc::ptr_eq(&list, &again));
            assert_eq!(Rc::strong_count(&equal), 1);
            let keys = Rc::from_raw(relar_key_union(dict, dict));
            drop(original);
            drop(mapping);
            drop(again);
            assert_eq!(Rc::strong_count(&list), 1);
            assert!(weak.upgrade().is_some());
            assert_eq!(
                relar_set_contains(Rc::as_ptr(&keys), Rc::as_ptr(&equal) as usize),
                1
            );
            drop(keys);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn validated_bytes_gather_matches_text_for_ascii_and_unicode() {
        // SAFETY: each byte buffer is validated while rooted and immutable; the
        // exact dictionary and result owners remain live throughout each use.
        unsafe {
            let dictionary = Rc::from_raw(relar_dict_new(3, 2));
            let pointer = Rc::as_ptr(&dictionary);
            for (index, key) in ["a", "é", "😺"].into_iter().enumerate() {
                let key = Rc::new(Str::new(key));
                relar_dict_set(pointer, Rc::as_ptr(&key) as usize, integer(index as i64));
            }
            for source in ["", "aaa", "aaé😺"] {
                let text = Str::new(source);
                let bytes = Rc::new(source.as_bytes().to_vec());
                let validated = Rc::from_raw(super::super::bytes::relar_bytes_validate_utf8(
                    Rc::as_ptr(&bytes),
                ));
                let from_text = Rc::from_raw(relar_dict_gather_str_int(&text, pointer));
                let from_bytes =
                    Rc::from_raw(relar_dict_gather_bytes_int(Rc::as_ptr(&validated), pointer));
                let words = |list: &List| {
                    list.0
                        .with(|values| values.iter().map(|value| value.0).collect::<Vec<_>>())
                };
                assert_eq!(words(&from_bytes), words(&from_text));
                assert_eq!(Rc::strong_count(&bytes), 2);
            }
        }
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn character_gather_observes_mutations_and_exact_unicode_keys() {
        // SAFETY: constructors transfer their owners to Rc, and all exact string
        // and dictionary inputs remain rooted through each gather/mutation.
        unsafe {
            let dictionary = Rc::from_raw(relar_dict_new(3, 2));
            let pointer = Rc::as_ptr(&dictionary);
            let keys = ["a", "é", "\u{301}", "😺", "\0", "\u{7f}", "\u{80}"]
                .map(|key| Rc::new(Str::new(key)));
            for (index, key) in keys.iter().enumerate() {
                relar_dict_set(pointer, Rc::as_ptr(key) as usize, integer(index as i64));
            }
            let text = Str::new("aaé\u{301}😺\0a\u{7f}\u{80}");
            let gathered = Rc::from_raw(relar_dict_gather_str_int(&text, pointer));
            let values = gathered.0.with(|values| {
                values
                    .iter()
                    .map(|value| value.as_small().unwrap())
                    .collect::<Vec<_>>()
            });
            assert_eq!(values, [0, 0, 1, 2, 3, 4, 0, 5, 6]);
            let ascii_edges =
                Rc::from_raw(relar_dict_gather_str_int(&Str::new("\0\u{7f}"), pointer));
            ascii_edges.0.with(|values| {
                assert_eq!(values[0].as_small(), Some(4));
                assert_eq!(values[1].as_small(), Some(5));
            });
            let alias = dictionary.clone();
            relar_dict_set(
                Rc::as_ptr(&alias),
                Rc::as_ptr(&keys[0]) as usize,
                integer(-7),
            );
            let changed = Rc::from_raw(relar_dict_gather_str_int(&Str::new("aaa"), pointer));
            changed.0.with(|values| {
                assert!(values.iter().all(|value| value.as_small() == Some(-7)));
            });
            relar_dict_delete(pointer, Rc::as_ptr(&keys[0]) as usize);
            assert!(relar_dict_gather_str_int(&Str::new("a"), pointer).is_null());
            let error = Rc::from_raw(relar_error_take());
            assert_eq!(
                relar_exception_matches(Rc::as_ptr(&error), Kind::KeyError as u32),
                1
            );
            let message = Rc::from_raw(relar_exception_message(Rc::as_ptr(&error)));
            assert_eq!(message.as_str(), "'a'");
            relar_dict_set(pointer, Rc::as_ptr(&keys[0]) as usize, integer(29));
            let restored = Rc::from_raw(relar_dict_gather_str_int(&Str::new("a"), pointer));
            restored
                .0
                .with(|values| assert_eq!(values[0].as_small(), Some(29)));
            let empty = Rc::from_raw(relar_dict_gather_str_int(&Str::new(""), pointer));
            empty.0.with(|values| assert!(values.is_empty()));
        }
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn character_gather_owns_big_integers_and_releases_partial_results() {
        let integer = Rc::new(BigInt::from(1_u128 << 120));
        let weak = Rc::downgrade(&integer);
        // SAFETY: dictionary/text handles are exact and rooted. A successful
        // gather transfers one list owner; each failed gather transfers none.
        unsafe {
            let dictionary = Rc::from_raw(relar_dict_new(3, 2));
            let key = Rc::new(Str::new("a"));
            relar_dict_set(
                Rc::as_ptr(&dictionary),
                Rc::as_ptr(&key) as usize,
                Rc::as_ptr(&integer) as usize,
            );
            drop(integer);
            assert_eq!(weak.strong_count(), 1);
            for (text, expected) in [
                ("aa😺z", "'😺'"),
                ("aa\0z", "'\\x00'"),
                ("aa\u{301}z", "'́'"),
            ] {
                let result = relar_dict_gather_str_int(&Str::new(text), Rc::as_ptr(&dictionary));
                assert!(result.is_null());
                assert_eq!(weak.strong_count(), 1);
                assert_eq!(relar_error_pending(), 1);
                let error = Rc::from_raw(relar_error_take());
                assert_eq!(
                    relar_exception_matches(Rc::as_ptr(&error), Kind::KeyError as u32),
                    1
                );
                let message = Rc::from_raw(relar_exception_message(Rc::as_ptr(&error)));
                assert_eq!(message.as_str(), expected);
            }
            let gathered = Rc::from_raw(relar_dict_gather_str_int(
                &Str::new("aaa"),
                Rc::as_ptr(&dictionary),
            ));
            assert_eq!(weak.strong_count(), 4);
            relar_dict_set(
                Rc::as_ptr(&dictionary),
                Rc::as_ptr(&key) as usize,
                Int::from_usize(7).into_raw(),
            );
            assert_eq!(weak.strong_count(), 3);
            let replaced = Rc::from_raw(relar_dict_gather_str_int(
                &Str::new("aaa"),
                Rc::as_ptr(&dictionary),
            ));
            replaced.0.with(|values| {
                assert!(values.iter().all(|value| value.as_small() == Some(7)));
            });
            drop(dictionary);
            assert_eq!(weak.strong_count(), 3);
            gathered.0.with(|values| {
                assert_eq!(values.len(), 3);
                assert!(
                    values
                        .iter()
                        .all(|value| value.big().as_ref() == &BigInt::from(1_u128 << 120))
                );
            });
            drop(gathered);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(relar_error_pending(), 0);
    }

    #[test]
    fn character_gather_tracks_heap_value_transitions_in_its_ascii_cache() {
        let large = Rc::new(BigInt::from(1_u128 << 120));
        let heap_word = Rc::as_ptr(&large) as usize;
        // SAFETY: every key/value remains rooted, and successful results and
        // pending failures each transfer exactly one owner to Rc.
        unsafe {
            let dictionary = Rc::from_raw(relar_dict_new(3, 2));
            let pointer = Rc::as_ptr(&dictionary);
            let keys = ["a", "b", "é"].map(|key| Rc::new(Str::new(key)));
            let count = || {
                dictionary
                    .0
                    .with(|storage| storage.dictionary().ascii_int.as_ref().unwrap().heap_values)
            };
            assert!(relar_dict_gather_str_int(&Str::new("a"), pointer).is_null());
            drop(Rc::from_raw(relar_error_take()));
            assert_eq!(count(), 0);
            for (key, word, expected) in [
                (0, heap_word, 1),
                (0, heap_word, 1),
                (1, heap_word, 2),
                (0, integer(0), 1),
                (2, heap_word, 1),
            ] {
                relar_dict_set(pointer, Rc::as_ptr(&keys[key]) as usize, word);
                assert_eq!(count(), expected);
                let before = Rc::strong_count(&large);
                let gathered =
                    Rc::from_raw(relar_dict_gather_str_int(Rc::as_ptr(&keys[key]), pointer));
                assert_eq!(
                    Rc::strong_count(&large),
                    before + usize::from(word == heap_word)
                );
                gathered.0.with(|values| assert_eq!(values[0].0, word));
                drop(gathered);
                assert_eq!(Rc::strong_count(&large), before);
            }
            relar_dict_delete(pointer, Rc::as_ptr(&keys[1]) as usize);
            assert_eq!(count(), 0);
            relar_dict_set(pointer, Rc::as_ptr(&keys[1]) as usize, heap_word);
            assert_eq!(count(), 1);
            relar_dict_delete(pointer, Rc::as_ptr(&keys[1]) as usize);
            assert_eq!(count(), 0);
            assert!(relar_dict_gather_str_int(&Str::new("aa\0z"), pointer).is_null());
            let error = Rc::from_raw(relar_error_take());
            let message = Rc::from_raw(relar_exception_message(Rc::as_ptr(&error)));
            assert_eq!(message.as_str(), "'\\x00'");
        }
        assert_eq!(Rc::strong_count(&large), 1);
        assert_eq!(relar_error_pending(), 0);
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
            assert!(
                object(dictionary)
                    .0
                    .with(|s| s.dictionary().tables.entry_count())
                    < 179
            );
            relar_object_release(iterator);
            relar_dict_delete(dictionary, integer(0));
            for key in 80..100 {
                relar_dict_delete(dictionary, integer(key));
            }
            assert!(object(dictionary).0.with(|s| {
                let table = &s.dictionary().tables;
                table.entry_count() <= 2 * table.len()
            }));
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
    fn repeated_reinsertion_compacts_with_independent_live_cursors() {
        let dictionary = relar_dict_new(2, 2);
        // SAFETY: the dictionary and both iterators stay rooted. Keys and values
        // are immediate integers and each output slot is initialized on success.
        unsafe {
            for key in 0..4 {
                relar_dict_set(dictionary, integer(key), integer(key));
            }
            let first = relar_dict_iter(dictionary, 0);
            let second = relar_dict_iter(dictionary, 0);
            let next = |iterator| {
                let mut key = 0;
                assert_eq!(
                    relar_mapping_iter_next(iterator, &mut key, std::ptr::null_mut()),
                    1
                );
                Int(key).as_small().unwrap()
            };
            assert_eq!(next(first), 0);
            assert_eq!(next(first), 1);
            assert_eq!(next(second), 0);
            for _ in 0..512 {
                relar_dict_delete(dictionary, integer(0));
                relar_dict_set(dictionary, integer(0), integer(0));
            }
            assert!(
                object(dictionary)
                    .0
                    .with(|s| s.dictionary().tables.entry_count())
                    <= 32
            );
            assert_eq!(next(first), 2);
            assert_eq!(next(second), 1);
            relar_object_release(first);
            relar_object_release(second);
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
