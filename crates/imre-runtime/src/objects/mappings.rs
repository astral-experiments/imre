//! Typed immutable-key hash tables with insertion-ordered dictionary entries.
//!
//! Keys are monomorphized once per table; values are plain owned words with one
//! shared descriptor. Sets store no value words. Each iterator owns a cursor
//! that the table can adjust when it removes vacant entries.

use crate::StrOwner;
use std::fmt::{self, Write};
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::sync::LazyLock;

mod counters;
mod ranking;
mod record_hash;
mod sorted_rows;

use hashbrown::HashTable;
use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::rc::{Rc, Weak};

use super::exceptions::{Error, Kind};
use super::words::{NULLABLE, OwnedWord, WordKind};
use super::{Bytes, Int, List, Object, Storage, StorageCell, Str, fail, object};

pub(super) type StringPairs = Vec<(StrOwner, StrOwner)>;

#[derive(Clone)]
pub(super) struct IntKey(pub(super) Int);

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

/// Path keys compare by spelling; logger keys compare by object identity.
/// Rooting the first inserted object keeps both its spelling and address valid.
#[derive(Clone)]
struct ObjectKey(crate::ObjectOwner);

impl PartialEq for ObjectKey {
    fn eq(&self, other: &Self) -> bool {
        if crate::ObjectOwner::ptr_eq(&self.0, &other.0) {
            return true;
        }
        // SAFETY: both owners root their objects; neither comparison runs callbacks.
        unsafe {
            self.0.0.with(|left| {
                other.0.0.with(|right| match (left, right) {
                    (Storage::Logger(_) | Storage::Socket(_), _)
                    | (_, Storage::Logger(_) | Storage::Socket(_)) => false,
                    (left, right) => left.path_equal(right),
                })
            })
        }
    }
}
impl Eq for ObjectKey {}
impl Hash for ObjectKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // SAFETY: the key roots this object throughout the nonreentrant hash.
        unsafe {
            self.0.0.with(|key| match key {
                Storage::Logger(_) | Storage::Socket(_) => {
                    crate::ObjectOwner::as_ptr(&self.0).hash(state)
                }
                path => path.hash_path(state),
            })
        };
    }
}

/// An immutable tuple or dataclass key roots its concrete record without
/// copying its fields into a second allocation.
struct TupleKey(*const usize);

impl Clone for TupleKey {
    fn clone(&self) -> Self {
        // SAFETY: this key roots one immutable, initialized tuple record.
        unsafe { super::records::imre_record_retain(self.0) };
        Self(self.0)
    }
}
impl Drop for TupleKey {
    fn drop(&mut self) {
        // SAFETY: every nonborrowed key owns one independently acquired record.
        unsafe { super::records::imre_record_release(self.0) };
    }
}
/// # Safety
/// The rooted tuple has only exact immutable scalar, optional, or tuple fields.
unsafe fn hash_tuple<H: Hasher>(value: *const usize, state: &mut H) -> Result<(), Error> {
    let mut pending = Vec::new();
    let mut current = (value, 0);
    'tuples: loop {
        let (value, position) = current;
        // SAFETY: the root owns every tuple in the traversal and its descriptor.
        let layout = unsafe { super::records::descriptor(value) };
        if layout.has_toml_hash() {
            // SAFETY: the key boundary prepares this frozen record's cached hash.
            let cached = unsafe { *value.add(layout.length) };
            debug_assert_ne!(cached, 0);
            cached.hash(state);
            let Some(next) = pending.pop() else {
                return Ok(());
            };
            current = next;
            continue;
        }
        if position == 0 {
            // Tuple equality requires the same descriptor. Hash it once per tuple.
            std::ptr::from_ref(layout).hash(state);
        }
        // SAFETY: a rooted record's semantic metadata is immutable and static.
        let semantics = unsafe { layout.semantics.as_ref() };
        for index in position..layout.length {
            // SAFETY: the static layout covers each initialized concrete field.
            unsafe {
                if semantics.is_some_and(|s| s.kind == 0 && (*s.fields.add(index)).flags & 1 == 0) {
                    continue;
                }
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
                    5 => ObjectKey::borrow(word).hash(state),
                    6 => {
                        if index + 1 < layout.length {
                            pending.try_reserve(1).map_err(|_| Error::memory_error())?;
                            pending.push((value, index + 1));
                        }
                        current = (word as *const usize, 0);
                        continue 'tuples;
                    }
                    7 if layout.is_value_struct() => super::structs::float_key(word).hash(state),
                    8 => (&*(word as *const Bytes)).hash(state),
                    _ => unreachable!("the checker excludes mutable and float tuple keys"),
                }
            }
        }
        let Some(next) = pending.pop() else {
            return Ok(());
        };
        current = next;
    }
}

trait TableKey {
    fn try_hash(&self) -> Result<u64, Error>;

    fn try_eq(&self, other: &Self) -> Result<bool, Error>;
}

impl<T: Eq + Hash> TableKey for T {
    fn try_hash(&self) -> Result<u64, Error> {
        Ok(HASHER.hash_one(self))
    }

    fn try_eq(&self, other: &Self) -> Result<bool, Error> {
        Ok(self == other)
    }
}

impl TableKey for TupleKey {
    fn try_hash(&self) -> Result<u64, Error> {
        let mut state = HASHER.build_hasher();
        // SAFETY: the key roots an immutable checked tuple throughout hashing.
        unsafe { hash_tuple(self.0, &mut state) }?;
        Ok(state.finish())
    }

    fn try_eq(&self, other: &Self) -> Result<bool, Error> {
        // SAFETY: both keys root immutable records of the checked key type.
        unsafe { super::values::try_equal(self.0 as usize, other.0 as usize, WordKind::new(6)) }
    }
}

