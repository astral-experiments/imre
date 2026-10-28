//! Hash and equality callbacks used by boxed dictionary lookups.

use std::hash::{BuildHasher, RandomState};
use std::ops::ControlFlow;
use std::sync::LazyLock;

use num_traits::ToPrimitive;

use super::Entries;
use crate::dynamic::value::{DictViewKind, NONE};
use crate::dynamic::{Context, Error, Id, Result, Value, weakref};

const EMPTY: usize = usize::MAX;
const DUMMY: usize = usize::MAX - 1;
const MODULUS: i64 = (1 << 61) - 1;

#[derive(Clone)]
pub(super) struct Index {
    slots: Vec<usize>,
    hashes: Vec<i64>,
    generation: usize,
}

struct Probe {
    index: usize,
    perturb: usize,
    mask: usize,
}

impl Probe {
    fn new(hash: i64, size: usize) -> Self {
        let perturb = hash as usize;
        Self {
            index: perturb & (size - 1),
            perturb,
            mask: size - 1,
        }
    }
}

impl Iterator for Probe {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        let index = self.index;
        self.perturb >>= 5;
        self.index = index
            .wrapping_mul(5)
            .wrapping_add(self.perturb)
            .wrapping_add(1)
            & self.mask;
        Some(index)
    }
}

/// A suspended dictionary probe, resumed after comparing a resident key in Python.
struct KeyLookup {
    dict: Id,
    key: Id,
    hash: i64,
    generation: usize,
    slots: Probe,
}

impl KeyLookup {
    fn new(ctx: &mut Context, dict: Id, key: Id, known_hash: Option<i64>) -> Result<Self> {
        let hash = match known_hash {
            Some(hash) => hash,
            None => ctx
                .dictionary_hash(key)
                .map_err(|error| ctx.dictionary_hash_error(dict, key, error))?,
        };
        ctx.index_dictionary(dict)?;
        let index = ctx
            .stored_entries(dict)
            .index
            .as_ref()
            .expect("indexed dictionary");
        Ok(Self {
            dict,
            key,
            hash,
            generation: index.generation,
            slots: Probe::new(hash, index.slots.len()),
        })
    }

    /// Revalidate a resident after its equality callback, then find the next matching hash.
    fn next(
        &mut self,
        ctx: &Context,
        previous: Option<(usize, Id, bool)>,
    ) -> ControlFlow<Option<usize>, (usize, Id)> {
        let entries = ctx.stored_entries(self.dict);
        let index = entries.index.as_ref().expect("indexed dictionary");
        if let Some((position, resident, equal)) = previous {
            let dense = entries.dense(position);
            if self.generation != index.generation || dense.is_none_or(|n| entries[n].0 != resident)
            {
                self.generation = index.generation;
                self.slots = Probe::new(self.hash, index.slots.len());
            } else if equal {
                return ControlFlow::Break(dense);
            }
        }
        for bucket in &mut self.slots {
            let position = index.slots[bucket];
            if position == EMPTY {
                return ControlFlow::Break(None);
            }
            if position == DUMMY {
                continue;
            }
            let dense = entries.dense(position).expect("live dictionary slot");
            let resident = entries[dense].0;
            if resident == self.key {
                return ControlFlow::Break(Some(dense));
            }
            if index.hashes[dense] == self.hash {
                return ControlFlow::Continue((position, resident));
            }
        }
        unreachable!("the dictionary probe always yields a bucket")
    }
}

impl Index {
    fn new(hashes: Vec<i64>, positions: Option<&[usize]>, size: usize, generation: usize) -> Self {
        let mut index = Self {
            slots: vec![EMPTY; size],
            hashes,
            generation,
        };
        for dense in 0..index.hashes.len() {
            let position = positions.map_or(dense, |p| p[dense]);
            index.place(index.hashes[dense], position);
        }
        index
    }

