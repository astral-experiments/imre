//! Hash and equality callbacks used by boxed dictionary lookups.

use std::ops::ControlFlow;

use num_traits::ToPrimitive;
use smallvec::SmallVec;

use super::Entries;
use crate::dynamic::value::NONE;
use crate::dynamic::{Context, Error, Id, Result, Value, weakref};

const EMPTY: usize = usize::MAX;
const DUMMY: usize = usize::MAX - 1;
use crate::python_hash::{IMAG, INF, MODULUS};

/// A pending operation in the left-to-right hash of a boxed key.
enum HashFrame {
    Xor(Id),
    Sequence {
        tuple: Option<Id>,
        len: usize,
        acc: u64,
        remaining: smallvec::IntoIter<[Id; 3]>,
    },
}

#[derive(Clone)]
pub(super) struct Index {
    slots: Vec<usize>,
    hashes: Vec<i64>,
    generation: usize,
}

impl Index {
    pub(super) fn buffer_bytes(&self) -> usize {
        self.slots.capacity() * size_of::<usize>() + self.hashes.capacity() * size_of::<i64>()
    }
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
    fn new(hashes: Vec<i64>, positions: Option<&[usize]>, size: usize) -> Result<Self> {
        let mut index = Self {
            slots: Vec::new(),
            hashes,
            generation: 0,
        };
        index.rebuild(positions, size)?;
        Ok(index)
    }

    /// Reserve the slots before replacing the probe layout so a refusal preserves lookups.
    fn rebuild(&mut self, positions: Option<&[usize]>, size: usize) -> Result<()> {
        self.slots
            .try_reserve_exact(size.saturating_sub(self.slots.len()))
            .map_err(|_| crate::Error::memory_error())?;
        self.slots.clear();
        self.slots.resize(size, EMPTY);
        for dense in 0..self.hashes.len() {
            let position = positions.map_or(dense, |p| p[dense]);
            self.place(self.hashes[dense], position);
        }
        Ok(())
    }

    /// Reserve the next hash before any insertion position becomes visible.
    pub(super) fn reserve_hash(&mut self) -> Result<()> {
        self.hashes
            .try_reserve(1)
            .map_err(|_| crate::Error::memory_error().into())
    }

    fn place(&mut self, hash: i64, position: usize) {
        let slot = Probe::new(hash, self.slots.len())
            .find(|&i| matches!(self.slots[i], EMPTY | DUMMY))
            .expect("dictionary has available slots");
        self.slots[slot] = position;
    }