/// Only the table's exact immutable key representation implements this.
trait ScalarKey: Clone + TableKey {
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
impl ScalarKey for StrOwner {
    const KIND: u32 = 3;
    fn word(&self) -> usize {
        StrOwner::as_ptr(self) as usize
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        // SAFETY: the caller roots an exact string handle; ManuallyDrop does not consume it.
        ManuallyDrop::new(unsafe { StrOwner::from_raw(word as *const Str) })
    }
    unsafe fn from_owned(word: usize) -> Self {
        // SAFETY: the caller transfers an independently acquired string owner.
        unsafe { StrOwner::from_raw(word as *const Str) }
    }
    fn into_word(self) -> usize {
        StrOwner::into_raw(self) as usize
    }
}
impl ScalarKey for crate::BytesOwner {
    const KIND: u32 = 8;
    fn word(&self) -> usize {
        crate::BytesOwner::as_ptr(self) as usize
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        // SAFETY: the caller roots an exact bytes handle; ManuallyDrop does not consume it.
        ManuallyDrop::new(unsafe { crate::BytesOwner::from_raw(word as *const Bytes) })
    }
    unsafe fn from_owned(word: usize) -> Self {
        // SAFETY: the caller transfers an independently acquired bytes owner.
        unsafe { crate::BytesOwner::from_raw(word as *const Bytes) }
    }
    fn into_word(self) -> usize {
        crate::BytesOwner::into_raw(self) as usize
    }
}

impl ScalarKey for ObjectKey {
    const KIND: u32 = 5;
    fn word(&self) -> usize {
        crate::ObjectOwner::as_ptr(&self.0) as usize
    }
    unsafe fn borrow(word: usize) -> ManuallyDrop<Self> {
        // SAFETY: the caller roots the exact object; the borrowed Rc is never dropped.
        ManuallyDrop::new(Self(unsafe {
            crate::ObjectOwner::from_raw(word as *const Object)
        }))
    }
    unsafe fn from_owned(word: usize) -> Self {
        // SAFETY: the caller transfers one independently acquired object owner.
        Self(unsafe { crate::ObjectOwner::from_raw(word as *const Object) })
    }
    fn into_word(self) -> usize {
        crate::ObjectOwner::into_raw(self.0) as usize
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
enum Cursors {
    Iterators(Vec<Weak<Cell<usize>>>),
    Oldest(usize),
}

struct Table<K, V> {
    index: HashTable<usize>,
    entries: Vec<Option<Entry<K, V>>>,
    cursors: Cursors,
}

// Every live entry needs at least eight bytes, including key-only set entries.
// This retains the compiler's common 2^60-1 bound for container lengths.
const _: () = assert!(size_of::<Option<Entry<IntKey, ()>>>() >= 8);
const _: () = assert!(size_of::<Option<Entry<StrOwner, ()>>>() >= 8);

impl<K: ScalarKey, V: Copy> Table<K, V> {
    fn new() -> Self {
        Self {
            index: HashTable::new(),
            entries: Vec::new(),
            cursors: Cursors::Iterators(Vec::new()),
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

    fn key_matches<'a>(
        entries: &'a [Option<Entry<K, V>>],
        hash: u64,
        key: &'a K,
        error: &'a mut Option<Error>,
    ) -> impl FnMut(&usize) -> bool + 'a {
        move |&position| {
            let entry = entries[position].as_ref().unwrap();
            entry.hash == hash
                && entry.key.try_eq(key).unwrap_or_else(|err| {
                    // Stop the bucket probe. Its caller checks the error before using the slot.
                    *error = Some(err);
                    true
                })
        }
    }

    fn key_position(&self, hash: u64, key: &K) -> Result<Option<usize>, Error> {
        let mut error = None;
        let found = self.index.find(
            hash,
            Self::key_matches(&self.entries, hash, key, &mut error),
        );
        error.map_or(Ok(found.copied()), Err)
    }

    fn reserve(&mut self, additional: usize) -> Result<(), Error> {
        let entries = &self.entries;
        self.index
            .try_reserve(additional, |&i| entries[i].as_ref().unwrap().hash)
            .map_err(|_| Error::allocation())?;
        self.entries
            .try_reserve(additional)
            .map_err(|_| Error::allocation())
    }

    fn insert_absent(&mut self, hash: u64, key: K, value: V) -> Result<(), Error> {
        self.compact();
        self.reserve(1)?;
        let position = self.entries.len();
        let entries = &self.entries;
        self.index
            .insert_unique(hash, position, |&i| entries[i].as_ref().unwrap().hash);
        self.entries.push(Some(Entry { hash, key, value }));
        Ok(())
    }

    /// Preserves first-insertion position and returns owners displaced by an update.
    fn insert(&mut self, key: K, value: V) -> Result<Option<(K, V)>, Error> {
        let hash = key.try_hash()?;
        if let Some(position) = self.key_position(hash, &key)? {
            let entry = self.entries[position].as_mut().unwrap();
            return Ok(Some((key, std::mem::replace(&mut entry.value, value))));
        }
        self.insert_absent(hash, key, value)?;
        Ok(None)
    }

    /// Probe an exact rooted key once. Only a miss acquires the table's key owner.
    unsafe fn setdefault(&mut self, word: usize, default: V) -> Result<(V, bool), Error> {
        // SAFETY: the caller roots an exact key of this table's representation.
        let key = unsafe { K::borrow(word) };
        let hash = key.try_hash()?;
        if let Some(index) = self.key_position(hash, &key)? {
            return Ok((self.entries[index].as_ref().unwrap().value, false));
        }
        self.insert_absent(hash, (*key).clone(), default)?;
        Ok((default, true))
    }

    /// Transfers the removed key and value so they can drop outside the mutation.
    fn remove(&mut self, key: &K) -> Result<Option<Entry<K, V>>, Error> {
        let hash = key.try_hash()?;
        let mut error = None;
        let found = self.index.find_entry(
            hash,
            Self::key_matches(&self.entries, hash, key, &mut error),
        );
        if let Some(error) = error {
            return Err(error);
        }
        let Ok(slot) = found else {
            return Ok(None);
        };
        let (position, _) = slot.remove();
        let entry = self.entries[position].take().unwrap();
        self.compact();
        Ok(Some(entry))
    }

    fn compact(&mut self) {
        if self.entries.len() < 32 || self.entries.len() - self.index.len() <= self.index.len() {
            return;
        }
        // Each cursor points just past the last entry it examined. After
        // compaction only the occupied slots in that prefix remain before it.
        match &mut self.cursors {
            Cursors::Iterators(iterators) => iterators.retain(|iterator| {
                let Some(position) = iterator.upgrade() else {
                    return false;
                };
                // After clear, a cursor can still be past the end of a smaller
                // replacement table. Remove only vacant slots before its index.
                let original = position.get();
                let prefix = original.min(self.entries.len());
                position.set(original - prefix + self.entries[..prefix].iter().flatten().count());
                true
            }),
            // Eviction leaves only empty slots before the private cache cursor.
            Cursors::Oldest(position) => *position = 0,
        }
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
    fn copy_keys(&self, result: &mut Table<K, ()>) -> Result<(), Error> {
        result.reserve(self.index.len().saturating_sub(result.index.len()))?;
        for entry in self.entries.iter().flatten() {
            if result.key_position(entry.hash, &entry.key)?.is_none() {
                result.insert_absent(entry.hash, entry.key.clone(), ())?;
            }
        }
        Ok(())
    }

    fn iterator_position(&mut self) -> Rc<Cell<usize>> {
        let Cursors::Iterators(iterators) = &mut self.cursors else {
            unreachable!("function cache tables are private");
        };
        iterators.retain(|iterator| iterator.strong_count() != 0);
        let position = Rc::new(Cell::new(0));
        iterators.push(Rc::downgrade(&position));
        position
    }

    fn eviction_key(&mut self) -> OwnedWord {
        let start = match &self.cursors {
            Cursors::Iterators(iterators) => {
                debug_assert!(iterators.is_empty());
                0
            }
            Cursors::Oldest(position) => *position,
        };
        let (next, key, _) = self.next(start).expect("nonempty bounded cache");
        self.cursors = Cursors::Oldest(next);
        key
    }

    unsafe fn lookup(&self, word: usize) -> Option<&Entry<K, V>>
    where
        K: Eq + Hash,
    {
        // SAFETY: the caller roots this table's exact key representation.
        let key = unsafe { K::borrow(word) };
        self.position(HASHER.hash_one(&*key), &*key)
            .map(|position| self.entries[position].as_ref().unwrap())
    }

    /// Promote a cache hit without dropping or reacquiring its key and value.
    /// Reserve before moving the entry so allocation failure preserves its order.
    unsafe fn promote(&mut self, word: usize) -> Result<bool, Error> {
        // SAFETY: the caller roots a word of this table's key kind.
        let key = unsafe { K::borrow(word) };
        let hash = key.try_hash()?;
        let mut error = None;
        let found = self.index.find_mut(
            hash,
            Self::key_matches(&self.entries, hash, &key, &mut error),
        );
        if let Some(error) = error {
            return Err(error);
        }
        let Some(slot) = found else {
            return Ok(false);
        };
        if *slot + 1 != self.entries.len() {
            self.entries
                .try_reserve(1)
                .map_err(|_| Error::allocation())?;
            let entry = self.entries[*slot].take();
            *slot = self.entries.len();
            self.entries.push(entry);
            self.compact();
        }
        Ok(true)
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
        missing: Missing,
    ) -> Result<Option<OwnedWord>, Error> {
        // SAFETY: the incoming key remains rooted for this whole operation.
        let key = unsafe { K::borrow(word) };
        let hash = key.try_hash()?;
        if let Some(position) = self.key_position(hash, &*key)? {
            let value = self.entries[position].as_ref().unwrap().value;
            // SAFETY: the table roots the stored value of the declared kind.
            return Ok(Some(unsafe { kind.acquire(value) }));
        }
        let element = match missing {
            Missing::Raise | Missing::NoFactory => return Ok(None),
            Missing::Zero => {
                // SAFETY: a Counter has exact integer values. Zero is immediate.
                return Ok(Some(unsafe { kind.acquire(Int::small(0).unwrap().0) }));
            }
            Missing::List(element) => element,
        };
        let list = default_list(element);
        debug_assert_eq!(list.kind, kind);
        self.insert_absent(hash, (*key).clone(), list.word)?;
        // SAFETY: the table now roots the published value.
        Ok(Some(unsafe { kind.acquire(list.into_word()) }))
    }
}

/// Allocate the list factory's result before transferring any owner to a table.
fn default_list(element: WordKind) -> OwnedWord {
    let (word, kind) = if element.raw() == 2 {
        (crate::imre_list_new() as usize, 4)
    } else {
        (super::imre_seq_new(element.raw()) as usize, 5)
    };
    // SAFETY: both constructors return a fresh strong owner of the selected kind.
    unsafe { OwnedWord::from_owned(word, WordKind::new(kind)) }
}

enum Tables<V> {
    Int(Table<IntKey, V>),
    Enum(Table<usize, V>),
    Str(Table<StrOwner, V>),
    Object(Table<ObjectKey, V>),
    Bytes(Table<crate::BytesOwner, V>),
    Tuple(Table<TupleKey, V>),
}

macro_rules! table {
    ($self:expr, $table:ident => $body:expr) => {
        match $self {
            Tables::Int($table) => $body,
            Tables::Enum($table) => $body,
            Tables::Str($table) => $body,
            Tables::Object($table) => $body,
            Tables::Bytes($table) => $body,
            Tables::Tuple($table) => $body,
        }
    };
}

impl<V: Copy> Tables<V> {
    fn try_for_each(
        &self,
        mut visit: impl FnMut(usize, V) -> Result<(), Error>,
    ) -> Result<(), Error> {
        table!(self, table => table.entries.iter().flatten().try_for_each(|entry| visit(entry.key.word(), entry.value)))
    }

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
            5 => Self::Object(Table::new()),
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
            Self::Object(_) => 5,
            Self::Bytes(_) => 8,
            Self::Tuple(_) => 6,
        }
    }
    fn copy_keys(&self, result: &mut Tables<()>) -> Result<(), Error> {
        match (self, result) {
            (Self::Int(table), Tables::Int(result)) => table.copy_keys(result),
            (Self::Enum(table), Tables::Enum(result)) => table.copy_keys(result),
            (Self::Str(table), Tables::Str(result)) => table.copy_keys(result),
            (Self::Object(table), Tables::Object(result)) => table.copy_keys(result),
            (Self::Bytes(table), Tables::Bytes(result)) => table.copy_keys(result),
            (Self::Tuple(table), Tables::Tuple(result)) => table.copy_keys(result),
            _ => unreachable!("key union requires the same key kind"),
        }
    }
    fn len(&self) -> usize {
        table!(self, table => table.index.len())
    }
    /// Compare live keys in insertion order, ignoring holes left by deletions.
    fn same_key_order(&self, other: &Self) -> Result<bool, Error> {
        fn same<K: TableKey, V>(a: &Table<K, V>, b: &Table<K, V>) -> Result<bool, Error> {
            if a.index.len() != b.index.len() {
                return Ok(false);
            }
            for (left, right) in a.entries.iter().flatten().zip(b.entries.iter().flatten()) {
                if !left.key.try_eq(&right.key)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => same(a, b),
            (Self::Enum(a), Self::Enum(b)) => same(a, b),
            (Self::Str(a), Self::Str(b)) => same(a, b),
            (Self::Object(a), Self::Object(b)) => same(a, b),
            (Self::Bytes(a), Self::Bytes(b)) => same(a, b),
            (Self::Tuple(a), Self::Tuple(b)) => same(a, b),
            _ => Ok(false),
        }
    }
    #[cfg(test)]
    fn entry_count(&self) -> usize {
        table!(self, table => table.entries.len())
    }
    fn iterator_position(&mut self) -> Rc<Cell<usize>> {
        table!(self, table => table.iterator_position())
    }
    unsafe fn try_lookup(&self, word: usize) -> Result<Option<V>, Error> {
        unsafe fn find<K: ScalarKey, V: Copy>(
            table: &Table<K, V>,
            word: usize,
        ) -> Result<Option<V>, Error> {
            // SAFETY: the caller roots the exact key kind of the dispatched table.
            let key = unsafe { K::borrow(word) };
            let position = table.key_position(key.try_hash()?, &key)?;
            Ok(position.map(|index| table.entries[index].as_ref().unwrap().value))
        }
        // SAFETY: each table borrows its exact rooted key kind.
        table!(self, table => unsafe { find(table, word) })
    }
    unsafe fn lookup(&self, word: usize) -> Option<V> {
        // SAFETY: the caller roots the key for this table.
        unsafe { self.try_lookup(word) }.unwrap_or_else(|error| error.returning(None))
    }
    fn insert(&mut self, key: OwnedWord, value: V) -> Result<Option<(OwnedWord, V)>, Error> {
        fn insert<K: ScalarKey, V: Copy>(
            table: &mut Table<K, V>,
            key: OwnedWord,
            value: V,
        ) -> Result<Option<(OwnedWord, V)>, Error> {
            assert_eq!(key.kind.raw(), K::KIND);
            // SAFETY: the key temporary transfers one exact typed owner.
            let key = unsafe { K::from_owned(key.into_word()) };
            Ok(table.insert(key, value)?.map(|(key, value)| {
                // SAFETY: insert transfers the unused incoming key reference.
                (
                    unsafe { OwnedWord::from_owned(key.into_word(), WordKind::new(K::KIND)) },
                    value,
                )
            }))
        }
        table!(self, table => insert(table, key, value))
    }
    unsafe fn remove(&mut self, word: usize) -> Result<Option<(OwnedWord, V)>, Error> {
        unsafe fn remove<K: ScalarKey, V: Copy>(
            table: &mut Table<K, V>,
            word: usize,
        ) -> Result<Option<(OwnedWord, V)>, Error> {
            // SAFETY: the caller roots an exact key of this table's kind.
            let key = unsafe { K::borrow(word) };
            Ok(table.remove(&key)?.map(|entry| {
                // SAFETY: the table transferred the stored key owner.
                unsafe {
                    (
                        OwnedWord::from_owned(entry.key.into_word(), WordKind::new(K::KIND)),
                        entry.value,
                    )
                }
            }))
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Missing {
    Raise,
    NoFactory,
    List(WordKind),
    Zero,
}

pub(super) trait ReleaseEntries {
    fn needs_queue(&self) -> bool;
    fn release_next(&mut self, next: &mut usize) -> bool;
}

/// Keeps the existing allocation available for incremental destruction.
pub(super) struct TableOwner<T: ReleaseEntries + 'static>(Option<Box<T>>);

impl<T: ReleaseEntries + 'static> TableOwner<T> {
    pub(super) fn new(table: T) -> Self {
        Self(Some(Box::new(table)))
    }
}

impl<T: ReleaseEntries + 'static> std::ops::Deref for TableOwner<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_deref().expect("table present until drop")
    }
}

impl<T: ReleaseEntries + 'static> std::ops::DerefMut for TableOwner<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_deref_mut().expect("table present until drop")
    }
}

impl<T: ReleaseEntries + 'static> Drop for TableOwner<T> {
    fn drop(&mut self) {
        let table = self.0.take().expect("table present until drop");
        if table.needs_queue() {
            super::records::release_table(table);
        }
    }
}

pub(super) struct Mapping {
    tables: Tables<usize>,
    value_kind: WordKind,
    /// Only subscription consults this policy; membership and get inspect stored keys.
    missing: Missing,
    /// An OrderedDict iterator detects key changes even if its size is restored.
    order_version: Option<usize>,
    /// Present only after a string-to-integer gather. Words borrow current table
    /// values; ASCII mutation updates the corresponding slot before old owners drop.
    ascii_int: Option<Box<AsciiIntCache>>,
}
pub(super) struct Set {
    tables: Tables<()>,
    frozen: bool,
}

impl Mapping {
    /// Render an unquoted attribute label and borrow its value from the rooted dictionary.
    pub(super) fn namespace_repr_entry(
        &self,
        output: &mut super::values::ReprWriter<'_>,
        position: usize,
    ) -> Result<Option<(usize, usize, WordKind)>, std::fmt::Error> {
        use std::fmt::Write as _;
        if position == 0 {
            output.write_str("namespace(")?;
        }
        let mut cursor = position;
        while let Some((next, key, value)) = self.tables.next(cursor) {
            cursor = next;
            // SAFETY: namespace constructors only accept exact string keys.
            let label = unsafe { (&*(key.word as *const Str)).as_str() };
            if label.is_empty() {
                continue;
            }
            if position != 0 {
                output.write_str(", ")?;
            }
            output.write_str(label)?;
            output.write_char('=')?;
            return Ok(Some((next, value, self.value_kind)));
        }
        output.write_char(')')?;
        Ok(None)
    }

    /// Copies entries in insertion order, retains immutable keys, and lets the
    /// caller choose whether each value is shared or copied through a memo.
    pub(super) fn copy_values(
        &self,
        mut value_copy: impl FnMut(OwnedWord) -> Result<OwnedWord, Error>,
    ) -> Result<Self, Error> {
        let mut copy = Self::new(self.tables.key_kind(), self.value_kind.raw());
        copy.missing = self.missing;
        let mut position = 0;
        while let Some((next, key, value)) = self.tables.next(position) {
            // SAFETY: the source mapping roots this value of the exact kind.
            let value = value_copy(unsafe { self.value_kind.acquire(value) })?;
            copy.insert(key, value)?;
            position = next;
        }
        copy.order_version = self.order_version.map(|_| 0);
        Ok(copy)
    }

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

    /// Keeps exact field owners alive after a string-key dictionary borrow ends.
    pub(super) fn owned_string_entries(&self) -> Vec<(String, OwnedWord)> {
        self.string_entries()
            .map(|(key, word)| {
                // SAFETY: the dictionary owns this value under its homogeneous descriptor.
                (key.to_string(), unsafe { self.value_kind.acquire(word) })
            })
            .collect()
    }

    /// A numeric-key dictionary cannot match a string placeholder.
    pub(super) fn logging_key(&self, key: &Str) -> Option<OwnedWord> {
        let Tables::Str(table) = &self.tables else {
            return None;
        };
        // SAFETY: the table and the lookup key are both exact strings under live borrows.
        unsafe {
            table
                .lookup(std::ptr::from_ref(key) as usize)
                .map(|e| self.value_kind.acquire(e.value))
        }
    }

    /// Look up an integer through an object view without reading a string-key
    /// table as a tagged integer. The result retains its exact value descriptor.
    pub(super) fn integer_word(&self, key: &Int) -> Option<OwnedWord> {
        let Tables::Int(table) = &self.tables else {
            return None;
        };
        // SAFETY: the caller roots an exact integer; the table owns the value.
        unsafe {
            table
                .lookup(key.0)
                .map(|entry| self.value_kind.acquire(entry.value))
        }
    }

    /// Subscribe with a dynamically represented key, preserving the mapping's missing-key policy.
    pub(super) fn subscript_value(&mut self, key: &OwnedWord) -> Result<Option<OwnedWord>, Error> {
        if key.kind.raw() != self.tables.key_kind() {
            return Ok(matches!(self.missing, Missing::Zero).then(|| {
                // SAFETY: a Counter has integer values and zero is immediate.
                unsafe { self.value_kind.acquire(Int::small(0).unwrap().0) }
            }));
        }
        // SAFETY: the rooted key matches the table's exact representation.
        unsafe { self.subscript(key.word) }
    }