    fn place(&mut self, hash: i64, position: usize) {
        let slot = Probe::new(hash, self.slots.len())
            .find(|&i| matches!(self.slots[i], EMPTY | DUMMY))
            .expect("dictionary has available slots");
        self.slots[slot] = position;
    }

    pub(super) fn compact(&mut self, size: usize) {
        *self = Self::new(
            std::mem::take(&mut self.hashes),
            None,
            size,
            self.generation + 1,
        );
    }

    pub(super) fn push(&mut self, hash: i64, position: usize) {
        self.place(hash, position);
        self.hashes.push(hash);
    }

    pub(super) fn remove(&mut self, dense: usize, position: usize) {
        let hash = self.hashes.remove(dense);
        let bucket = Probe::new(hash, self.slots.len())
            .find(|&i| self.slots[i] == position)
            .expect("stored dictionary key");
        self.slots[bucket] = DUMMY;
    }
}

impl Entries {
    /// Identify changes to insertion positions even when the live size is restored.
    pub(in super::super) fn order_state(&self) -> (usize, usize, usize) {
        (
            self.next,
            self.index.as_ref().map_or(0, |index| index.generation),
            self.order.as_ref().map_or(0, |order| order.revision),
        )
    }

    /// Remove all pairs and invalidate lookups suspended in a key comparison.
    pub(in super::super) fn clear(&mut self) -> Vec<(Id, Id)> {
        let old = std::mem::take(self);
        if let Some(index) = old.index {
            let size = (self.usable + 1).next_power_of_two();
            self.index = Some(Index::new(Vec::new(), None, size, index.generation + 1));
        }
        old.values
    }

    fn dense(&self, position: usize) -> Option<usize> {
        if let Some(positions) = &self.positions {
            positions.binary_search(&position).ok()
        } else {
            (position < self.values.len()).then_some(position)
        }
    }

    pub(in super::super) fn stored_hash(&self, dense: usize) -> Option<i64> {
        self.index.as_ref().map(|i| i.hashes[dense])
    }
}

// Python reserves -1 as the error return from a hash slot.
fn normalized(hash: i64) -> i64 {
    if hash == -1 { -2 } else { hash }
}

fn integer_hash(value: &crate::Int) -> i64 {
    value.to_i64().map_or_else(
        || {
            (value.big().as_ref() % MODULUS)
                .to_i64()
                .expect("bounded remainder")
        },
        |n| n % MODULUS,
    )
}