    pub(super) fn compact(&mut self, size: usize) -> Result<()> {
        self.rebuild(None, size)?;
        self.generation += 1;
        Ok(())
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
    /// Ordered iterators track node mutations separately from the size of the base dict.
    pub(in super::super) fn order_state(&self) -> usize {
        self.order.as_ref().map_or(0, |order| order.revision)
    }

    /// Remove all pairs and invalidate lookups suspended in a key comparison.
    pub(in super::super) fn clear(&mut self) -> Vec<(Id, Id)> {
        let old = std::mem::take(self);
        self.order = old.order;
        if let Some(order) = &mut self.order {
            order.by_dense.clear();
        }
        if let Some(index) = old.index {
            let size = (self.usable + 1).next_power_of_two();
            let mut index = index;
            index.hashes.clear();
            index.slots.truncate(size);
            index.slots.fill(EMPTY);
            index.generation += 1;
            self.index = Some(index);
        }
        old.values
    }

    /// Copy the ordered entries and their cached probes without calling Python hash slots.
    pub(in super::super) fn try_clone(&self) -> Result<Self> {
        /// Copy one table buffer after ensuring the allocator can hold it.
        fn copy<T: Copy>(items: &[T]) -> Result<Vec<T>> {
            let mut out = Vec::new();
            out.try_reserve_exact(items.len())
                .map_err(|_| crate::Error::memory_error())?;
            out.extend_from_slice(items);
            Ok(out)
        }

        Ok(Self {
            values: copy(&self.values)?,
            positions: self.positions.as_deref().map(copy).transpose()?,
            index: match &self.index {
                Some(index) => Some(Index {
                    slots: copy(&index.slots)?,
                    hashes: copy(&index.hashes)?,
                    generation: index.generation,
                }),
                None => None,
            },
            order: None,
            next: self.next,
            usable: self.usable,
        })
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
            -INF
        } else {
            INF
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
        let nodes = entries.ordered_nodes();
        if (if last { nodes.last() } else { nodes.first() }).is_some_and(|&(node, _)| node == key) {
            return Ok(());
        }
        let (_, dense) = self.find_dictionary_key(dict, key, None)?;
        let entries = self.stored_entries(dict);
        let rank = dense
            .and_then(|i| entries.ordered_rank(i))
            .ok_or_else(|| self.lookup_error("KeyError", key))?;
        self.stored_entries_mut(dict).move_to_end(rank, last);
        Ok(())
    }

    /// Detach the node before the backing lookup can run an equality callback.
    pub(in super::super) fn unlink_ordered_node(&mut self, dict: Id, dense: usize, rank: usize) {
        let key = self
            .stored_entries_mut(dict)
            .unlink_ordered_node(dense, rank);
        self.heap.weak_edge_removed(key);
    }

    pub(in super::super) fn clear_ordered_dict(&mut self, dict: Id) {
        let removed = self
            .stored_entries_mut(dict)
            .order
            .as_mut()
            .map(|order| {
                order.revision = order.revision.wrapping_add(1);
                std::mem::take(&mut order.keys)
            })
            .unwrap_or_default();
        let finalize = self.heap.has_unbound_finalizer(super::super::NONE)
            && removed
                .iter()
                .any(|&(key, _)| self.heap.could_own_finalizer(key, true));
        for (key, _) in removed {
            self.heap.weak_edge_removed(key);
        }
        super::super::builtins::clear_container(self, dict);
        if finalize {
            self.collect_unreachable();
        }
    }

    fn stored_entries_mut(&mut self, dict: Id) -> &mut Entries {
        match self.heap.get_mut(dict) {
            Value::Set { entries, .. }
            | Value::Dict(entries)
            | Value::DefaultDict { entries, .. } => entries,
            _ => unreachable!("dictionary storage"),
        }
    }

    fn unhashable(&self, key: Id) -> Error {
        Error::new(
            "TypeError",
            format!("unhashable type: '{}'", self.type_name(key)),
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
                .unwrap_or_else(|| self.type_name(key));
            if matches!(module.as_str(), "builtins" | "__main__") {
                name.to_owned()
            } else {
                format!("{module}.{name}")
            }
        } else {
            self.type_name(key).to_owned()
        };
        Error::new(
            "TypeError",
            format!("cannot use '{name}' as a {} ({})", role, error.message),
        )
    }

    fn tuple_keys(&self, key: Id) -> Result<Option<SmallVec<[Id; 3]>>> {
        let mut items = SmallVec::new();
        match self.tuple_backing(key) {
            Value::Tuple(values) | Value::NamedTuple { values, .. } => {
                items
                    .try_reserve(values.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                items.extend_from_slice(values);
            }
            Value::Instance { class, fields, .. } => {
                let Some(names) = self.namedtuple_fields(*class) else {
                    return Ok(None);
                };
                items
                    .try_reserve(names.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                items.extend(names.iter().map(|name| fields[name]));
            }
            _ => return Ok(None),
        }
        Ok(Some(items))
    }

    /// Record a successful hash before the container can be probed again.
    fn cache_key_hash(&mut self, key: Id, hash: i64) -> Result<i64> {
        self.heap
            .immutable_hashes
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        self.heap.immutable_hashes.insert(key, hash);
        Ok(hash)
    }

    /// Hash the stable arena handle without calling any Python special methods.
    pub(in super::super) fn identity_hash(key: Id) -> i64 {
        normalized(crate::python_hash::bytes(&key.to_ne_bytes()))
    }

    pub(in super::super) fn dictionary_hash(&mut self, mut key: Id) -> Result<i64> {
        let mut parents = SmallVec::new();
        'visit: loop {
            let mut hash = match self.dictionary_hash_step(key, &mut parents)? {
                ControlFlow::Continue(child) => {
                    key = child;
                    continue;
                }
                ControlFlow::Break(hash) => hash,
            };
            while let Some(parent) = parents.last_mut() {
                match parent {
                    HashFrame::Xor(id) => {
                        hash = normalized(hash ^ crate::python_hash::bytes(&id.to_ne_bytes()))
                    }
                    HashFrame::Sequence {
                        tuple,
                        len,
                        acc,
                        remaining,
                    } => {
                        *acc = tuple_lane(*acc, hash);
                        if let Some(child) = remaining.next() {
                            key = child;
                            continue 'visit;
                        }
                        hash = if let Some(key) = *tuple {
                            self.cache_key_hash(key, finish_tuple(*acc, *len))?
                        } else if *acc == u64::MAX {
                            1_546_275_796
                        } else {
                            *acc as i64
                        };
                    }
                }
                parents.pop();
            }
            return Ok(hash);
        }
    }

    /// Hash a leaf or save its enclosing operation and visit the next child.
    fn dictionary_hash_step(
        &mut self,
        key: Id,
        parents: &mut SmallVec<[HashFrame; 2]>,
    ) -> Result<ControlFlow<i64, Id>> {
        if let Value::MappingProxy(mapping) = *self.heap.get(key) {
            return Ok(ControlFlow::Continue(mapping));
        }
        if let Value::ParamSpecMarker(kind, _) = self.heap.get(key) {
            return Err(Error::new(
                "TypeError",
                format!("unhashable type: '{}'", kind.class()),
            ));
        }
        if let Value::RegexPattern(pattern) = self.heap.get(key) {
            if pattern.source == NONE {
                return Ok(ControlFlow::Break(Self::identity_hash(key)));
            }
            let (source, program_hash) = (pattern.source, pattern.program_hash());
            return Ok(ControlFlow::Break(normalized(
                self.dictionary_hash(source)? ^ program_hash,
            )));
        }
        if let Value::Union(union) = *self.heap.get(key) {
            return union.hash(self).map(ControlFlow::Break);
        }
        if matches!(self.heap.get(key), Value::BytesIoBuffer { .. }) {
            return Ok(ControlFlow::Break(Self::identity_hash(key)));
        }
        if let Value::MemoryView { owner, hash, .. } = *self.heap.get(key) {
            if let Some(hash) = hash {
                return Ok(ControlFlow::Break(hash));
            }
            let owner = Self::memoryview_owner(owner)?;
            if !self.view_is_readonly(key)? {
                return Err(Error::new(
                    "ValueError",
                    "cannot hash writable memoryview object",
                ));
            }
            let (format, _) = self.view_format_width(owner, self.view_layout(key)?);
            if !matches!(format.trim_start_matches('@'), "B" | "b" | "c") {
                return Err(Error::new(
                    "ValueError",
                    "memoryview: hashing is restricted to formats 'B', 'b' or 'c'",
                ));
            }
            let hash = self.with_buffer_export(key, |ctx| {
                ctx.dictionary_hash(owner)?;
                Ok(crate::python_hash::bytes(&ctx.memoryview_bytes(key)?))
            })?;
            if let Value::MemoryView { hash: cached, .. } = self.heap.get_mut(key) {
                *cached = Some(hash);
            }
            return Ok(ControlFlow::Break(hash));
        }
        if let Value::Weak(wr) = *self.heap.get(key) {
            if wr.kind != super::super::weakref::Kind::Reference {
                return Err(self.unhashable(key));
            }
            return self.weak_reference_hash(key, wr).map(ControlFlow::Break);
        }
        if let Some(class) = self.heap.get(key).instance_class()
            && let Some(slot) = self.class_attr(class, "__hash__")
        {
            if slot == NONE {
                return Err(self.unhashable(key));
            }
            if matches!(self.heap.get(slot), Value::Native(name) if name == "object.__hash__") {
                return Ok(ControlFlow::Break(Self::identity_hash(key)));
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
            return Ok(ControlFlow::Break(normalized(hash)));
        }
        // Immutable scalar subclasses inherit the hash of their backing value.
        let receiver = key;
        let key = self.scalar_value(key);
        let child = match self.heap.get(key) {
            Value::Method {
                callable, receiver, ..
            } => Some((*callable, *receiver)),
            Value::GenericAlias(alias)
                if matches!(alias.kind, super::super::typing::AliasKind::Literal(_)) =>
            {
                let members = super::super::typing::literal_members(self, alias.args)?;
                return Ok(ControlFlow::Continue(members));
            }
            Value::GenericAlias(alias) => Some((alias.args, alias.origin)),
            _ => None,
        };
        if let Some((child, xor)) = child {
            parents
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            parents.push(HashFrame::Xor(xor));
            return Ok(ControlFlow::Continue(child));
        }
        if let Some(&hash) = self.heap.immutable_hashes.get(&key) {
            return Ok(ControlFlow::Break(hash));
        }
        let sequence = if let Value::Slice(parts) = *self.heap.get(key) {
            // Slice components are hashed again on each lookup, in start/stop/step order.
            Some((SmallVec::from_buf(parts), None))
        } else {
            self.tuple_keys(key)?.map(|items| (items, Some(key)))
        };
        if let Some((items, tuple)) = sequence {
            let len = items.len();
            let mut remaining = items.into_iter();
            if let Some(child) = remaining.next() {
                parents
                    .try_reserve(1)
                    .map_err(|_| Error::new("MemoryError", ""))?;
                parents.push(HashFrame::Sequence {
                    tuple,
                    len,
                    acc: TUPLE_START,
                    remaining,
                });
                return Ok(ControlFlow::Continue(child));
            }
            return self
                .cache_key_hash(key, finish_tuple(TUPLE_START, 0))
                .map(ControlFlow::Break);
        }
        let hash = if let Value::Set {
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
            self.cache_key_hash(key, hash)?
        } else {
            match self.heap.get(key) {
                Value::Int(n) => integer_hash(n),
                Value::Bool(b) => i64::from(*b),
                Value::Float(f) => float_hash(*f, key),
                Value::Complex(z) => normalized(float_hash(z.real, key))
                    .wrapping_add(IMAG.wrapping_mul(normalized(float_hash(z.imag, key)))),
                Value::Native(_)
                    if matches!(
                        self.builtin_type_name(key),
                        Some("type" | "wrapper_descriptor" | "method_descriptor")
                    ) =>
                {
                    Self::identity_hash(key)
                }
                Value::Native(name) => {
                    crate::python_hash::text(crate::dynamic::builtins::native_implementation(name))
                }
                Value::Str(text) => crate::python_hash::text(text),
                Value::SurrogateStr(points) => {
                    crate::python_hash::codepoints(points.iter().copied())
                }
                Value::Bytes(bytes) => crate::python_hash::bytes(bytes),
                Value::None => 0xfca8_6420,
                Value::Calendar { value, tzinfo, .. } => {
                    let value = value.clone();
                    let kind = value.class();
                    let mut duration = value.hash_delta_parts();
                    if matches!(kind, "time" | "datetime")
                        && (receiver != key
                            || crate::dynamic::stdlib::datetime::comparison::custom(self, *tzinfo))
                        && let Some(offset) =
                            crate::dynamic::stdlib::datetime::comparison::hash_offset(
                                self, receiver,
                            )?
                    {
                        duration = Some(value.clock_hash_parts(offset));
                    }
                    let hash = if let Some((d, s, u)) = duration {
                        finish_tuple(
                            [d, s, u]
                                .into_iter()
                                .fold(TUPLE_START, |acc, n| tuple_lane(acc, normalized(n as i64))),
                            3,
                        )
                    } else {
                        let [year, month, day, hour, minute, second, microsecond, _] =
                            if kind == "time" {
                                let [h, m, s, us, _] = value.clock_parts().map(i128::from);
                                [0, 0, 0, h, m, s, us, 0]
                            } else {
                                value.fields()
                            };
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
                        crate::python_hash::bytes(match kind {
                            "date" => &bytes[..4],
                            "time" => &bytes[4..],
                            _ => &bytes,
                        })
                    };
                    self.cache_key_hash(key, normalized(hash))?
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
        Ok(ControlFlow::Break(normalized(hash)))
    }

    /// Look up a Python name in immutable builtin type dictionaries. Type lookup clears errors.
    pub(in super::super) fn lookup_builtin_type_name(
        &mut self,
        key: Id,
        groups: &[&[&'static str]],
    ) -> Option<&'static str> {
        let mut names = groups.iter().flat_map(|group| group.iter().copied());
        if let Value::Str(text) = self.heap.get(key) {
            return names.find(|&name| name == text);
        }
        let hash = self.dictionary_hash(key).ok()?;
        for name in names {
            if normalized(crate::python_hash::text(name)) == hash {
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
            Tuple(std::iter::Zip<smallvec::IntoIter<[Id; 3]>, smallvec::IntoIter<[Id; 3]>>),
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
                    } else if let (Value::RegexPattern(a), Value::RegexPattern(b)) =
                        (self.heap.get(left), self.heap.get(right))
                    {
                        if a.same_program(b) {
                            Work::Pair(a.source, b.source)
                        } else {
                            Work::Done(false)
                        }
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
                        (self.tuple_keys(left)?, self.tuple_keys(right)?)
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
        let entries = self.stored_entries(dict);
        let mut keys = Vec::new();
        keys.try_reserve_exact(entries.len())
            .map_err(|_| crate::Error::memory_error())?;
        keys.extend(entries.iter().map(|&(key, _)| key));
        let mut hashes = Vec::new();
        hashes
            .try_reserve_exact(keys.len())
            .map_err(|_| crate::Error::memory_error())?;
        for key in keys {
            hashes.push(self.dictionary_hash(key)?);
        }
        let entries = self.stored_entries_mut(dict);
        let size = (entries.usable + 1).next_power_of_two();
        entries.index = Some(Index::new(hashes, entries.positions.as_deref(), size)?);
        Ok(())
    }

    pub(in super::super) fn find_dictionary_key(
        &mut self,
        dict: Id,
        key: Id,
        known_hash: Option<i64>,
    ) -> Result<(i64, Option<usize>)> {
        let roots = self.heap.temporary_mark();
        self.heap.keep(dict);
        self.heap.keep(key);
        let result = KeyLookup::new(self, dict, key, known_hash).and_then(|mut lookup| {
            let mut previous = None;
            loop {
                match lookup.next(self, previous) {
                    ControlFlow::Break(found) => return Ok((lookup.hash, found)),
                    ControlFlow::Continue((position, resident)) => {
                        previous =
                            Some((position, resident, self.dictionary_equal(resident, key)?));
                    }
                }
            }
        });
        self.heap.release_temporaries_since(roots);
        result
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
        self.store_dictionary_value(dict, key, value, hash, index, overwrite)
    }

    pub(in super::super) fn set_ordered_dictionary_key(
        &mut self,
        dict: Id,
        key: Id,
        value: Id,
    ) -> Result<Id> {
        self.heap.keep(value);
        let (hash, index) = self.find_dictionary_key(dict, key, None)?;
        self.store_dictionary_value(dict, key, value, hash, index, true)?;
        // The backing value is visible to equality hooks during the separate node lookup.
        if let Err(error) = self.add_ordered_node(dict, key, hash) {
            for root in error.roots() {
                self.heap.keep(root);
            }
            super::super::builtins::remove_dict_item_hashed(self, dict, key, Some(hash))?;
            return Err(error);
        }
        Ok(value)
    }

    fn add_ordered_node(&mut self, dict: Id, key: Id, hash: i64) -> Result<()> {
        let (_, dense) = self.find_dictionary_key(dict, key, Some(hash))?;
        let dense = dense.ok_or_else(|| self.lookup_error("KeyError", key))?;
        let entries = self.stored_entries_mut(dict);
        if entries.ordered_rank(dense).is_none() {
            entries.reserve_ordered_node()?;
            entries.link_ordered_key(dense, key, hash);
        }
        Ok(())
    }

    fn store_dictionary_value(
        &mut self,
        dict: Id,
        key: Id,
        value: Id,
        hash: i64,
        index: Option<usize>,
        overwrite: bool,
    ) -> Result<Id> {
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