    /// The caller roots a key with this table's exact representation.
    unsafe fn subscript(&mut self, key: usize) -> Result<Option<OwnedWord>, Error> {
        // SAFETY: the table owns its values and only a sealed list factory can run.
        unsafe {
            table!(&mut self.tables, table => table.subscript(key, self.value_kind, self.missing))
        }
    }

    pub(super) fn integer_entries(&self) -> impl Iterator<Item = (&Int, usize)> {
        let Tables::Int(table) = &self.tables else {
            fail("SystemError", "expected an integer-key dictionary");
        };
        table
            .entries
            .iter()
            .flatten()
            .map(|entry| (&entry.key.0, entry.value))
    }

    /// A character ordinal probes only an exact integer-key table. Returned words
    /// borrow this immutable mapping; string, bytes, and tuple keys never match.
    pub(super) fn translation_entry(&self, ch: char) -> Option<(WordKind, usize)> {
        let Tables::Int(table) = &self.tables else {
            return None;
        };
        let key = Int::small(i64::from(u32::from(ch))).unwrap().into_raw();
        // SAFETY: every Unicode ordinal is a tagged immediate of this table's key type.
        unsafe { table.lookup(key) }.map(|entry| (self.value_kind, entry.value))
    }

    /// Transfers two exact owners only for a new key. Duplicate inputs are
    /// dropped together, preserving existing entries during strict decoding.
    pub(super) fn insert_unique(
        &mut self,
        key: OwnedWord,
        value: OwnedWord,
    ) -> Result<bool, Error> {
        assert_eq!(key.kind.raw(), self.tables.key_kind());
        assert_eq!(value.kind, self.value_kind);
        // SAFETY: the temporary key roots this table's exact immutable key kind.
        if unsafe { self.tables.try_lookup(key.word) }?.is_some() {
            return Ok(false);
        }
        let displaced = self.insert(key, value)?;
        debug_assert!(displaced.is_none());
        Ok(true)
    }

    /// Transfer the value and publish its cache slot only after the table owns it.
    fn insert(
        &mut self,
        key: OwnedWord,
        value: OwnedWord,
    ) -> Result<Option<(OwnedWord, OwnedWord)>, Error> {
        let key_word = key.word;
        let old = self.tables.insert(key, value.word)?;
        if old.is_none() {
            self.keys_changed();
        }
        let word = value.into_word();
        // SAFETY: the table or the displaced pair roots the key. The table owns
        // the new value, and each displaced word transfers its stored owner.
        unsafe {
            self.update_ascii_int(key_word, word);
            Ok(old.map(|(key, word)| (key, OwnedWord::from_owned(word, self.value_kind))))
        }
    }

    fn keys_changed(&mut self) {
        if let Some(version) = &mut self.order_version {
            *version = version.wrapping_add(1);
        }
    }

    /// Retains exact string entries in insertion order while preparing a process launch.
    /// No borrowed table storage or language owner crosses the fork boundary.
    pub(super) fn string_pairs(&self) -> Result<StringPairs, Error> {
        if self.tables.key_kind() != 3 || self.value_kind.raw() != 3 {
            fail("SystemError", "expected a string-to-string dictionary");
        }
        let mut pairs = Vec::new();
        pairs
            .try_reserve_exact(self.tables.len())
            .map_err(|_| Error::allocation())?;
        self.tables.all(|key, value| {
            // SAFETY: the checked exact dictionary roots both immutable strings.
            unsafe {
                pairs.push((
                    crate::StrOwner::acquire_word(key),
                    crate::StrOwner::acquire_word(value),
                ));
            }
            true
        });
        Ok(pairs)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.tables.len() == 0
    }

    /// A typed dict of None can initialize a mutable document's removal values.
    pub(super) fn document_null_keys(&self) -> Vec<StrOwner> {
        assert_eq!((self.tables.key_kind(), self.value_kind.raw()), (3, 0));
        let mut keys = Vec::with_capacity(self.tables.len());
        self.tables.all(|key, _| {
            // SAFETY: the mapping holds a strong reference to each exact string key.
            keys.push(unsafe { crate::StrOwner::acquire_word(key) });
            true
        });
        keys
    }

    /// Borrowed document entries stay rooted by the unmodified mapping.
    pub(super) fn document_entries(&self) -> impl Iterator<Item = (&Str, usize)> + '_ {
        assert_eq!(self.value_kind.raw(), 5);
        let Tables::Str(table) = &self.tables else {
            unreachable!("expected a string-keyed document");
        };
        table
            .entries
            .iter()
            .flatten()
            .map(|entry| (entry.key.as_ref(), entry.value))
    }

    pub(super) fn document_pairs(&self) -> Vec<(StrOwner, crate::ObjectOwner)> {
        assert_eq!((self.tables.key_kind(), self.value_kind.raw()), (3, 5));
        let mut pairs = Vec::with_capacity(self.tables.len());
        self.tables.all(|key, value| {
            // SAFETY: the exact native mapping roots each typed key and value.
            pairs.push(unsafe {
                (
                    crate::StrOwner::acquire_word(key),
                    crate::ObjectOwner::acquire_word(value),
                )
            });
            true
        });
        pairs
    }

    /// Compares the counts at every key in the union, including keys stored with
    /// negative counts on only one side. Missing counts are zero.
    ///
    /// # Safety
    /// Both mappings are rooted Counters with exact integer values.
    unsafe fn counter_counts_all(&self, other: &Self, mut check: impl FnMut(i32) -> bool) -> bool {
        let shared_kind = self.tables.key_kind() == other.tables.key_kind();
        let zero = Int::small(0).unwrap().0;
        // SAFETY: equal table kinds share a key representation, and both tables
        // root their borrowed keys and counts throughout these immutable visits.
        unsafe {
            self.tables.all(|key, left| {
                let right = if shared_kind {
                    other.tables.lookup(key)
                } else {
                    None
                };
                check(crate::imre_int_compare(left, right.unwrap_or(zero)))
            }) && other.tables.all(|key, right| {
                (shared_kind && self.tables.lookup(key).is_some())
                    || check(crate::imre_int_compare(zero, right))
            })
        }
    }

    /// # Safety
    /// Both tables and all values are rooted and immutable for this traversal.
    pub(super) unsafe fn compare_entry(
        &self,
        other: &Self,
        position: usize,
    ) -> super::values::Comparison {
        use super::values::Comparison;
        if self.missing == Missing::Zero && other.missing == Missing::Zero {
            // Counts of absent keys are zero in either direction, including
            // for another Counter nested in a record or in a sequence.
            // SAFETY: the missing-value policy identifies two exact Counters.
            return Comparison::Done(unsafe { self.counter_counts_all(other, |order| order == 0) });
        }
        if self.tables.key_kind() != other.tables.key_kind()
            || self.value_kind != other.value_kind
            || self.tables.len() != other.tables.len()
        {
            return Comparison::Done(false);
        }
        if position == 0 && self.order_version.is_some() && other.order_version.is_some() {
            match self.tables.same_key_order(&other.tables) {
                Ok(true) => {}
                Ok(false) => return Comparison::Done(false),
                Err(error) => return Comparison::Error(error),
            }
        }
        let Some((next, key, left)) = self.tables.next(position) else {
            return Comparison::Done(true);
        };
        // SAFETY: table kinds match and both tables retain their entries.
        match unsafe { other.tables.try_lookup(key.word) } {
            Ok(Some(right)) => Comparison::Child {
                left,
                right,
                kind: self.value_kind,
                resume: Some(next),
            },
            Ok(None) => Comparison::Done(false),
            Err(error) => Comparison::Error(error),
        }
    }

    /// Copies the next entry while the original table roots both words.
    pub(super) fn repr_entry(
        &self,
        output: &mut super::values::ReprWriter<'_>,
        position: usize,
    ) -> Result<Option<super::values::ReprEntry>, fmt::Error> {
        if self.missing == Missing::Zero {
            self.write_counter_repr(output)?;
            return Ok(None);
        }
        if position == 0 {
            if self.order_version.is_some() {
                output.write_str("OrderedDict(")?;
                if self.tables.len() == 0 {
                    output.write_char(')')?;
                    return Ok(None);
                }
            }
            match self.missing {
                Missing::List(_) => output.write_str("defaultdict(<class 'list'>, ")?,
                Missing::NoFactory => output.write_str("defaultdict(None, ")?,
                _ => {}
            }
            output.write_char('{')?;
        }
        let Some((next, key, value)) = self.tables.next(position) else {
            output.write_char('}')?;
            if self.order_version.is_some()
                || matches!(self.missing, Missing::List(_) | Missing::NoFactory)
            {
                output.write_char(')')?;
            }
            return Ok(None);
        };
        if position != 0 {
            output.write_str(", ")?;
        }
        Ok(Some(super::values::ReprEntry {
            next,
            key: (key.word, key.kind),
            value: Some((value, self.value_kind)),
        }))
    }

    fn write_counter_repr(&self, output: &mut impl Write) -> fmt::Result {
        if self.tables.len() == 0 {
            return output.write_str("Counter()");
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(self.tables.len())
            .map_err(|_| fmt::Error)?;
        self.tables.all(|key, count| {
            entries.push((key, count, entries.len()));
            true
        });
        // The ordinal preserves insertion order for ties without allocating sort scratch.
        entries.sort_unstable_by(|a, b| {
            // SAFETY: the immutable table roots both exact integer counts.
            unsafe { crate::imre_int_compare(b.1, a.1) }
                .cmp(&0)
                .then_with(|| a.2.cmp(&b.2))
        });
        output.write_str("Counter({")?;
        for (index, (key, count, _)) in entries.into_iter().enumerate() {
            if index != 0 {
                output.write_str(", ")?;
            }
            // SAFETY: a Counter roots its exact integer or string keys and integer counts.
            unsafe {
                if self.tables.key_kind() == 2 {
                    super::values::write_int_repr(output, key)?;
                } else {
                    super::exceptions::write_string_repr(&*(key as *const Str), output)?;
                }
                output.write_str(": ")?;
                super::values::write_int_repr(output, count)?;
            }
        }
        output.write_str("})")
    }

    pub(super) fn new(key_kind: u32, value_kind: u32) -> Self {
        Self {
            tables: Tables::new(key_kind),
            value_kind: WordKind::new(value_kind),
            missing: Missing::Raise,
            order_version: None,
            ascii_int: None,
        }
    }

    /// Detach entries and cached values before releasing any key or value. A
    /// live cursor keeps its numeric position if the dictionary is refilled.
    fn take_contents(&mut self) -> Self {
        let mut empty = Self::new(self.tables.key_kind(), self.value_kind.raw());
        empty.missing = self.missing;
        empty.order_version = self.order_version;
        if self.tables.len() != 0 {
            empty.keys_changed();
        }
        let cursors = table!(&mut self.tables, table => {
            std::mem::replace(&mut table.cursors, Cursors::Iterators(Vec::new()))
        });
        if matches!(cursors, Cursors::Iterators(_)) {
            table!(&mut empty.tables, table => table.cursors = cursors);
        }
        std::mem::replace(self, empty)
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
    pub(super) fn is_empty(&self) -> bool {
        self.tables.len() == 0
    }

    fn mutable_tables(&mut self, method: &str) -> Result<&mut Tables<()>, Error> {
        if self.frozen {
            return Err(Error::new(
                Kind::AttributeError,
                format!("'frozenset' object has no attribute '{method}'"),
            ));
        }
        Ok(&mut self.tables)
    }

    /// Transfer exact string keys into a new, immutable set.
    fn frozen_strings(
        keys: impl Iterator<Item = Result<OwnedWord, Error>>,
    ) -> Result<crate::ObjectOwner, Error> {
        let mut tables = Tables::new(3);
        for key in keys {
            tables.insert(key?, ())?;
        }
        crate::ObjectOwner::try_new(Object(StorageCell::new(Storage::Set(TableOwner::new(
            Self {
                tables,
                frozen: true,
            },
        )))))
        .map_err(|_| Error::allocation())
    }

    /// Deep-copy admission proves each set member is unchanged by deepcopy.
    pub(super) fn copy_keys(&self) -> Result<Self, Error> {
        let mut tables = Tables::new(self.tables.key_kind());
        self.tables.copy_keys(&mut tables)?;
        Ok(Self {
            tables,
            frozen: self.frozen,
        })
    }

    /// Borrow these integer keys while the caller copies a child descriptor set.
    pub(super) fn integers(&self) -> impl Iterator<Item = &Int> {
        let Tables::Int(table) = &self.tables else {
            fail("SystemError", "expected a native integer set");
        };
        table.entries.iter().flatten().map(|entry| &entry.key.0)
    }

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
        output: &mut super::values::ReprWriter<'_>,
        position: usize,
    ) -> Result<Option<super::values::ReprEntry>, std::fmt::Error> {
        use std::fmt::Write as _;
        if position == 0 {
            if self.tables.len() == 0 {
                output.write_str(if self.frozen { "frozenset()" } else { "set()" })?;
                return Ok(None);
            }
            if self.frozen {
                output.write_str("frozenset(")?;
            }
            output.write_char('{')?;
        }
        let Some((next, key, ())) = self.tables.next(position) else {
            output.write_str(if self.frozen { "})" } else { "}" })?;
            return Ok(None);
        };
        if position != 0 {
            output.write_str(", ")?;
        }
        Ok(Some(super::values::ReprEntry {
            next,
            key: (key.word, key.kind),
            value: None,
        }))
    }
}
fn release_next_key<K, V>(entries: &mut [Option<Entry<K, V>>], next: &mut usize) -> bool {
    for slot in &mut entries[*next..] {
        *next += 1;
        if slot.take().is_some() {
            return true;
        }
    }
    false
}

