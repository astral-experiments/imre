//! Hash and equality callbacks used by boxed dictionary lookups.

use std::hash::{BuildHasher, RandomState};
use std::sync::LazyLock;

use num_traits::ToPrimitive;

use super::{Context, Entries, Error, Id, Result, Value};
use crate::dynamic::value::{DictViewKind, NONE};

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
    fn stored_entries(&self, dict: Id) -> &Entries {
        match self.heap.get(dict) {
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => entries,
            _ => unreachable!("dictionary storage"),
        }
    }

    fn stored_entries_mut(&mut self, dict: Id) -> &mut Entries {
        match self.heap.get_mut(dict) {
            Value::Dict(entries) | Value::DefaultDict { entries, .. } => entries,
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

    pub(in super::super) fn key_hash_error(&self, key: Id, error: Error, role: &str) -> Error {
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
            if module == "builtins" {
                name.to_owned()
            } else {
                format!("{module}.{name}")
            }
        } else {
            self.key_type_name(key).to_owned()
        };
        Error::new(
            "TypeError",
            format!("cannot use '{name}' as a {role} ({})", error.message),
        )
    }

    fn hash_slot(&self, class: Id) -> Option<Id> {
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return None;
        };
        for &base in std::iter::once(&class).chain(ancestors) {
            if let Some(slot) = self.own_class_attr(base, "__hash__") {
                return Some(slot);
            }
            if self.own_class_attr(base, "__eq__").is_some() {
                return Some(NONE);
            }
        }
        None
    }

    fn tuple_keys(&self, key: Id) -> Option<Vec<Id>> {
        match self.heap.get(key) {
            Value::Tuple(values) | Value::NamedTuple { values, .. } => Some(values.clone()),
            Value::Instance { class, fields, .. } => self
                .namedtuple_fields(*class)
                .map(|names| names.iter().map(|name| fields[name]).collect()),
            _ => None,
        }
    }

    pub(in super::super) fn hash_key(&mut self, key: Id) -> Result<i64> {
        if let Value::Instance { class, .. } = *self.heap.get(key)
            && let Some(slot) = self.hash_slot(class)
        {
            if slot == NONE {
                return Err(self.unhashable(key));
            }
            let method = self.bind_attribute(slot, key, class)?;
            let returned = self.call(method, vec![], vec![])?;
            let hash = match self.heap.get(returned) {
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
        if let Some(&hash) = self.heap.immutable_hashes.get(&key) {
            return Ok(hash);
        }
        let hash = if let Some(values) = self.tuple_keys(key) {
            let mut acc = TUPLE_START;
            for &value in &values {
                acc = tuple_lane(acc, self.hash_key(value)?);
            }
            let hash = finish_tuple(acc, values.len());
            self.heap.immutable_hashes.insert(key, hash);
            hash
        } else if let Value::Set {
            index,
            frozen: true,
            ..
        } = self.heap.get(key)
        {
            let hash = index.frozen_hash();
            self.heap.immutable_hashes.insert(key, hash);
            hash
        } else {
            match self.heap.get(key) {
                Value::Int(n) => integer_hash(n),
                Value::Bool(b) => i64::from(*b),
                Value::Float(f) => float_hash(*f, key),
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
                Value::Range { start, stop, step } => {
                    let n = super::super::builtins::range_len(*start, *stop, *step)?;
                    let a = if n == 0 {
                        0xfca8_6420
                    } else {
                        *start % MODULUS
                    };
                    let b = if n <= 1 { 0xfca8_6420 } else { *step % MODULUS };
                    finish_tuple(
                        [
                            (n as u64 % MODULUS as u64) as i64,
                            normalized(a),
                            normalized(b),
                        ]
                        .into_iter()
                        .fold(TUPLE_START, tuple_lane),
                        3,
                    )
                }
                _ if !super::super::builtins::hashable(self, key) => {
                    return Err(self.unhashable(key));
                }
                _ => data_hash(&key.to_ne_bytes()),
            }
        };
        Ok(normalized(hash))
    }

    pub(in super::super) fn keys_equal(&mut self, left: Id, right: Id) -> Result<bool> {
        if left == right {
            return Ok(true);
        }
        if let Some(answer) = self.binary_special("==", left, right)? {
            return self.truth(answer);
        }
        if let Some(equal) = self.set_is_subset(left, right, true)? {
            return Ok(equal);
        }
        if let (Some(left), Some(right)) = (self.tuple_keys(left), self.tuple_keys(right)) {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (a, b) in left.into_iter().zip(right) {
                if !self.keys_equal(a, b)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        self.checked_equal(left, right)
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
            .map(|key| self.hash_key(key))
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
        let hash = match known_hash {
            Some(hash) => hash,
            None => self
                .hash_key(key)
                .map_err(|error| self.key_hash_error(key, error, "dict key"))?,
        };
        self.index_dictionary(dict)?;
        'restart: loop {
            let index = self
                .stored_entries(dict)
                .index
                .as_ref()
                .expect("indexed dictionary");
            let generation = index.generation;
            for bucket in Probe::new(hash, index.slots.len()) {
                let entries = self.stored_entries(dict);
                let index = entries.index.as_ref().expect("indexed dictionary");
                let position = index.slots[bucket];
                if position == EMPTY {
                    return Ok((hash, None));
                }
                if position == DUMMY {
                    continue;
                }
                let dense = entries.dense(position).expect("live dictionary slot");
                let candidate = entries[dense].0;
                if candidate == key {
                    return Ok((hash, Some(dense)));
                }
                if index.hashes[dense] != hash {
                    continue;
                }
                self.heap.keep(candidate);
                let equal = self.keys_equal(candidate, key)?;
                let entries = self.stored_entries(dict);
                let index = entries.index.as_ref().expect("indexed dictionary");
                let dense = entries.dense(position);
                if index.generation != generation || dense.is_none_or(|n| entries[n].0 != candidate)
                {
                    continue 'restart;
                }
                if equal {
                    return Ok((hash, dense));
                }
            }
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
            if overwrite {
                entries[i].1 = value;
            }
            return Ok(entries[i].1);
        }
        entries.push((key, value), hash);
        Ok(value)
    }
}