fn float_hash(value: f64, identity: Id) -> i64 {
    if !value.is_finite() {
        return if value.is_nan() {
            identity as i64
        } else if value.is_sign_negative() {
            -314159
        } else {
            314159
        };
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let significand = (bits & ((1 << 52) - 1)) | (u64::from(exponent != 0) << 52);
    let shift = (exponent.max(1) - 1023 - 52).rem_euclid(61);
    let magnitude = ((significand << shift) & MODULUS as u64) | (significand >> (61 - shift));
    if value.is_sign_negative() {
        -(magnitude as i64)
    } else {
        magnitude as i64
    }
}

const TUPLE_START: u64 = 2_870_177_450_012_600_261;

fn tuple_lane(hash: u64, lane: i64) -> u64 {
    hash.wrapping_add((lane as u64).wrapping_mul(14_029_467_366_897_019_727))
        .rotate_left(31)
        .wrapping_mul(11_400_714_785_074_694_791)
}

fn finish_tuple(hash: u64, length: usize) -> i64 {
    let hash = hash.wrapping_add(length as u64 ^ (TUPLE_START ^ 3_527_539));
    if hash == u64::MAX {
        1_546_275_796
    } else {
        hash as i64
    }
}

fn data_hash(bytes: &[u8]) -> i64 {
    static SEED: LazyLock<RandomState> = LazyLock::new(RandomState::new);
    if bytes.is_empty() {
        0
    } else {
        SEED.hash_one(bytes) as i64
    }
}

impl Context {
    pub(in super::super) fn stored_entries(&self, dict: Id) -> &Entries {
        match self.heap.get(dict) {
            Value::Set { entries, .. }
            | Value::Dict(entries)
            | Value::DefaultDict { entries, .. } => entries,
            _ => unreachable!("dictionary storage"),
        }
    }

    /// Resolve a move after evaluating its direction, preserving the resident key.
    pub(in super::super) fn move_dictionary_key(
        &mut self,
        dict: Id,
        key: Id,
        last: bool,
    ) -> Result<()> {
        let entries = self.stored_entries(dict);
        let Some(end) = entries.len().checked_sub(1) else {
            return Err(self.lookup_error("KeyError", key));
        };
        if entries[entries.ordered_dense(if last { end } else { 0 })].0 == key {
            return Ok(());
        }
        let (_, dense) = self.find_dictionary_key(dict, key, None)?;
        let dense = dense.ok_or_else(|| self.lookup_error("KeyError", key))?;
        self.stored_entries_mut(dict).move_to_end(dense, last)
    }

    fn stored_entries_mut(&mut self, dict: Id) -> &mut Entries {
        match self.heap.get_mut(dict) {
            Value::Set { entries, .. }
            | Value::Dict(entries)
            | Value::DefaultDict { entries, .. } => entries,
            _ => unreachable!("dictionary storage"),
        }
    }

    fn key_type_name(&self, key: Id) -> &str {
        match self.heap.get(key) {
            Value::DictView {
                kind: DictViewKind::Keys,
                ..
            } => "dict_keys",
            Value::DictView {
                kind: DictViewKind::Items,
                ..
            } => "dict_items",
            Value::DictView {
                kind: DictViewKind::Values,
                ..
            } => "dict_values",
            _ => self.type_name(key),
        }
    }

    fn unhashable(&self, key: Id) -> Error {
        Error::new(
            "TypeError",
            format!("unhashable type: '{}'", self.key_type_name(key)),
        )
    }

    fn dictionary_hash_error(&self, container: Id, key: Id, error: Error) -> Error {
        self.key_hash_error(
            key,
            if matches!(self.heap.get(container), Value::Set { .. }) {
                "set element"
            } else {
                "dict key"
            },
            error,
        )
    }

    pub(in super::super) fn key_hash_error(&self, key: Id, role: &str, error: Error) -> Error {
        if error.kind != "TypeError" {
            return error;
        }
        let name = if let Value::Instance { class, .. } = *self.heap.get(key)
            && let Value::Class { scope, .. } = *self.heap.get(class)
        {
            let module = &self.scope_data(scope).module;
            let name = self
                .own_class_attr(class, "__qualname__")
                .and_then(|id| self.str_value(id).ok())
                .unwrap_or_else(|| self.key_type_name(key));
            if matches!(module.as_str(), "builtins" | "__main__") {
                name.to_owned()
            } else {
                format!("{module}.{name}")
            }
        } else {
            self.key_type_name(key).to_owned()
        };
        Error::new(
            "TypeError",
            format!("cannot use '{name}' as a {} ({})", role, error.message),
        )
    }

    fn tuple_keys(&self, key: Id) -> Option<Vec<Id>> {
        match self.tuple_backing(key) {
            Value::Tuple(values) | Value::NamedTuple { values, .. } => Some(values.clone()),
            Value::Instance { class, fields, .. } => self
                .namedtuple_fields(*class)
                .map(|names| names.iter().map(|name| fields[name]).collect()),
            _ => None,
        }
    }

    /// Hash the stable arena handle without calling any Python special methods.
    pub(in super::super) fn identity_hash(key: Id) -> i64 {
        normalized(data_hash(&key.to_ne_bytes()))
    }

    pub(in super::super) fn dictionary_hash(&mut self, key: Id) -> Result<i64> {
        if let Value::Union(union) = *self.heap.get(key) {
            return union.hash(self);
        }
        if let Value::Weak(wr) = *self.heap.get(key) {
            if wr.kind != super::super::weakref::Kind::Reference {
                return Err(self.unhashable(key));
            }
            return self.weak_reference_hash(key, wr);
        }
        if let Value::Instance { class, .. } = *self.heap.get(key)
            && let Some(slot) = self.class_attr(class, "__hash__")
        {
            if slot == NONE {
                return Err(self.unhashable(key));
            }
            if matches!(self.heap.get(slot), Value::Native(name) if name == "object.__hash__") {
                return Ok(Self::identity_hash(key));
            }
            let method = self.bind_attribute(slot, key, class)?;
            let returned = self.call(method, vec![], vec![])?;
            let hash = match self.heap.get(self.scalar_value(returned)) {
                Value::Int(n) => n.to_i64().unwrap_or_else(|| integer_hash(n)),
                Value::Bool(b) => i64::from(*b),
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        "__hash__ method should return an integer",
                    ));
                }
            };
            return Ok(normalized(hash));
        }
        if matches!(self.heap.get(key), Value::Method { .. }) {
            let mut receivers = smallvec::SmallVec::<[Id; 4]>::new();
            let mut callable = key;
            while let Value::Method {
                callable: inner,
                receiver,
                ..
            } = *self.heap.get(callable)
            {
                receivers
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                receivers.push(receiver);
                callable = inner;
            }
            let mut hash = self.dictionary_hash(callable)?;
            for receiver in receivers.into_iter().rev() {
                hash = normalized(hash ^ data_hash(&receiver.to_ne_bytes()));
            }
            return Ok(hash);
        }
        // Immutable scalar subclasses inherit the hash of their backing value.
        let key = self.scalar_value(key);
        if let Value::GenericAlias(alias) = self.heap.get(key) {
            if matches!(alias.kind, super::super::typing::AliasKind::Literal(_)) {
                let members = super::super::typing::literal_members(self, alias.args)?;
                return self.dictionary_hash(members);
            }
            let (origin, args) = (alias.origin, alias.args);
            return Ok(normalized(
                data_hash(&origin.to_ne_bytes()) ^ self.dictionary_hash(args)?,
            ));
        }
        if let Some(&hash) = self.heap.immutable_hashes.get(&key) {
            return Ok(hash);
        }
        let hash = if let Some(values) = self.tuple_keys(key) {
            let mut acc = TUPLE_START;
            for &value in &values {
                acc = tuple_lane(acc, self.dictionary_hash(value)?);
            }
            let hash = finish_tuple(acc, values.len());
            self.heap.immutable_hashes.insert(key, hash);
            hash
        } else if let Value::Slice(parts) = *self.heap.get(key) {
            // Slice components are hashed again on each lookup, in start/stop/step order.
            let mut hash = TUPLE_START;
            for part in parts {
                hash = tuple_lane(hash, self.dictionary_hash(part)?);
            }
            if hash == u64::MAX {
                1_546_275_796
            } else {
                hash as i64
            }
        } else if let Value::Set {
            entries,
            frozen: true,
        } = self.heap.get(key)
        {
            // Frozenset hashes are independent of item order. Cache the successful
            // result so repeated dictionary probes leave the members untouched.
            let size = entries.len();
            let mut hash = 0_u64;
            for i in 0..size {
                let lane = entries.stored_hash(i).expect("stored set hash") as u64;
                hash ^= ((lane ^ 89_869_747) ^ (lane << 16)).wrapping_mul(3_644_798_167);
            }
            hash ^= (size as u64 + 1).wrapping_mul(1_927_868_237);
            hash ^= (hash >> 11) ^ (hash >> 25);
            hash = hash.wrapping_mul(69_069).wrapping_add(907_133_923);
            let hash = if hash == u64::MAX {
                590_923_713
            } else {
                hash as i64
            };
            self.heap.immutable_hashes.insert(key, hash);
            hash
        } else {
            match self.heap.get(key) {
                Value::Int(n) => integer_hash(n),
                Value::Bool(b) => i64::from(*b),
                Value::Float(f) => float_hash(*f, key),
                Value::Complex(z) => normalized(float_hash(z.real, key))
                    .wrapping_add(1_000_003_i64.wrapping_mul(normalized(float_hash(z.imag, key)))),
                Value::Native(_)
                    if matches!(
                        self.builtin_type_name(key),
                        Some("type" | "wrapper_descriptor" | "method_descriptor")
                    ) =>
                {
                    Self::identity_hash(key)
                }
                Value::Str(text) | Value::Native(text) => data_hash(text.as_bytes()),
                Value::Bytes(bytes) => data_hash(bytes),
                Value::None => 0xfca8_6420,
                Value::Calendar { value, .. } => {
                    if let Some((d, s, u)) = value.hash_delta_parts() {
                        finish_tuple(
                            [d, s, u]
                                .into_iter()
                                .fold(TUPLE_START, |acc, n| tuple_lane(acc, normalized(n as i64))),
                            3,
                        )
                    } else {
                        let [year, month, day, hour, minute, second, microsecond, _] =
                            value.fields();
                        let bytes = [
                            (year >> 8) as u8,
                            year as u8,
                            month as u8,
                            day as u8,
                            hour as u8,
                            minute as u8,
                            second as u8,
                            (microsecond >> 16) as u8,
                            (microsecond >> 8) as u8,
                            microsecond as u8,
                        ];
                        data_hash(if value.class() == "date" {
                            &bytes[..4]
                        } else {
                            &bytes
                        })
                    }
                }
                Value::Range(range) => {
                    let len = range.len.as_small();
                    let a = if len == Some(0) {
                        0xfca8_6420
                    } else {
                        integer_hash(&range.start)
                    };
                    let b = if matches!(len, Some(0 | 1)) {
                        0xfca8_6420
                    } else {
                        integer_hash(&range.step)
                    };
                    finish_tuple(
                        [integer_hash(&range.len), normalized(a), normalized(b)]
                            .into_iter()
                            .fold(TUPLE_START, tuple_lane),
                        3,
                    )
                }
                _ if !super::super::builtins::hashable(self, key) => {
                    return Err(self.unhashable(key));
                }
                _ => Self::identity_hash(key),
            }
        };
        Ok(normalized(hash))
    }

    /// Look up a Python name in immutable builtin type dictionaries. Type lookup clears errors.
    pub(in super::super) fn lookup_builtin_type_name(
        &mut self,
        key: Id,
        names: &[&'static str],
    ) -> Option<&'static str> {
        if let Value::Str(text) = self.heap.get(key) {
            return names.iter().copied().find(|&name| name == text);
        }
        let hash = self.dictionary_hash(key).ok()?;
        for &name in names {
            if normalized(data_hash(name.as_bytes())) == hash {
                let stored = self.text(name);
                if self.dictionary_equal(stored, key).ok()? {
                    return Some(name);
                }
            }
        }
        None
    }

    pub(in super::super) fn dictionary_equal(&mut self, left: Id, right: Id) -> Result<bool> {
        if left == right {
            return Ok(true);
        }
        let roots = self.heap.temporary_mark();
        self.heap.keep(left);
        self.heap.keep(right);
        let result = self.compare_key_pairs(left, right);
        self.heap.release_temporaries_since(roots);
        result
    }

    /// Suspend the enclosing tuple or live set probe while comparing a resident and its key.
    fn compare_key_pairs(&mut self, left: Id, right: Id) -> Result<bool> {
        enum Work {
            Pair(Id, Id),
            Tuple(std::iter::Zip<std::vec::IntoIter<Id>, std::vec::IntoIter<Id>>),
            Set(Id, Id, usize),
            Probe(KeyLookup, Option<(usize, Id)>),
            Done(bool),
        }

        let mut frames = Vec::new();
        let mut work = Work::Pair(left, right);
        let mut equal = true;
        loop {
            let (resume, child) = match work {
                Work::Pair(a, b)
                    if a != b
                        && matches!(
                            (self.heap.get(a), self.heap.get(b)),
                            (Value::Method { .. }, Value::Method { .. })
                        ) =>
                {
                    match self.method_equality_operands(a, b) {
                        Some((left, right, true)) => {
                            work = Work::Pair(left, right);
                            continue;
                        }
                        // Python compares the callables before testing receiver identity.
                        Some((left, right, false)) => (Work::Done(false), Work::Pair(left, right)),
                        None => {
                            work = Work::Done(false);
                            continue;
                        }
                    }
                }
                Work::Pair(left, right) => {
                    equal = true;
                    work = if left == right {
                        Work::Done(true)
                    } else if let (Value::Weak(a), Value::Weak(b)) =
                        (self.heap.get(left), self.heap.get(right))
                        && a.kind == weakref::Kind::Reference
                        && b.kind == weakref::Kind::Reference
                        && let (Some(a), Some(b)) = (a.target, b.target)
                        && a != b
                        && matches!(
                            (self.heap.get(a), self.heap.get(b)),
                            (Value::Set { .. }, Value::Set { .. })
                                | (Value::GenericAlias(_), Value::GenericAlias(_))
                                | (Value::Method { .. }, Value::Method { .. })
                        )
                    {
                        // The references do not keep these targets alive across callbacks.
                        self.heap.keep(a);
                        self.heap.keep(b);
                        Work::Pair(a, b)
                    } else if let Some(answer) = self.binary_special("==", left, right)? {
                        Work::Done(self.truth(answer)?)
                    } else if matches!(
                        (self.heap.get(left), self.heap.get(right)),
                        (Value::GenericAlias(_), Value::GenericAlias(_))
                    ) {
                        if let Some((a, b)) =
                            super::super::typing::checked_alias_parts(self, left, right)?
                        {
                            Work::Pair(a, b)
                        } else {
                            Work::Done(false)
                        }
                    } else if let (Value::Union(a), Value::Union(b)) =
                        (self.heap.get(left), self.heap.get(right))
                        && a.unhashable.is_none()
                        && b.unhashable.is_none()
                    {
                        Work::Pair(a.members, b.members)
                    } else if let (Value::Slice(a), Value::Slice(b)) =
                        (self.heap.get(left), self.heap.get(right))
                    {
                        let answer = super::super::slice::compare(self, "==", *a, *b)?;
                        Work::Done(self.truth(answer)?)
                    } else if let (Value::Set { entries: a, .. }, Value::Set { entries: b, .. }) =
                        (self.heap.get(left), self.heap.get(right))
                    {
                        if a.len() == b.len() {
                            Work::Set(left, right, 0)
                        } else {
                            Work::Done(false)
                        }
                    } else if let (Some(a), Some(b)) =
                        (self.tuple_keys(left), self.tuple_keys(right))
                    {
                        if a.len() == b.len() {
                            Work::Tuple(a.into_iter().zip(b))
                        } else {
                            Work::Done(false)
                        }
                    } else {
                        Work::Done(self.checked_equal_impl(left, right, false)?)
                    };
                    continue;
                }
                Work::Tuple(mut pairs) => {
                    if equal && let Some((a, b)) = pairs.next() {
                        (Work::Tuple(pairs), Work::Pair(a, b))
                    } else {
                        work = Work::Done(equal);
                        continue;
                    }
                }
                Work::Set(source, target, position) => {
                    if equal
                        && let Some((at, key, _, hash)) = self.dictionary_pair(source, position)
                    {
                        (
                            Work::Set(source, target, at + 1),
                            Work::Probe(KeyLookup::new(self, target, key, hash)?, None),
                        )
                    } else {
                        work = Work::Done(equal);
                        continue;
                    }
                }
                Work::Probe(mut lookup, candidate) => {
                    match lookup.next(self, candidate.map(|(position, id)| (position, id, equal))) {
                        ControlFlow::Break(found) => {
                            work = Work::Done(found.is_some());
                            continue;
                        }
                        ControlFlow::Continue((position, resident)) => {
                            let pair = Work::Pair(resident, lookup.key);
                            (Work::Probe(lookup, Some((position, resident))), pair)
                        }
                    }
                }
                Work::Done(result) => {
                    let Some((parent, roots)) = frames.pop() else {
                        return Ok(result);
                    };
                    self.heap.release_temporaries_since(roots);
                    equal = result;
                    work = parent;
                    continue;
                }
            };
            frames
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            let roots = self.heap.temporary_mark();
            match child {
                Work::Pair(a, b) => {
                    self.heap.keep(a);
                    self.heap.keep(b);
                }
                Work::Probe(ref lookup, _) => self.heap.keep(lookup.key),
                _ => unreachable!(),
            }
            frames.push((resume, roots));
            work = child;
        }
    }

    pub(in super::super) fn dictionary_pair(
        &self,
        dict: Id,
        position: usize,
    ) -> Option<(usize, Id, Id, Option<i64>)> {
        let entries = self.stored_entries(dict);
        let (position, key, value) = entries.at(position)?;
        let dense = entries.dense(position).expect("live dictionary entry");
        Some((position, key, value, entries.stored_hash(dense)))
    }

    fn index_dictionary(&mut self, dict: Id) -> Result<()> {
        if self.stored_entries(dict).index.is_some() {
            return Ok(());
        }
        // Internal constructors can assemble a dictionary directly from builtin names.
        let keys: Vec<_> = self
            .stored_entries(dict)
            .iter()
            .map(|&(key, _)| key)
            .collect();
        let hashes = keys
            .into_iter()
            .map(|key| self.dictionary_hash(key))
            .collect::<Result<Vec<_>>>()?;
        let entries = self.stored_entries_mut(dict);
        let size = (entries.usable + 1).next_power_of_two();
        entries.index = Some(Index::new(hashes, entries.positions.as_deref(), size, 0));
        Ok(())
    }

    pub(in super::super) fn find_dictionary_key(
        &mut self,
        dict: Id,
        key: Id,
        known_hash: Option<i64>,
    ) -> Result<(i64, Option<usize>)> {
        self.heap.keep(dict);
        self.heap.keep(key);
        let mut lookup = KeyLookup::new(self, dict, key, known_hash)?;
        let mut previous = None;
        loop {
            match lookup.next(self, previous) {
                ControlFlow::Break(found) => return Ok((lookup.hash, found)),
                ControlFlow::Continue((position, resident)) => {
                    self.heap.keep(resident);
                    previous = Some((position, resident, self.dictionary_equal(resident, key)?));
                }
            }
        }
    }

    /// Insert a cache entry and identify the actual resident after reentrant comparison.
    pub(in super::super) fn set_cache_key(
        &mut self,
        dict: Id,
        key: Id,
        value: Id,
        hash: i64,
    ) -> Result<Id> {
        self.heap.keep(value);
        let (_, index) = self.find_dictionary_key(dict, key, Some(hash))?;
        let entries = self.stored_entries_mut(dict);
        if let Some(index) = index {
            let resident = entries[index].0;
            let previous = std::mem::replace(&mut entries[index].1, value);
            if previous != value {
                self.heap.weak_edge_removed(previous);
            }
            Ok(resident)
        } else {
            entries.push((key, value), hash)?;
            Ok(key)
        }
    }

    pub(in super::super) fn set_dictionary_key(
        &mut self,
        dict: Id,
        key: Id,
        value: Id,
        known_hash: Option<i64>,
        overwrite: bool,
    ) -> Result<Id> {
        self.heap.keep(value);
        let (hash, index) = self.find_dictionary_key(dict, key, known_hash)?;
        let entries = self.stored_entries_mut(dict);
        if let Some(i) = index {
            if !overwrite {
                return Ok(entries[i].1);
            }
            let previous = std::mem::replace(&mut entries[i].1, value);
            if previous != value {
                self.heap.weak_edge_removed(previous);
            }
            return Ok(value);
        }
        entries.push((key, value), hash)?;
        Ok(value)
    }
}