impl ReleaseEntries for Mapping {
    fn needs_queue(&self) -> bool {
        self.tables.len() > 3
            && (self.value_kind.can_queue_release() || matches!(self.tables, Tables::Tuple(_)))
    }

    fn release_next(&mut self, next: &mut usize) -> bool {
        let tuple_keys = matches!(self.tables, Tables::Tuple(_));
        table!(&mut self.tables, table => {
            let len = table.entries.len();
            while *next < len {
                let slot = &mut table.entries[*next];
                *next += 1;
                if let Some(entry) = slot {
                    // SAFETY: zero marks the dictionary owner as transferred.
                    unsafe { self.value_kind.release(std::mem::take(&mut entry.value)) };
                    if self.value_kind.can_queue_release() {
                        return true;
                    }
                }
            }
            // Dictionary values precede keys, both in insertion order.
            let mut key = *next - len;
            let pending = tuple_keys && release_next_key(&mut table.entries, &mut key);
            *next = len + key;
            pending
        })
    }
}

impl ReleaseEntries for Set {
    fn needs_queue(&self) -> bool {
        self.tables.len() > 3 && matches!(self.tables, Tables::Tuple(_))
    }

    fn release_next(&mut self, next: &mut usize) -> bool {
        let Tables::Tuple(table) = &mut self.tables else {
            unreachable!()
        };
        release_next_key(&mut table.entries, next)
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
    OrderedDictMutation,
    SetSize,
}
impl IterationError {
    fn message(self) -> &'static str {
        match self {
            Self::DictionarySize => "dictionary changed size during iteration",
            Self::DictionaryKeys => "dictionary keys changed during iteration",
            Self::OrderedDictMutation => "OrderedDict mutated during iteration",
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
    owner: crate::ObjectOwner,
    /// Size for a dict or set; key version for an OrderedDict.
    expected: usize,
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
                if self.kind != IteratorKind::Set
                    && let Some(version) = storage.dictionary().order_version
                {
                    if self.remaining == 0 {
                        return Ok(None);
                    }
                    if version != self.expected {
                        return Err(IterationError::OrderedDictMutation);
                    }
                } else if storage.iteration_len(self.kind) != self.expected {
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
                if matches!(
                    error,
                    IterationError::DictionaryKeys | IterationError::OrderedDictMutation
                ) {
                    self.exhausted = true;
                } else {
                    self.expected = usize::MAX;
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
    /// Borrow at most two keys in normal iteration order. The words stay rooted
    /// by this storage borrow. This avoids creating an iterator to unpack a pair.
    pub(super) fn first_two_keys(&self) -> (usize, u32, [usize; 2]) {
        fn keys<V: Copy>(table: &Tables<V>) -> (usize, u32, [usize; 2]) {
            let mut words = [0; 2];
            let size = table.len();
            if size == 2 {
                let mut dest = words.iter_mut();
                table.all(|key, _| {
                    *dest.next().unwrap() = key;
                    true
                });
            }
            (size, table.key_kind(), words)
        }
        match self {
            Self::Dictionary(dict) => keys(&dict.tables),
            Self::Set(set) => keys(&set.tables),
            _ => unreachable!("checked mapping or set"),
        }
    }

    pub(super) fn dictionary(&self) -> &Mapping {
        match self {
            Self::Dictionary(value) => value,
            _ => fail("SystemError", "expected a dictionary object"),
        }
    }
    /// Length of a mapping or set retained through a boxed Python value.
    pub(super) fn mapping_or_set_len(&self) -> Option<usize> {
        match self {
            Self::Dictionary(mapping) => Some(mapping.tables.len()),
            Self::Set(set) => Some(set.tables.len()),
            _ => None,
        }
    }

    /// Subscribe with a rooted Python int without interpreting it as a different table key kind.
    pub(super) unsafe fn integer_subscript(
        &mut self,
        key: usize,
    ) -> Option<Result<OwnedWord, Error>> {
        let Self::Dictionary(mapping) = self else {
            return None;
        };
        // SAFETY: the caller keeps its native integer alive until the boxed key acquires it.
        let key = unsafe { WordKind::new(2).acquire(key) };
        Some(
            mapping
                .subscript_value(&key)
                .and_then(|item| item.ok_or_else(|| Error::key(key))),
        )
    }
    pub(super) fn dictionary_mut(&mut self) -> &mut Mapping {
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

pub(super) struct FunctionCache {
    table: crate::ObjectOwner,
    hits: usize,
    misses: usize,
}

impl FunctionCache {
    fn new(key: u32, value: u32) -> Self {
        Self {
            // SAFETY: imre_dict_new returns an independent strong owner.
            table: unsafe { crate::ObjectOwner::from_raw(imre_dict_new(key, value)) },
            hits: 0,
            misses: 0,
        }
    }

    unsafe fn probe(&mut self, key: usize, capacity: i64) -> Result<bool, Error> {
        let hit = match capacity {
            0 => false,
            // SAFETY: the wrapper preserves this key's owner and layout.
            -1 => unsafe {
                self.table
                    .0
                    .with(|s| s.dictionary().tables.try_lookup(key))?
                    .is_some()
            },
            // SAFETY: only the private insertion order changes; no user code can run.
            _ => unsafe {
                self.table.0.with_mut(
                    |s| table!(&mut s.dictionary_mut().tables, table => table.promote(key)),
                )?
            },
        };
        if hit {
            self.hits += 1
        } else {
            self.misses += 1
        }
        Ok(hit)
    }

    unsafe fn insert(
        &mut self,
        key: usize,
        word: usize,
        capacity: usize,
    ) -> Result<Option<(OwnedWord, OwnedWord)>, Error> {
        // SAFETY: the caller roots both exact operands; displaced owners escape the borrow.
        unsafe {
            self.table.0.with_mut(|s| {
                let mapping = s.dictionary_mut();
                if mapping.tables.try_lookup(key)?.is_some() {
                    return Ok(None);
                }
                let key = WordKind::new(mapping.tables.key_kind()).acquire(key);
                let value = mapping.value_kind.acquire(word);
                let old = mapping.insert(key, value)?;
                debug_assert!(old.is_none());
                if mapping.tables.len() <= capacity {
                    return Ok(None);
                }

                // Reserve and insert first: an allocation error must not evict the oldest entry.
                let table = &mut mapping.tables;
                let oldest = table!(table, table => table.eviction_key());
                let (key, word) = table.remove(oldest.word)?.unwrap();
                mapping.update_ascii_int(key.word, 0);
                Ok(Some((key, OwnedWord::from_owned(word, mapping.value_kind))))
            })
        }
    }

    fn stat(&self, field: u32) -> usize {
        match field {
            0 => self.hits,
            1 => self.misses,
            // SAFETY: the registry roots this private table and no callbacks occur.
            3 => unsafe {
                self.table
                    .0
                    .with(|storage| storage.dictionary().tables.len())
            },
            _ => unreachable!("invalid cache statistic"),
        }
    }
}

thread_local! {
    static FUNCTION_CACHES: std::cell::RefCell<std::collections::HashMap<usize, crate::ObjectOwner>> =
        std::cell::RefCell::default();
}

/// Module decorators live for the program; dynamic ones own this same state directly.
#[unsafe(no_mangle)]
pub extern "C" fn imre_module_cache(id: usize, key: u32, value: u32) -> *const Object {
    FUNCTION_CACHES.with_borrow_mut(|caches| {
        let cache = caches.entry(id).or_insert_with(|| {
            // SAFETY: creation transfers an independent strong reference to the registry.
            unsafe { crate::ObjectOwner::from_raw(imre_cache_new(key, value)) }
        });
        crate::ObjectOwner::into_raw(cache.clone())
    })
}

/// Each application of a decorator owns its own counters, table and recency order.
#[unsafe(no_mangle)]
pub extern "C" fn imre_cache_new(key: u32, value: u32) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::FunctionCache(Box::new(FunctionCache::new(key, value))),
    ))))
}

impl Storage {
    fn cache(&self) -> &FunctionCache {
        match self {
            Self::FunctionCache(cache) => cache,
            _ => unreachable!("expected a private function cache"),
        }
    }

    fn cache_mut(&mut self) -> &mut FunctionCache {
        match self {
            Self::FunctionCache(cache) => cache,
            _ => unreachable!("expected a private function cache"),
        }
    }
}

/// # Safety
/// The caller roots a live private cache state for this read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_table(state: *const Object) -> *const Object {
    // SAFETY: an independent table owner leaves the borrow.
    crate::ObjectOwner::into_raw(unsafe { object(state).0.with(|s| s.cache().table.clone()) })
}

/// # Safety
/// Both the private state and its exact key are rooted through the lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_probe(state: *const Object, key: usize, capacity: i64) -> bool {
    // SAFETY: lookup preserves both owners and does not invoke user code.
    crate::finish(
        unsafe {
            object(state)
                .0
                .with_mut(|s| s.cache_mut().probe(key, capacity))
        },
        false,
    )
}

/// # Safety
/// State, key and value are live and have the kinds passed to the constructor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_insert(
    state: *const Object,
    key: usize,
    word: usize,
    capacity: usize,
) {
    // SAFETY: eviction transfers owners out of the storage borrow.
    let evicted = unsafe {
        object(state)
            .0
            .with_mut(|s| s.cache_mut().insert(key, word, capacity))
    };
    drop(crate::finish(evicted, None));
}

/// # Safety
/// The caller roots a private cache; field is hits, misses or current size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_stat(state: *const Object, field: u32) -> usize {
    // SAFETY: this read returns a plain count.
    Int::from_usize(unsafe { object(state).0.with(|s| s.cache().stat(field)) }).into_raw()
}

/// # Safety
/// The state belongs to a wrapper with the specified key and value kinds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cache_clear(state: *const Object, key: u32, value: u32) {
    let empty = FunctionCache::new(key, value);
    // SAFETY: displaced entries drop after the new state is installed and the borrow ends.
    drop(unsafe {
        object(state)
            .0
            .with_mut(|s| std::mem::replace(s.cache_mut(), empty))
    });
}

/// Look up one value lazily through a read-only string-key mapping view.
///
/// # Safety
/// Both handles are rooted; the mapping stores supported concrete or boxed values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mapping_object_get(
    value: *const Object,
    key: *const Str,
) -> *const Object {
    // SAFETY: the shared dictionary borrow only retains the found value.
    let found = unsafe { object(value).0.with(|s| s.dictionary().logging_key(&*key)) };
    let Some(field) = found else {
        // SAFETY: the same dictionary and string key remain rooted.
        return unsafe { missing_key(value, key as usize) }.returning(std::ptr::null());
    };
    if field.kind.raw() == 5 {
        // The only accepted object-handle value layout is the existing box.
        return field.into_word() as *const Object;
    }
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::ObjectValue(field),
    ))))
}

/// Creates a dictionary with concrete scalar keys and one shared value descriptor.
#[unsafe(no_mangle)]
pub extern "C" fn imre_dict_new(key_kind: u32, value_kind: u32) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Dictionary(TableOwner::new(Mapping::new(key_kind, value_kind))),
    ))))
}

/// Select the bound dictionary's class before consuming the first key.
///
/// # Safety
/// The caller roots both mappings. `fresh` is a new, unreachable plain
/// dictionary with the result's key and value types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_fromkeys_target(
    owner: *const Object,
    fresh: *const Object,
) -> *const Object {
    // SAFETY: only the receiver's class and missing-key policy leave its borrow.
    let (policy, ordered) = unsafe {
        object(owner).0.with(|storage| {
            let source = storage.dictionary();
            (source.missing, source.order_version.is_some())
        })
    };
    let missing = match policy {
        Missing::Zero => {
            return Error::new(
                Kind::NotImplementedError,
                "Counter.fromkeys() is undefined.  Use Counter(iterable) instead.",
            )
            .returning(std::ptr::null());
        }
        Missing::List(_) | Missing::NoFactory => Missing::NoFactory,
        Missing::Raise => Missing::Raise,
    };
    // SAFETY: the target is still private; the result needs its own strong reference.
    unsafe {
        object(fresh).0.with_mut(|storage| {
            let target = storage.dictionary_mut();
            target.missing = missing;
            target.order_version = ordered.then_some(0);
        });
        crate::ObjectOwner::increment_strong_count(fresh);
    }
    fresh
}

/// Give the constructor's freshly populated dictionary OrderedDict identity.
///
/// # Safety
/// The argument is a rooted plain dictionary created for this constructor; no
/// user code has observed it. The result has an independent strong reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_ordered_dict(value: *const Object) -> *const Object {
    // SAFETY: the compiler creates this exact dictionary before the call.
    unsafe {
        object(value)
            .0
            .with_mut(|s| s.dictionary_mut().order_version = Some(0));
        crate::ObjectOwner::increment_strong_count(value);
    }
    value
}

/// Copies the mapping in insertion order, preserving its subscription policy.
/// The fresh mapping acquires its keys and values without duplicating them.
///
/// # Safety
/// The input is a rooted dictionary, Counter or closed default-list dictionary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_copy(value: *const Object) -> *const Object {
    // SAFETY: the source owns every copied word; insertion in the fresh table
    // hashes only closed immutable keys and cannot reenter the source.
    let result = unsafe {
        object(value)
            .0
            .with(|storage| storage.dictionary().copy_values(Ok))
    };
    super::exceptions::finish(
        result.map(|copy| {
            crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                Storage::Dictionary(TableOwner::new(copy)),
            ))))
        }),
        std::ptr::null(),
    )
}

/// Constructs an integer-valued Counter with non-inserting zero subscriptions.
#[unsafe(no_mangle)]
pub extern "C" fn imre_counter_new(key_kind: u32) -> *const Object {
    let mut mapping = Mapping::new(key_kind, 2);
    mapping.missing = Missing::Zero;
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Dictionary(TableOwner::new(mapping)),
    ))))
}

/// Add the source counts in insertion order. Updating a counter from itself doubles each count.
///
/// # Safety
/// Both arguments root dictionaries with the same immutable key type and integer values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_counter_update(target: *const Object, source: *const Object) {
    // SAFETY: the iterator acquires the source and each pair before mutating the target.
    // When the handles alias, replacing a count leaves the source's keys intact.
    let updated = (|| unsafe {
        let iter = crate::ObjectOwner::from_raw(imre_dict_iter(source, 1));
        while let Some((key, Some(count))) = iter
            .0
            .with_mut(|s| s.mapping_iterator_mut().next())
            .expect("only counter values change during update")
        {
            let current = Int(imre_dict_get_default(target, key.word, 1));
            let total = Int(crate::imre_int_binary(current.0, count.word, 0));
            dict_set(target, key.word, total.0)?;
        }
        Ok(())
    })();
    crate::finish(updated, ());
}

/// Update a bound mapping without changing the position of existing keys.
///
/// # Safety
/// The caller roots dictionaries with the same closed key and value types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_update(target: *const Object, source: *const Object) {
    // SAFETY: a Counter can arrive through a plain dict annotation. Its bound
    // method adds counts, even when the two arguments are the same object.
    if unsafe {
        object(target)
            .0
            .with(|s| s.dictionary().missing == Missing::Zero)
    } {
        // SAFETY: this policy guarantees integer values, and both rooted
        // dictionaries have the same key and value representations.
        unsafe { imre_counter_update(target, source) };
        return;
    }
    if target == source {
        return;
    }
    // SAFETY: each source pair acquires both owners before we borrow the
    // destination. Displaced owners are released outside that mutable borrow.
    let result = (|| unsafe {
        let mut position = 0;
        while let Some((next, key, value)) = object(source).0.with(|storage| {
            let mapping = storage.dictionary();
            mapping
                .tables
                .next(position)
                .map(|(next, key, word)| (next, key, mapping.value_kind.acquire(word)))
        }) {
            let old = object(target)
                .0
                .with_mut(|s| s.dictionary_mut().insert(key, value))?;
            drop(old);
            position = next;
        }
        Ok(())
    })();
    crate::finish(result, ());
}

/// Sums exact integers after evaluating the dictionary and the start value.
///
/// # Safety
/// The caller roots a dictionary of integers and a tagged integer start.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_sum_values(mapping: *const Object, start: usize) -> usize {
    // SAFETY: no callbacks occur while reading these immutable integer values.
    unsafe {
        crate::imre_int_retain(start);
        let mut total = Int(start);
        object(mapping).0.with(|s| {
            s.dictionary().tables.all(|_, word| {
                total = Int(crate::imre_int_binary(total.0, word, 0));
                true
            })
        });
        total.into_raw()
    }
}

/// # Safety
/// `value` roots an exact Counter; addition only reads its immutable integer words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_counter_total(value: *const Object) -> usize {
    // SAFETY: the caller roots this integer mapping and the zero start is immediate.
    unsafe { imre_dict_sum_values(value, Int::small(0).unwrap().0) }
}

/// Tests multiset inclusion, optionally requiring at least one smaller count.
///
/// # Safety
/// Both arguments root exact integer-valued Counters; they may alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_counter_subset(
    left: *const Object,
    right: *const Object,
    strict: u32,
) -> u32 {
    if left == right {
        return u32::from(strict == 0);
    }
    // SAFETY: immutable visits borrow only rooted keys and integer counts.
    u32::from(unsafe {
        object(left).0.with(|a| {
            object(right).0.with(|b| {
                let mut smaller = false;
                a.dictionary().counter_counts_all(b.dictionary(), |order| {
                    smaller |= order < 0;
                    order <= 0
                }) && (strict == 0 || smaller)
            })
        })
    })
}

/// Attaches the exact element descriptor to a native `defaultdict(list)`.
#[unsafe(no_mangle)]
pub extern "C" fn imre_default_list_dict_new(key_kind: u32, element_kind: u32) -> *const Object {
    let mut mapping = Mapping::new(key_kind, if element_kind == 2 { 4 } else { 5 });
    mapping.missing = Missing::List(WordKind::new(element_kind));
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Dictionary(TableOwner::new(mapping)),
    ))))
}

/// # Safety
/// The caller roots a closed list-valued defaultdict.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_defaultdict_has_factory(value: *const Object) -> u32 {
    // SAFETY: only a flag leaves this immutable storage borrow.
    u32::from(unsafe {
        object(value)
            .0
            .with(|s| matches!(s.dictionary().missing, Missing::List(_)))
    })
}

/// # Safety
/// The caller roots a defaultdict with the given list element kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_defaultdict_set_factory(
    value: *const Object,
    element: u32,
    enabled: u32,
) {
    let policy = if enabled == 0 {
        Missing::NoFactory
    } else {
        Missing::List(WordKind::new(element))
    };
    // SAFETY: a factory change preserves both the table's representations and its entries.
    unsafe {
        object(value)
            .0
            .with_mut(|s| s.dictionary_mut().missing = policy)
    };
}

/// An explicit call runs the factory before checking the table, even for an existing key.
///
/// # Safety
/// The caller roots the mapping and a key of its exact representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_defaultdict_missing(value: *const Object, key: usize) -> usize {
    // SAFETY: only copied factory metadata escapes the borrow.
    let policy = unsafe { object(value).0.with(|s| s.dictionary().missing) };
    match policy {
        Missing::NoFactory => {
            // SAFETY: the key remains rooted until the exception owns it.
            unsafe { missing_key(value, key) }.returning(0)
        }
        Missing::List(element) => {
            let list = default_list(element);
            // SAFETY: the rooted default matches the table's values. On a hit
            // the unused factory result drops after the stored value is retained.
            unsafe { imre_dict_setdefault(value, key, list.word) }
        }
        Missing::Raise | Missing::Zero => {
            let class = if matches!(policy, Missing::Zero) {
                "Counter"
            } else {
                "dict"
            };
            Error::new(Kind::TypeError, format!("descriptor '__missing__' for 'collections.defaultdict' objects doesn't apply to a '{class}' object")).returning(0)
        }
    }
}

/// # Safety
/// `value` is a rooted dictionary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_len(value: *const Object) -> usize {
    // SAFETY: only its copied length escapes.
    Int::from_usize(unsafe { object(value).0.with(|s| s.dictionary().tables.len()) }).into_raw()
}
/// # Safety
/// Both inputs are rooted and `key` has the dictionary's exact key representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_contains(value: *const Object, key: usize) -> u32 {
    // SAFETY: the caller roots the dictionary and its exact-kind key.
    crate::finish(unsafe { contains_key(value, key) }.map(u32::from), 0)
}

/// Test membership without invoking a missing-key policy or suppressing a key error.
pub(super) unsafe fn contains_key(value: *const Object, key: usize) -> Result<bool, Error> {
    // SAFETY: hashing/equality cannot call user code.
    unsafe {
        object(value).0.with(|s| {
            s.dictionary()
                .tables
                .try_lookup(key)
                .map(|found| found.is_some())
        })
    }
}
unsafe fn try_lookup(value: *const Object, key: usize) -> Result<Option<OwnedWord>, Error> {
    // SAFETY: only an independently acquired value leaves storage.
    unsafe {
        object(value).0.with(|s| {
            let mapping = s.dictionary();
            Ok(mapping
                .tables
                .try_lookup(key)?
                .map(|word| mapping.value_kind.acquire(word)))
        })
    }
}
pub(super) unsafe fn lookup(value: *const Object, key: usize) -> Option<OwnedWord> {
    // SAFETY: both inputs stay rooted for this lookup.
    unsafe { try_lookup(value, key) }.unwrap_or_else(|error| error.returning(None))
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
pub unsafe extern "C" fn imre_dict_get(value: *const Object, key: usize) -> usize {
    // SAFETY: a missing key without a factory becomes KeyError at this boundary.
    match unsafe { subscript(value, key) } {
        Ok(Some(owned)) => owned.into_word(),
        Err(error) => error.returning(0),
        // SAFETY: both roots outlive the lookup and the error's acquisition.
        Ok(None) => unsafe { missing_key(value, key) }.returning(0),
    }
}

/// Follow the dictionary's missing-key policy. Only a plain missing entry
/// returns None, allowing ChainMap to continue without constructing a KeyError.
pub(super) unsafe fn subscript(
    value: *const Object,
    key: usize,
) -> Result<Option<OwnedWord>, Error> {
    // SAFETY: this native subscription only hashes immutable keys and constructs
    // an empty builtin list, so no storage borrow crosses a user callback.
    unsafe {
        object(value)
            .0
            .with_mut(|storage| storage.dictionary_mut().subscript(key))
    }
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
pub unsafe extern "C" fn imre_dict_gather_str_int(
    text: *const Str,
    value: *const Object,
) -> *const List {
    // SAFETY: the exact dictionary and immutable string remain rooted.
    unsafe { gather_str_int(&*text, value) }
}

/// Gathers directly from compiler-validated UTF-8 bytes without copying a string.
///
/// # Safety
/// `text` is rooted immutable bytes that already passed `imre_bytes_validate_utf8`
/// at the source decode expression. `value` is a rooted exact `dict[str, int]`,
/// with the same mutation-free comprehension proof as `imre_dict_gather_str_int`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_gather_bytes_int(
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
    let result: Result<Vec<Int>, Error> = unsafe {
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
            let missing = |character| {
                if mapping.missing == Missing::Zero {
                    Ok(Int::small(0).unwrap().0)
                } else {
                    let mut key = Str::new("");
                    key.push(character);
                    Err(Error::string_key(key))
                }
            };
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
                .map_err(|_| Error::allocation())?;
            if ascii_small {
                for (slot, &character) in values.spare_capacity_mut()[..length]
                    .iter_mut()
                    .zip(text.as_bytes())
                {
                    // SAFETY: ascii_small proved this immutable text is ASCII,
                    // so every byte indexes within the 128-entry lookup cache.
                    let word = *ascii.values.get_unchecked(usize::from(character));
                    slot.write(Int(if word == 0 {
                        missing(char::from(character))?
                    } else {
                        word
                    }));
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
                let word = if word == 0 { missing(character)? } else { word };
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
pub unsafe extern "C" fn imre_dict_try_get(
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
pub unsafe extern "C" fn imre_dict_get_default(
    value: *const Object,
    key: usize,
    default: usize,
) -> usize {
    // SAFETY: all declared input representations remain rooted.
    match unsafe { try_lookup(value, key) } {
        Ok(Some(owned)) => return owned.into_word(),
        Ok(None) => {}
        Err(error) => return error.returning(0),
    }
    // SAFETY: only copied representation metadata escapes this rooted read.
    let kind = unsafe { object(value).0.with(|s| s.dictionary().value_kind) };
    // SAFETY: the eager default remains rooted and has this exact value kind.
    unsafe { kind.acquire(default) }.into_word()
}

/// Return the stored value or insert the already evaluated default, bypassing
/// the receiver's missing-key hook. Only a new entry changes iteration order.
///
/// # Safety
/// The mapping and both argument words are rooted with their exact declared kinds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_setdefault(
    value: *const Object,
    key: usize,
    default: usize,
) -> usize {
    // SAFETY: all inputs remain rooted throughout the non-reentrant lookup and
    // insertion. The table retains its own value and the caller receives another owner.
    let result = unsafe {
        object(value).0.with_mut(|s| {
            let mapping = s.dictionary_mut();
            let (word, inserted) =
                table!(&mut mapping.tables, table => table.setdefault(key, default))?;
            if inserted {
                mapping.value_kind.acquire(word).into_word();
                mapping.update_ascii_int(key, word);
                mapping.keys_changed();
            }
            Ok(mapping.value_kind.acquire(word).into_word())
        })
    };
    crate::finish(result, 0)
}

/// # Safety
/// Inputs match this dictionary's concrete types and preserve heap acyclicity.
/// Acquisition precedes mutation, including replacement of the sole stored owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_set(value: *const Object, key: usize, word: usize) {
    // SAFETY: generated arguments obey the native insertion contract.
    crate::finish(unsafe { dict_set(value, key, word) }, ());
}

/// # Safety
/// The rooted container and words must have the exact declared kinds.
pub(super) unsafe fn dict_set(value: *const Object, key: usize, word: usize) -> Result<(), Error> {
    // SAFETY: only copied descriptor metadata leaves storage.
    let (key_kind, value_kind) = unsafe {
        object(value).0.with(|s| {
            let mapping = s.dictionary();
            (WordKind::new(mapping.tables.key_kind()), mapping.value_kind)
        })
    };
    // SAFETY: both arguments are rooted before replacing any old owners.
    let (key, new) = unsafe { (key_kind.acquire(key), value_kind.acquire(word)) };
    // SAFETY: only the displaced owners leave the non-reentrant mutation.
    let old = unsafe {
        object(value)
            .0
            .with_mut(|s| s.dictionary_mut().insert(key, new))
    };
    old.map(drop)
}
/// Removes one entry and keeps its returned owners out of the storage borrow.
///
/// # Safety
/// The exact dictionary and key must remain rooted for the call.
unsafe fn take_entry(
    value: *const Object,
    key: usize,
) -> Result<(Option<(OwnedWord, OwnedWord)>, Missing), Error> {
    // SAFETY: lookup/removal cannot reenter; returned values own their references.
    unsafe {
        object(value).0.with_mut(|s| {
            let mapping = s.dictionary_mut();
            let old = mapping
                .tables
                .remove(key)?
                .map(|(key, word)| (key, OwnedWord::from_owned(word, mapping.value_kind)));
            mapping.update_ascii_int(key, 0);
            if old.is_some() {
                mapping.keys_changed();
            }
            Ok((old, mapping.missing))
        })
    }
}
/// # Safety
/// The exact dictionary and key must be rooted; write an owned word to the
/// independent output slot only when a stored entry was removed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_try_pop(
    value: *const Object,
    key: usize,
    out: *mut usize,
) -> u32 {
    // SAFETY: roots and output satisfy the caller contract.
    let old = match unsafe { take_entry(value, key) } {
        Ok((old, _)) => old,
        Err(error) => return error.returning(0),
    };
    let Some((_key, value)) = old else {
        return 0;
    };
    // SAFETY: only success writes one owned word to the independent output slot.
    unsafe { out.write(value.into_word()) };
    1
}
/// # Safety
/// The exact dictionary, key, and (unless required) value-typed default must
/// remain rooted. An absent required key sets KeyError; otherwise return one owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_pop(
    value: *const Object,
    key: usize,
    default: usize,
    required: u32,
) -> usize {
    // SAFETY: all inputs remain rooted across the removal.
    let old = match unsafe { take_entry(value, key) } {
        Ok((old, _)) => old,
        Err(error) => return error.returning(0),
    };
    if let Some((_key, value)) = old {
        return value.into_word();
    }
    if required != 0 {
        // SAFETY: a failed removal leaves the key and its dictionary rooted.
        return unsafe { missing_key(value, key) }.returning(0);
    }
    // SAFETY: the eager fallback has the declared value kind.
    unsafe {
        object(value)
            .0
            .with(|s| s.dictionary().value_kind.acquire(default))
    }
    .into_word()
}
/// # Safety
/// Root the dictionary and its exact key. Missing raises KeyError in ordinary
/// dictionaries and is a no-op for a Counter. Drop removed owners after mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_delete(value: *const Object, key: usize) {
    // SAFETY: inputs remain rooted until after the operation.
    let (old, missing) = match unsafe { take_entry(value, key) } {
        Ok(removed) => removed,
        Err(error) => return error.raise(),
    };
    if old.is_none() && missing != Missing::Zero {
        // SAFETY: the missing input remains rooted after the failed lookup.
        unsafe { missing_key(value, key) }.raise();
    }
    drop(old);
}

/// # Safety
/// The caller roots an exact dictionary owner throughout the mutation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_clear(value: *const Object) {
    // SAFETY: all stored words transfer to the detached table. Destruction
    // happens after the exclusive storage borrow ends.
    let previous = unsafe {
        object(value)
            .0
            .with_mut(|s| s.dictionary_mut().take_contents())
    };
    drop(previous);
}

/// # Safety
/// Root a dictionary. Mode0 yields keys, mode1 items. Release the internal
/// iterator on every loop exit and never store it in a user container.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_dict_iter(value: *const Object, mode: u32) -> *const Object {
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
    let owner = unsafe { crate::ObjectOwner::acquire_word(value as usize) };
    // SAFETY: the independent owner roots storage; only an owned cursor and size escape.
    let (position, length, expected) = unsafe {
        owner.0.with_mut(|s| {
            let length = s.iteration_len(kind);
            let expected = match kind {
                IteratorKind::Set => length,
                _ => s.dictionary().order_version.unwrap_or(length),
            };
            (s.iteration_position(kind), length, expected)
        })
    };
    let iterator = MappingIterator {
        owner,
        expected,
        position,
        remaining: length,
        kind,
        exhausted: false,
    };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::MappingIterator(iterator),
    ))))
}
#[unsafe(no_mangle)]
pub extern "C" fn imre_set_new(kind: u32) -> *const Object {
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Set(TableOwner::new(Set {
            tables: Tables::new(kind),
            frozen: false,
        })),
    ))))
}

/// Materialize an immutable string set from an exact list of strings.
///
/// # Safety
/// The caller roots the list and checks pending state before using the result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_frozenset_strings(source: *const Object) -> *const Object {
    // SAFETY: the source roots each exact string and insertion cannot reenter Python.
    let result = unsafe {
        object(source).0.with(|s| {
            Set::frozen_strings(
                s.sequence()
                    .strings()
                    .map(|(word, _)| Ok(WordKind::new(3).acquire(word))),
            )
        })
    };
    crate::finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// Return the immutable standard library name set for the Python language target.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_stdlib_module_names() -> *const Object {
    thread_local! {
        static NAMES: std::cell::OnceCell<crate::ObjectOwner> = const { std::cell::OnceCell::new() };
    }
    NAMES.with(|root| {
        if let Some(names) = root.get() {
            return crate::ObjectOwner::into_raw(names.clone());
        }
        let keys = include_str!("../../native/stdlib-module-names.txt")
            .lines()
            .map(|name| {
                let text = Str::try_new(name).map_err(|_| Error::allocation())?;
                let key = StrOwner::try_new(text).map_err(|_| Error::allocation())?;
                // SAFETY: transfer the newly owned native string to the set.
                Ok(unsafe {
                    OwnedWord::from_owned(StrOwner::into_raw(key) as usize, WordKind::new(3))
                })
            });
        match Set::frozen_strings(keys) {
            Ok(names) => crate::ObjectOwner::into_raw(root.get_or_init(|| names).clone()),
            Err(error) => error.returning(std::ptr::null()),
        }
    })
}

/// Reads distinct string keys directly from their source indexes and sorts the
/// resulting list. Each dictionary already supplies unique keys and their hashes.
///
/// # Safety
/// The array contains two to four rooted dictionaries with exact string keys.
/// No callback or mutation occurs until every result owner has been acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sorted_string_keys(
    sources: *const *const Object,
    count: usize,
) -> *const Object {
    fn strings(storage: &Storage) -> &Table<StrOwner, usize> {
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
    crate::ObjectOwner::into_raw(list)
}

/// Creates a fresh set from two evaluated key views or sets. A direct view
/// borrows its dictionary here, so effects in the other operand remain visible.
///
/// # Safety
/// Both inputs are rooted dictionaries or sets with the same immutable key kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_key_union(
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
    for source in std::iter::once(left).chain((left != right).then_some(right)) {
        // SAFETY: the destination is private and distinct from both inputs. All
        // hash operations use sealed immutable keys and cannot call user code.
        let copied = unsafe {
            object(source).0.with(|storage| match storage {
                Storage::Dictionary(dict) => dict.tables.copy_keys(&mut result),
                Storage::Set(set) => set.tables.copy_keys(&mut result),
                _ => unreachable!("the checker proves both key sources"),
            })
        };
        if let Err(error) = copied {
            return error.returning(std::ptr::null());
        }
    }
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::Set(TableOwner::new(Set {
            tables: result,
            frozen: false,
        })),
    ))))
}
/// Builds an intersection directly from two key views or sets. The output owns
/// each selected key, including when the inputs alias each other.
///
/// # Safety
/// Both operands must remain rooted and share the same immutable key type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_key_intersection(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    // SAFETY: both checked inputs remain rooted across the filter.
    unsafe { key_filter(left, right, true) }
}

/// Returns keys in the left set or dictionary view that the right does not contain.
///
/// # Safety
/// Both operands must remain rooted and share the same immutable key type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_key_difference(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    // SAFETY: both checked inputs remain rooted across the filter.
    unsafe { key_filter(left, right, false) }
}

unsafe fn key_filter(
    left: *const Object,
    right: *const Object,
    keep_present: bool,
) -> *const Object {
    // SAFETY: the operands are immutably borrowed. All inserts target the fresh
    // result, and exact scalar key hashing cannot reenter user code.
    let result = unsafe {
        object(left).0.with(|left| {
            let kind = match left {
                Storage::Dictionary(dict) => dict.tables.key_kind(),
                Storage::Set(set) => set.tables.key_kind(),
                _ => unreachable!("expected dictionary keys or set"),
            };
            let result = crate::ObjectOwner::from_raw(imre_set_new(kind));
            object(right).0.with(|right| {
                let select = |key| {
                    let present = match right {
                        Storage::Dictionary(dict) => dict.tables.try_lookup(key)?.is_some(),
                        Storage::Set(set) => set.tables.try_lookup(key)?.is_some(),
                        _ => unreachable!("expected dictionary keys or set"),
                    };
                    if present == keep_present {
                        set_add(crate::ObjectOwner::as_ptr(&result), key)?;
                    }
                    Ok(())
                };
                match left {
                    Storage::Dictionary(dict) => dict.tables.try_for_each(|key, _| select(key)),
                    Storage::Set(set) => set.tables.try_for_each(|key, ()| select(key)),
                    _ => unreachable!(),
                }
            })?;
            Ok(crate::ObjectOwner::into_raw(result))
        })
    };
    crate::finish(result, std::ptr::null())
}

/// # Safety
/// `value` is a rooted set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_set_len(value: *const Object) -> usize {
    // SAFETY: only the copied length escapes storage.
    Int::from_usize(unsafe { object(value).0.with(|s| s.set().tables.len()) }).into_raw()
}
/// # Safety
/// Root both the set and its exact-kind scalar key.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_set_contains(value: *const Object, key: usize) -> u32 {
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
pub unsafe extern "C" fn imre_set_add(value: *const Object, key: usize) {
    // SAFETY: generated arguments obey the native insertion contract.
    crate::finish(unsafe { set_add(value, key) }, ());
}

/// # Safety
/// The rooted container and words must have the exact declared kinds.
pub(super) unsafe fn set_add(value: *const Object, key: usize) -> Result<(), Error> {
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
            .with_mut(|s| s.set_mut().mutable_tables("add")?.insert(key, ()))
    };
    old.map(drop)
}
/// # Safety
/// Root the set and exact key. Missing is a no-op; removed owners drop afterward.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_set_discard(value: *const Object, key: usize) {
    // SAFETY: returned key owners remain independent of their former table slots.
    let removed = unsafe {
        object(value)
            .0
            .with_mut(|s| s.set_mut().mutable_tables("discard")?.remove(key))
    };
    if let Err(error) = removed {
        error.raise();
    }
}
/// # Safety
/// Root the set and release this internal cursor on every loop exit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_set_iter(value: *const Object) -> *const Object {
    // SAFETY: the caller supplies the exact set shape.
    unsafe { new_iterator(value, IteratorKind::Set) }
}
/// Pull one owned item (two owned words in dictionary-items mode). Release
/// the Object storage borrow before a composed iterator can pull another cursor.
pub(super) fn pull(cursor: &Object) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, Error> {
    let mut adapter = None;
    // SAFETY: leaf cursor pulls cannot reenter. Only owners escape the borrow.
    let next = unsafe {
        cursor.0.with_mut(|storage| match storage {
            Storage::IteratorAdapter(state) => {
                adapter = Some(Rc::clone(state));
                Ok(None)
            }
            Storage::Repeat(iterator) => Ok(iterator.next().map(|value| (value, None))),
            Storage::PathIterator(iterator) => iterator.next().map(|path| {
                // A non-null path transfers its owner to the caller of pull().
                (!path.is_null())
                    .then(|| (OwnedWord::from_owned(path as usize, WordKind::new(5)), None))
            }),
            Storage::StringIterator(iterator) => Ok(iterator.next()),
            Storage::RegexIterator(iterator) => Ok(iterator.next()),
            Storage::LineIterator(iterator) => iterator.next(),
            Storage::BytesIterator(iterator) => Ok(iterator.next()),
            Storage::DequeIterator(iterator) => iterator.next(),
            _ => storage
                .mapping_iterator_mut()
                .next()
                .map_err(|error| Error::new(Kind::RuntimeError, error.message())),
        })
    }?;
    match adapter {
        Some(adapter) => adapter.next().map(|item| item.map(|item| (item, None))),
        None => Ok(next),
    }
}

/// # Safety
/// Root the cursor; output words are aligned, independent, outside runtime storage,
/// and contain no unconsumed owner. Only success writes them; keys/set/string mode
/// ignores value_out and permits null. Size errors install RuntimeError and return0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_mapping_iter_next(
    value: *const Object,
    key_out: *mut usize,
    value_out: *mut usize,
) -> u32 {
    // SAFETY: the caller roots an exact cursor.
    let next = match pull(unsafe { object(value) }) {
        Ok(next) => next,
        Err(error) => return error.returning(0),
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
        imre_error_pending, imre_error_take, imre_exception_matches, imre_exception_message,
    };
    use crate::objects::{imre_object_release, imre_seq_append, imre_seq_get_index, imre_seq_new};
    use num_bigint::BigInt;

    fn integer(value: i64) -> usize {
        Int::small(value).unwrap().into_raw()
    }

    #[test]
    fn counter_repr_propagates_writer_failures_and_preserves_ties() {
        struct Budget(usize);
        impl Write for Budget {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                self.0 = self.0.checked_sub(text.len()).ok_or(fmt::Error)?;
                Ok(())
            }
        }

        // SAFETY: the counter owns its table; all keys and counts are immediate integers.
        let counter = unsafe { crate::ObjectOwner::from_raw(imre_counter_new(2)) };
        for (key, count) in [(2, 7), (-8, -4), (1, 7)] {
            // SAFETY: the live counter owns exact immediate integer keys and counts.
            unsafe {
                imre_dict_set(
                    crate::ObjectOwner::as_ptr(&counter),
                    integer(key),
                    integer(count),
                )
            };
        }
        // SAFETY: this shared borrow does not mutate the rooted counter.
        unsafe {
            counter.0.with(|storage| {
                let mapping = storage.dictionary();
                let expected = "Counter({2: 7, 1: 7, -8: -4})";
                for budget in 0..expected.len() {
                    assert!(mapping.write_counter_repr(&mut Budget(budget)).is_err());
                }
                let mut text = String::new();
                mapping.write_counter_repr(&mut text).unwrap();
                assert_eq!(text, expected);
            })
        };
    }

    #[test]
    fn deeply_nested_tuple_keys_hash_every_field_in_order() {
        use crate::objects::records::{
            RecordDescriptor, RecordSemantics, imre_record_new, imre_record_release,
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
            let mut key = imre_record_new(&layout);
            key.add(1).write(integer(7));
            for i in 0..20_000 {
                let parent = imre_record_new(&layout);
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
            assert!(hash_tuple(key, &mut observed).is_ok());
            imre_record_release(key);
        }
        assert_eq!(observed.finish(), expected.finish());
    }

    #[test]
    fn refused_table_reservation_preserves_entries_and_allows_retry() {
        let mut table = Table::<usize, ()>::new();
        assert!(table.insert(7, ()).is_ok());
        assert!(table.reserve(usize::MAX).is_err());
        assert_eq!(table.position(HASHER.hash_one(7_usize), &7), Some(0));
        assert!(table.insert(8, ()).is_ok());
        assert_eq!(table.index.len(), 2);
    }

    #[test]
    fn cache_eviction_keeps_lru_order_across_compaction() {
        let mut table = Table::<usize, ()>::new();
        for key in 0..2 {
            assert!(table.insert_absent(HASHER.hash_one(key), key, ()).is_ok());
        }
        for key in 2..256 {
            // SAFETY: enum tags are immediate scalar keys.
            assert!(matches!(unsafe { table.promote(0) }, Ok(true)));
            assert!(table.insert_absent(HASHER.hash_one(key), key, ()).is_ok());
            let oldest = table.eviction_key();
            assert_eq!(oldest.word, key - 1);
            assert!(matches!(table.remove(&oldest.word), Ok(Some(_))));
        }
    }

    #[test]
    fn cached_hash_collisions_preserve_keys_across_growth_and_compaction() {
        // Force every key into one full-hash collision, including during transfer
        // between independently allocated tables. Keys still compare by value.
        let mut source = Table::<usize, ()>::new();
        let mut result = Table::<usize, ()>::new();
        for key in 1..90 {
            assert!(source.insert_absent(42, key, ()).is_ok());
        }
        assert!(source.copy_keys(&mut result).is_ok());
        assert!(source.copy_keys(&mut result).is_ok());
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
                let text = StrOwner::new(Str::new(spelling));
                crate::ObjectOwner::from_raw(super::super::paths::imre_path_new(StrOwner::as_ptr(
                    &text,
                )))
            });
            let weak = crate::ObjectOwner::downgrade(&original);
            let mapping = crate::ObjectOwner::from_raw(imre_default_list_dict_new(5, 3));
            let dict = crate::ObjectOwner::as_ptr(&mapping);
            let list = crate::ObjectOwner::from_raw(imre_dict_get(
                dict,
                crate::ObjectOwner::as_ptr(&original) as usize,
            ) as *const Object);
            let again = crate::ObjectOwner::from_raw(imre_dict_get(
                dict,
                crate::ObjectOwner::as_ptr(&equal) as usize,
            ) as *const Object);
            assert!(crate::ObjectOwner::ptr_eq(&list, &again));
            assert_eq!(crate::ObjectOwner::strong_count(&equal), 1);
            let keys = crate::ObjectOwner::from_raw(imre_key_union(dict, dict));
            drop(original);
            drop(mapping);
            drop(again);
            assert_eq!(crate::ObjectOwner::strong_count(&list), 1);
            assert!(weak.upgrade().is_some());
            assert_eq!(
                imre_set_contains(
                    crate::ObjectOwner::as_ptr(&keys),
                    crate::ObjectOwner::as_ptr(&equal) as usize
                ),
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
            let dictionary = crate::ObjectOwner::from_raw(imre_dict_new(3, 2));
            let pointer = crate::ObjectOwner::as_ptr(&dictionary);
            for (index, key) in ["a", "é", "😺"].into_iter().enumerate() {
                let key = StrOwner::new(Str::new(key));
                imre_dict_set(
                    pointer,
                    StrOwner::as_ptr(&key) as usize,
                    integer(index as i64),
                );
            }
            for source in ["", "aaa", "aaé😺"] {
                let text = Str::new(source);
                let bytes = crate::BytesOwner::new(source.as_bytes().to_vec());
                let validated =
                    crate::BytesOwner::from_raw(super::super::bytes::imre_bytes_validate_utf8(
                        crate::BytesOwner::as_ptr(&bytes),
                    ));
                let from_text = Rc::from_raw(imre_dict_gather_str_int(&text, pointer));
                let from_bytes = Rc::from_raw(imre_dict_gather_bytes_int(
                    crate::BytesOwner::as_ptr(&validated),
                    pointer,
                ));
                let words = |list: &List| {
                    list.0
                        .with(|values| values.iter().map(|value| value.0).collect::<Vec<_>>())
                };
                assert_eq!(words(&from_bytes), words(&from_text));
                assert_eq!(crate::BytesOwner::strong_count(&bytes), 2);
            }
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn character_gather_observes_mutations_and_exact_unicode_keys() {
        // SAFETY: constructors transfer ownership, and all exact string
        // and dictionary inputs remain rooted through each gather/mutation.
        unsafe {
            let dictionary = crate::ObjectOwner::from_raw(imre_dict_new(3, 2));
            let pointer = crate::ObjectOwner::as_ptr(&dictionary);
            let keys = ["a", "é", "\u{301}", "😺", "\0", "\u{7f}", "\u{80}"]
                .map(|key| StrOwner::new(Str::new(key)));
            for (index, key) in keys.iter().enumerate() {
                imre_dict_set(
                    pointer,
                    StrOwner::as_ptr(key) as usize,
                    integer(index as i64),
                );
            }
            let text = Str::new("aaé\u{301}😺\0a\u{7f}\u{80}");
            let gathered = Rc::from_raw(imre_dict_gather_str_int(&text, pointer));
            let values = gathered.0.with(|values| {
                values
                    .iter()
                    .map(|value| value.as_small().unwrap())
                    .collect::<Vec<_>>()
            });
            assert_eq!(values, [0, 0, 1, 2, 3, 4, 0, 5, 6]);
            let ascii_edges =
                Rc::from_raw(imre_dict_gather_str_int(&Str::new("\0\u{7f}"), pointer));
            ascii_edges.0.with(|values| {
                assert_eq!(values[0].as_small(), Some(4));
                assert_eq!(values[1].as_small(), Some(5));
            });
            let alias = dictionary.clone();
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&alias),
                StrOwner::as_ptr(&keys[0]) as usize,
                integer(-7),
            );
            let changed = Rc::from_raw(imre_dict_gather_str_int(&Str::new("aaa"), pointer));
            changed.0.with(|values| {
                assert!(values.iter().all(|value| value.as_small() == Some(-7)));
            });
            imre_dict_delete(pointer, StrOwner::as_ptr(&keys[0]) as usize);
            assert!(imre_dict_gather_str_int(&Str::new("a"), pointer).is_null());
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            assert_eq!(
                imre_exception_matches(crate::ObjectOwner::as_ptr(&error), Kind::KeyError as u32),
                1
            );
            let message =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(message.as_str(), "'a'");
            imre_dict_set(pointer, StrOwner::as_ptr(&keys[0]) as usize, integer(29));
            let restored = Rc::from_raw(imre_dict_gather_str_int(&Str::new("a"), pointer));
            restored
                .0
                .with(|values| assert_eq!(values[0].as_small(), Some(29)));
            let empty = Rc::from_raw(imre_dict_gather_str_int(&Str::new(""), pointer));
            empty.0.with(|values| assert!(values.is_empty()));
        }
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn character_gather_owns_big_integers_and_releases_partial_results() {
        let integer = Rc::new(BigInt::from(1_u128 << 120));
        let weak = Rc::downgrade(&integer);
        // SAFETY: dictionary/text handles are exact and rooted. A successful
        // gather transfers one list owner; each failed gather transfers none.
        unsafe {
            let dictionary = crate::ObjectOwner::from_raw(imre_dict_new(3, 2));
            let key = StrOwner::new(Str::new("a"));
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&dictionary),
                StrOwner::as_ptr(&key) as usize,
                Rc::as_ptr(&integer) as usize,
            );
            drop(integer);
            assert_eq!(weak.strong_count(), 1);
            for (text, expected) in [
                ("aa😺z", "'😺'"),
                ("aa\0z", "'\\x00'"),
                ("aa\u{301}z", "'́'"),
            ] {
                let result = imre_dict_gather_str_int(
                    &Str::new(text),
                    crate::ObjectOwner::as_ptr(&dictionary),
                );
                assert!(result.is_null());
                assert_eq!(weak.strong_count(), 1);
                assert_eq!(imre_error_pending(), 1);
                let error = crate::ObjectOwner::from_raw(imre_error_take());
                assert_eq!(
                    imre_exception_matches(
                        crate::ObjectOwner::as_ptr(&error),
                        Kind::KeyError as u32
                    ),
                    1
                );
                let message =
                    StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
                assert_eq!(message.as_str(), expected);
            }
            let gathered = Rc::from_raw(imre_dict_gather_str_int(
                &Str::new("aaa"),
                crate::ObjectOwner::as_ptr(&dictionary),
            ));
            assert_eq!(weak.strong_count(), 4);
            imre_dict_set(
                crate::ObjectOwner::as_ptr(&dictionary),
                StrOwner::as_ptr(&key) as usize,
                Int::from_usize(7).into_raw(),
            );
            assert_eq!(weak.strong_count(), 3);
            let replaced = Rc::from_raw(imre_dict_gather_str_int(
                &Str::new("aaa"),
                crate::ObjectOwner::as_ptr(&dictionary),
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
        assert_eq!(imre_error_pending(), 0);
    }

    #[test]
    fn character_gather_tracks_heap_value_transitions_in_its_ascii_cache() {
        let large = Rc::new(BigInt::from(1_u128 << 120));
        let heap_word = Rc::as_ptr(&large) as usize;
        // SAFETY: every key/value remains rooted, and successful results and
        // pending failures each transfer exactly one owner to Rc.
        unsafe {
            let dictionary = crate::ObjectOwner::from_raw(imre_dict_new(3, 2));
            let pointer = crate::ObjectOwner::as_ptr(&dictionary);
            let keys = ["a", "b", "é"].map(|key| StrOwner::new(Str::new(key)));
            let count = || {
                dictionary
                    .0
                    .with(|storage| storage.dictionary().ascii_int.as_ref().unwrap().heap_values)
            };
            assert!(imre_dict_gather_str_int(&Str::new("a"), pointer).is_null());
            drop(crate::ObjectOwner::from_raw(imre_error_take()));
            assert_eq!(count(), 0);
            for (key, word, expected) in [
                (0, heap_word, 1),
                (0, heap_word, 1),
                (1, heap_word, 2),
                (0, integer(0), 1),
                (2, heap_word, 1),
            ] {
                imre_dict_set(pointer, StrOwner::as_ptr(&keys[key]) as usize, word);
                assert_eq!(count(), expected);
                let before = Rc::strong_count(&large);
                let gathered = Rc::from_raw(imre_dict_gather_str_int(
                    StrOwner::as_ptr(&keys[key]),
                    pointer,
                ));
                assert_eq!(
                    Rc::strong_count(&large),
                    before + usize::from(word == heap_word)
                );
                gathered.0.with(|values| assert_eq!(values[0].0, word));
                drop(gathered);
                assert_eq!(Rc::strong_count(&large), before);
            }
            imre_dict_delete(pointer, StrOwner::as_ptr(&keys[1]) as usize);
            assert_eq!(count(), 0);
            imre_dict_set(pointer, StrOwner::as_ptr(&keys[1]) as usize, heap_word);
            assert_eq!(count(), 1);
            imre_dict_delete(pointer, StrOwner::as_ptr(&keys[1]) as usize);
            assert_eq!(count(), 0);
            assert!(imre_dict_gather_str_int(&Str::new("aa\0z"), pointer).is_null());
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            let message =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(message.as_str(), "'\\x00'");
        }
        assert_eq!(Rc::strong_count(&large), 1);
        assert_eq!(imre_error_pending(), 0);
    }

    unsafe fn collect_keys(iterator: *const Object) -> Vec<i64> {
        let mut values = Vec::new();
        let mut word = 0;
        // SAFETY: the caller roots a keys iterator for integer keys; every
        // successful output is consumed before its stack slot is reused.
        unsafe {
            while imre_mapping_iter_next(iterator, &mut word, std::ptr::null_mut()) != 0 {
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
        let dictionary = imre_dict_new(2, 2);
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
                imre_dict_set(dictionary, key.0, integer(index as i64));
            }
            for (index, value) in values.iter().enumerate() {
                let equal_key = Int::new(value.clone());
                assert_eq!(imre_dict_contains(dictionary, equal_key.0), 1);
                assert_eq!(
                    Int(imre_dict_get(dictionary, equal_key.0)).as_small(),
                    Some(index as i64)
                );
            }
            assert_eq!(Int(imre_dict_len(dictionary)).as_small(), Some(6));
            imre_object_release(dictionary);
        }

        let dictionary = imre_dict_new(3, 2);
        let texts = ["é", "e\u{301}", "name\0suffix", "😺"];
        // SAFETY: independent string owners have equal values without sharing an
        // allocation; exact UTF-8 equality does not normalize Unicode forms.
        unsafe {
            for (index, text) in texts.iter().enumerate() {
                let key = StrOwner::new(Str::from(*text));
                imre_dict_set(
                    dictionary,
                    StrOwner::as_ptr(&key) as usize,
                    integer(index as i64),
                );
            }
            for (index, text) in texts.iter().enumerate() {
                let key = StrOwner::new(Str::from(*text));
                assert_eq!(
                    Int(imre_dict_get(dictionary, StrOwner::as_ptr(&key) as usize)).as_small(),
                    Some(index as i64)
                );
            }
            assert_eq!(Int(imre_dict_len(dictionary)).as_small(), Some(4));
            imre_object_release(dictionary);
        }
    }

    #[test]
    fn replacement_acquires_sole_owners_and_reclaims_deleted_keys() {
        let key = Rc::new(BigInt::from(1_u128 << 120));
        let value = Rc::new(BigInt::from(1_u128 << 100));
        let key_weak = Rc::downgrade(&key);
        let value_weak = Rc::downgrade(&value);
        let (key_word, value_word) = (Rc::as_ptr(&key) as usize, Rc::as_ptr(&value) as usize);
        let dictionary = imre_dict_new(2, 2);
        // SAFETY: the dictionary roots its existing key/value during self-assignment;
        // the equal deletion key is independently owned. Snapshots are consumed once.
        unsafe {
            imre_dict_set(dictionary, key_word, value_word);
            drop((key, value));
            imre_dict_set(dictionary, key_word, value_word);
            assert_eq!(value_weak.strong_count(), 1);
            let saved = Int(imre_dict_get(dictionary, key_word));
            imre_dict_set(dictionary, key_word, integer(7));
            let equal_key = Int::new(BigInt::from(1_u128 << 120));
            imre_dict_delete(dictionary, equal_key.0);
            assert!(key_weak.upgrade().is_none());
            imre_object_release(dictionary);
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
                let dictionary = imre_dict_new(2, kind);
                imre_dict_set(dictionary, integer(0), stored);
                let mut out = 73;
                assert_eq!(imre_dict_try_get(dictionary, integer(0), &mut out), 1);
                assert_eq!(out, stored);
                out = 73;
                assert_eq!(imre_dict_try_get(dictionary, integer(1), &mut out), 0);
                assert_eq!(out, 73);
                assert_eq!(
                    imre_dict_get_default(dictionary, integer(1), default),
                    default
                );
                imre_object_release(dictionary);
            }
        }

        let dictionary = imre_dict_new(2, 5);
        // SAFETY: the new field object is owned by this Rc and contains one Int;
        // the default result acquires ownership independently of that Rc.
        unsafe {
            let default = crate::ObjectOwner::from_raw(imre_seq_new(2));
            let weak = crate::ObjectOwner::downgrade(&default);
            imre_seq_append(crate::ObjectOwner::as_ptr(&default), integer(17), 2);
            let result = crate::ObjectOwner::from_raw(imre_dict_get_default(
                dictionary,
                integer(0),
                crate::ObjectOwner::as_ptr(&default) as usize,
            ) as *const Object);
            drop(default);
            imre_object_release(dictionary);
            assert_eq!(
                Int(imre_seq_get_index(crate::ObjectOwner::as_ptr(&result), 0)).as_small(),
                Some(17)
            );
            drop(result);
            assert!(weak.upgrade().is_none());
        }
    }

    #[test]
    fn items_return_owned_snapshots_and_observe_later_value_updates() {
        // SAFETY: each constructor reference is transferred exactly once to Rc.
        let dictionary = unsafe { crate::ObjectOwner::from_raw(imre_dict_new(2, 3)) };
        let dictionary_weak = crate::ObjectOwner::downgrade(&dictionary);
        let original = StrOwner::new(Str::from("original"));
        let replacement = StrOwner::new(Str::from("replacement"));
        let original_weak = StrOwner::downgrade(&original);
        let replacement_weak = StrOwner::downgrade(&replacement);
        let pointer = crate::ObjectOwner::as_ptr(&dictionary);
        // SAFETY: item outputs are independent stack slots. The iterator roots
        // its owner after the external dictionary reference is released.
        unsafe {
            for key in [0, 1] {
                imre_dict_set(pointer, integer(key), StrOwner::as_ptr(&original) as usize);
            }
            let iterator = imre_dict_iter(pointer, 1);
            let (mut key, mut value) = (0, 0);
            assert_eq!(imre_mapping_iter_next(iterator, &mut key, &mut value), 1);
            assert_eq!(Int(key).as_small(), Some(0));
            let first = StrOwner::from_raw(value as *const Str);
            for key in [0, 1] {
                imre_dict_set(
                    pointer,
                    integer(key),
                    StrOwner::as_ptr(&replacement) as usize,
                );
            }
            drop((original, replacement, dictionary));
            assert!(dictionary_weak.upgrade().is_some());
            assert_eq!(imre_mapping_iter_next(iterator, &mut key, &mut value), 1);
            assert_eq!(Int(key).as_small(), Some(1));
            let second = StrOwner::from_raw(value as *const Str);
            imre_object_release(iterator);
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
        let dictionary = imre_dict_new(2, 2);
        // SAFETY: all keys/values are immediate Int words. Iterators and each
        // yielded key are consumed once; storage inspection returns copied lengths.
        unsafe {
            for key in 0..100 {
                imre_dict_set(dictionary, integer(key), integer(key));
            }
            let iterator = imre_dict_iter(dictionary, 0);
            let mut first = 0;
            assert_eq!(
                imre_mapping_iter_next(iterator, &mut first, std::ptr::null_mut()),
                1
            );
            assert_eq!(Int(first).as_small(), Some(0));
            for key in 1..80 {
                imre_dict_delete(dictionary, integer(key));
            }
            for key in 100..179 {
                imre_dict_set(dictionary, integer(key), integer(key));
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
            imre_object_release(iterator);
            imre_dict_delete(dictionary, integer(0));
            for key in 80..100 {
                imre_dict_delete(dictionary, integer(key));
            }
            assert!(object(dictionary).0.with(|s| {
                let table = &s.dictionary().tables;
                table.entry_count() <= 2 * table.len()
            }));
            imre_dict_set(dictionary, integer(120), integer(999));
            let iterator = imre_dict_iter(dictionary, 0);
            assert_eq!(collect_keys(iterator), (100..179).collect::<Vec<_>>());
            imre_object_release(iterator);
            imre_dict_delete(dictionary, integer(120));
            imre_dict_set(dictionary, integer(120), integer(1000));
            let iterator = imre_dict_iter(dictionary, 0);
            assert_eq!(
                collect_keys(iterator),
                (100..179)
                    .filter(|&key| key != 120)
                    .chain([120])
                    .collect::<Vec<_>>()
            );
            imre_object_release(iterator);
            imre_object_release(dictionary);
        }
    }

    #[test]
    fn repeated_reinsertion_compacts_with_independent_live_cursors() {
        let dictionary = imre_dict_new(2, 2);
        // SAFETY: the dictionary and both iterators stay rooted. Keys and values
        // are immediate integers and each output slot is initialized on success.
        unsafe {
            for key in 0..4 {
                imre_dict_set(dictionary, integer(key), integer(key));
            }
            let first = imre_dict_iter(dictionary, 0);
            let second = imre_dict_iter(dictionary, 0);
            let next = |iterator| {
                let mut key = 0;
                assert_eq!(
                    imre_mapping_iter_next(iterator, &mut key, std::ptr::null_mut()),
                    1
                );
                Int(key).as_small().unwrap()
            };
            assert_eq!(next(first), 0);
            assert_eq!(next(first), 1);
            assert_eq!(next(second), 0);
            for _ in 0..512 {
                imre_dict_delete(dictionary, integer(0));
                imre_dict_set(dictionary, integer(0), integer(0));
            }
            assert!(
                object(dictionary)
                    .0
                    .with(|s| s.dictionary().tables.entry_count())
                    <= 32
            );
            assert_eq!(next(first), 2);
            assert_eq!(next(second), 1);
            imre_object_release(first);
            imre_object_release(second);
            imre_object_release(dictionary);
        }
    }

    #[test]
    fn iterator_mutation_errors_and_permanent_exhaustion_are_distinct() {
        let dictionary = imre_dict_new(2, 2);
        // SAFETY: every iterator remains rooted during inspection; step returns
        // owned values and exposes errors without invoking process-level fail.
        unsafe {
            let empty = imre_dict_iter(dictionary, 0);
            assert!(step(empty).unwrap().is_none());
            imre_dict_set(dictionary, integer(0), integer(0));
            assert!(step(empty).unwrap().is_none());
            imre_object_release(empty);

            let resized = imre_dict_iter(dictionary, 0);
            assert!(step(resized).unwrap().is_some());
            imre_dict_set(dictionary, integer(1), integer(1));
            assert!(matches!(step(resized), Err(IterationError::DictionarySize)));
            imre_object_release(resized);

            let changed = imre_dict_iter(dictionary, 0);
            assert_eq!(
                Int(step(changed).unwrap().unwrap().0.into_word()).as_small(),
                Some(0)
            );
            imre_dict_delete(dictionary, integer(0));
            imre_dict_set(dictionary, integer(2), integer(2));
            assert_eq!(
                Int(step(changed).unwrap().unwrap().0.into_word()).as_small(),
                Some(1)
            );
            assert!(matches!(step(changed), Err(IterationError::DictionaryKeys)));
            imre_object_release(changed);
            imre_object_release(dictionary);
        }
    }

    #[test]
    fn sets_deduplicate_integer_values_and_own_yielded_snapshots() {
        let original = Rc::new(BigInt::from(1_u128 << 120));
        let equal = Rc::new(BigInt::from(1_u128 << 120));
        let original_weak = Rc::downgrade(&original);
        let equal_weak = Rc::downgrade(&equal);
        let set = imre_set_new(2);
        // SAFETY: all keys are canonical rooted Int words. Every yielded owner
        // is transferred into Int before its independent output slot is reused.
        unsafe {
            imre_set_add(set, Rc::as_ptr(&original) as usize);
            imre_set_add(set, Rc::as_ptr(&equal) as usize);
            assert_eq!(Int(imre_set_len(set)).as_small(), Some(1));
            drop(equal);
            assert!(equal_weak.upgrade().is_none());
            drop(original);
            for key in [crate::MIN_SMALL, -1, 0, crate::MAX_SMALL] {
                imre_set_add(set, integer(key));
            }
            imre_set_discard(set, integer(7));
            assert_eq!(Int(imre_set_len(set)).as_small(), Some(5));
            let iterator = imre_set_iter(set);
            let mut snapshots = Vec::new();
            let mut word = 0;
            while imre_mapping_iter_next(iterator, &mut word, std::ptr::null_mut()) != 0 {
                snapshots.push(Int(word));
            }
            imre_object_release(iterator);
            let expected = Int::new(BigInt::from(1_u128 << 120));
            assert_eq!(imre_set_contains(set, expected.0), 1);
            imre_set_discard(set, expected.0);
            assert_eq!(imre_set_contains(set, expected.0), 0);
            imre_object_release(set);
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
        let set = unsafe { crate::ObjectOwner::from_raw(imre_set_new(3)) };
        let set_weak = crate::ObjectOwner::downgrade(&set);
        let pointer = crate::ObjectOwner::as_ptr(&set);
        let texts = ["é", "e\u{301}", "name\0suffix", "😺"];
        let mut keys_weak = Vec::new();
        // SAFETY: each string is rooted through insertion. The iterator roots the
        // set after its external owner is dropped, and outputs acquire their own Rc.
        unsafe {
            for text in texts {
                let key = StrOwner::new(Str::from(text));
                keys_weak.push(StrOwner::downgrade(&key));
                imre_set_add(pointer, StrOwner::as_ptr(&key) as usize);
                let duplicate = StrOwner::new(Str::from(text));
                imre_set_add(pointer, StrOwner::as_ptr(&duplicate) as usize);
            }
            assert_eq!(Int(imre_set_len(pointer)).as_small(), Some(4));
            let iterator = imre_set_iter(pointer);
            drop(set);
            assert!(set_weak.upgrade().is_some());
            let mut snapshots = Vec::new();
            let mut word = 0;
            while imre_mapping_iter_next(iterator, &mut word, std::ptr::null_mut()) != 0 {
                snapshots.push(StrOwner::from_raw(word as *const Str));
            }
            imre_object_release(iterator);
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
        let set = imre_set_new(2);
        // SAFETY: the set and iterators remain rooted; all values are immediate
        // integers and step returns only independent owners or an error.
        unsafe {
            let empty = imre_set_iter(set);
            assert!(step(empty).unwrap().is_none());
            imre_set_add(set, integer(0));
            assert!(step(empty).unwrap().is_none());
            imre_object_release(empty);

            let resized = imre_set_iter(set);
            imre_set_add(set, integer(0));
            imre_set_discard(set, integer(1));
            assert!(step(resized).unwrap().is_some());
            imre_set_add(set, integer(1));
            assert!(matches!(step(resized), Err(IterationError::SetSize)));
            imre_object_release(resized);

            let same_size = imre_set_iter(set);
            let yielded = step(same_size).unwrap().unwrap().0;
            imre_set_discard(set, yielded.into_word());
            imre_set_add(set, integer(2));
            // Sets have no dictionary-style remaining-key check or ordering
            // guarantee when deletion/insertion restores the original size.
            while step(same_size).unwrap().is_some() {}
            imre_object_release(same_size);
            imre_object_release(set);
        }
    }
}
