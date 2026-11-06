use super::{Heap, Id, Value};
use crate::dynamic::stdlib::{BinaryFile, io::MemoryStream};
use indexmap::IndexMap;
use std::collections::{HashSet, VecDeque};

// Retain the small weak-only search when no finalizer paths have been learned.
const MAX_WEAK_ENDPOINTS: usize = 16;
const MAX_NODES: usize = 8192;
const MAX_EDGES: usize = 65536;
const CHILDREN_PER_VISIT: usize = 256;
const SEARCH_BACKOFF: u8 = 63;

#[derive(Clone, Copy)]
enum Root {
    Global(usize),
    Frame(usize),
}

#[derive(Clone, Copy)]
enum Origin {
    Root(Root),
    Edge { parent: usize, slot: usize },
}

#[derive(Default)]
pub(super) struct RootPaths {
    anchors: Vec<(Root, Id)>,
    // Kept sorted between searches for membership checks at collection points.
    nodes: Vec<Id>,
    edges: Vec<(Id, usize, Id)>,
    retry: u8,
    // Dictionary slots must be interpreted the same way during discovery and validation.
    dict_keys: bool,
    retry_with_keys: bool,
}

/// Strong edges in ordinary containers and namespaces. A missing edge merely loses the proof.
struct Links<'a> {
    direct: [Option<Id>; 2],
    members: Members<'a>,
}

enum Members<'a> {
    None,
    Sequence(&'a [Id]),
    Names(&'a IndexMap<String, Id>, Option<Id>),
    Keys(&'a [(Id, Id)]),
    Values(&'a [(Id, Id)]),
    Entries(&'a [(Id, Id)]),
}

impl<'a> Links<'a> {
    fn from(value: &'a Value, dict_keys: bool) -> Self {
        let (direct, members) = match value {
            Value::Module { scope, .. } => ([Some(*scope), None], Members::None),
            Value::Scope(scope) => (
                [scope.namespace, scope.exec_globals.map(|g| g.dict)],
                Members::Names(&scope.names, None),
            ),
            Value::Dict(entries) if dict_keys => ([None; 2], Members::Entries(entries)),
            Value::Dict(entries) => ([None; 2], Members::Values(entries)),
            Value::Set { entries, .. } => ([None; 2], Members::Keys(entries)),
            Value::List(items) | Value::Tuple(items) => ([None; 2], Members::Sequence(items)),
            Value::Map { inputs, .. } | Value::Zip { inputs, .. } => {
                ([None; 2], Members::Sequence(inputs))
            }
            Value::Generator { iterators, .. } => ([None; 2], Members::Sequence(iterators)),
            Value::LoopIterator { source, peeked } => ([Some(*source), *peeked], Members::None),
            Value::Instance {
                fields,
                namespace,
                dictionary,
                memory_stream,
                ..
            } => {
                let stream = match memory_stream {
                    Some(MemoryStream::BufferedFile(file)) => Some(*file),
                    Some(MemoryStream::Reader(reader)) => reader.raw.get(),
                    Some(MemoryStream::BufferedBytes(writer)) => writer.raw.get(),
                    Some(
                        MemoryStream::UninitializedReader(raw)
                        | MemoryStream::UninitializedWriter(raw),
                    ) => *raw,
                    Some(MemoryStream::Wrapper(wrapper)) => wrapper.buffer_id(),
                    _ => None,
                };
                ([*namespace, *dictionary], Members::Names(fields, stream))
            }
            Value::File(file) if let BinaryFile::Descriptor(fd) = file.as_ref() => {
                ([Some(fd.raw), None], Members::None)
            }
            Value::Function {
                defaults,
                kwdefaults,
                ..
            } => ([Some(*defaults), Some(*kwdefaults)], Members::None),
            _ => ([None; 2], Members::None),
        };
        Self { direct, members }
    }

    fn len(&self) -> usize {
        self.direct.len()
            + match self.members {
                Members::None => 0,
                Members::Sequence(items) => items.len(),
                Members::Names(names, buffer) => names.len() + usize::from(buffer.is_some()),
                Members::Keys(entries) | Members::Values(entries) => entries.len(),
                Members::Entries(entries) => entries.len() * 2,
            }
    }

    fn is_dict_key(&self, slot: usize) -> bool {
        matches!(self.members, Members::Entries(_))
            && slot >= self.direct.len()
            && (slot - self.direct.len()).is_multiple_of(2)
    }

    fn get(&self, index: usize) -> Option<Id> {
        if let Some(&edge) = self.direct.get(index) {
            return edge;
        }
        let index = index - self.direct.len();
        match self.members {
            Members::None => None,
            Members::Sequence(items) => items.get(index).copied(),
            Members::Names(names, buffer) if index == names.len() => buffer,
            Members::Names(names, _) => names.get_index(index).map(|(_, id)| *id),
            Members::Keys(entries) => entries.get(index).map(|(key, _)| *key),
            Members::Values(entries) => entries.get(index).map(|(_, value)| *value),
            Members::Entries(entries) => {
                let &(key, value) = entries.get(index / 2)?;
                Some(if index.is_multiple_of(2) { key } else { value })
            }
        }
    }
}

impl RootPaths {
    fn contains(&self, id: Id) -> bool {
        self.nodes.binary_search(&id).is_ok()
    }

    /// Check roots before edges, and parents before children, so a stale slot is never dereferenced.
    fn valid(&self, heap: &Heap, globals: [Id; 2]) -> bool {
        !self.nodes.is_empty()
            && self.anchors.iter().all(|&(origin, root)| match origin {
                Root::Frame(index) => heap
                    .frames
                    .get(index)
                    .is_some_and(|frame| frame.scope == root),
                Root::Global(index) => globals[index] == root,
            })
            && self.edges.iter().all(|&(parent, slot, child)| {
                Links::from(heap.get(parent), self.dict_keys).get(slot) == Some(child)
            })
    }

    fn discover(
        heap: &Heap,
        globals: [Id; 2],
        endpoints: impl Iterator<Item = Id>,
        from_frames: bool,
        dict_keys: bool,
    ) -> Option<Self> {
        let mut edges_left = MAX_EDGES;
        let mut nodes_left = heap.slots.len().min(MAX_NODES);
        let mut wanted = HashSet::new();
        for id in endpoints {
            edges_left = edges_left.checked_sub(1)?;
            wanted.try_reserve(1).ok()?;
            wanted.insert(id);
        }
        let mut paths = Self {
            dict_keys,
            ..Self::default()
        };
        let global_roots = globals
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, Root::Global(i)));
        if from_frames && !wanted.is_empty() {
            let roots = heap
                .frames
                .iter()
                .enumerate()
                .rev()
                .map(|(i, frame)| (frame.scope, Root::Frame(i)))
                // Visit directly held globals before descending into large frame-local containers.
                .chain(global_roots.clone());
            paths.search(
                heap,
                roots,
                &mut wanted,
                &mut nodes_left,
                &mut edges_left,
                false,
            )?;
        }
        if !wanted.is_empty() && nodes_left != 0 && edges_left != 0 {
            paths.search(
                heap,
                global_roots,
                &mut wanted,
                &mut nodes_left,
                &mut edges_left,
                true,
            )?;
        }
        Some(paths)
    }

    /// The insertion-ordered tree is also the BFS queue. Propagate required endpoints to
    /// their ancestors in one reverse pass, then emit the surviving edges parent-first.
    fn search(
        &mut self,
        heap: &Heap,
        roots: impl Iterator<Item = (Id, Root)>,
        wanted: &mut HashSet<Id>,
        nodes_left: &mut usize,
        edges_left: &mut usize,
        follow_modules: bool,
    ) -> Option<()> {
        let mut tree = IndexMap::<Id, (Origin, bool)>::new();
        for (root, origin) in roots {
            if *edges_left == 0 || *nodes_left == 0 {
                break;
            }
            *edges_left -= 1;
            if !tree.contains_key(&root) {
                *nodes_left -= 1;
                tree.try_reserve(1).ok()?;
                tree.insert(root, (Origin::Root(origin), wanted.remove(&root)));
            }
        }
        let mut front = 0;
        let mut deferred = VecDeque::new();
        // An unrepresented owner can consume the search budget. Keep the paths already
        // found; the caller must still account for every current owner before skipping a trace.
        'search: while !wanted.is_empty() && *edges_left != 0 && *nodes_left != 0 {
            // Explore newly discovered nodes before resuming a wide container's remaining edges.
            let (index, start) = if front < tree.len() {
                let index = front;
                front += 1;
                (index, 0usize)
            } else if let Some(next) = deferred.pop_front() {
                next
            } else {
                break;
            };
            let (&parent, _) = tree.get_index(index)?;
            let value = heap.get(parent);
            if !follow_modules && matches!(value, Value::Module { .. }) {
                continue;
            }
            let links = Links::from(value, self.dict_keys);
            let end = links.len().min(start.saturating_add(CHILDREN_PER_VISIT));
            if end < links.len() {
                deferred.try_reserve(1).ok()?;
                deferred.push_back((index, end));
            }
            for slot in start..end.min(start.saturating_add(*edges_left)) {
                *edges_left -= 1;
                let Some(child) = links.get(slot) else {
                    continue;
                };
                if tree.contains_key(&child) {
                    continue;
                }
                if *nodes_left == 0 {
                    break 'search;
                }
                let found = wanted.remove(&child);
                // A registry key can prove an endpoint without traversing unrelated keys.
                if !found
                    && (heap.slot_shapes[child as usize].is_scalar_leaf()
                        || links.is_dict_key(slot))
                {
                    continue;
                }
                *nodes_left -= 1;
                tree.try_reserve(1).ok()?;
                tree.insert(
                    child,
                    (
                        Origin::Edge {
                            parent: index,
                            slot,
                        },
                        found,
                    ),
                );
                if wanted.is_empty() {
                    break;
                }
            }
        }
        for index in (0..tree.len()).rev() {
            if let (_, &(Origin::Edge { parent, .. }, true)) = tree.get_index(index)? {
                let (_, (_, needed)) = tree.get_index_mut(parent)?;
                *needed = true;
            }
        }
        // The tree contains each ID once. Only paths kept by an earlier
        // search can duplicate one of the IDs being appended here.
        let known = self.nodes.len();
        for (&id, &(origin, keep)) in &tree {
            if !keep || self.nodes[..known].binary_search(&id).is_ok() {
                continue;
            }
            self.nodes.try_reserve(1).ok()?;
            self.nodes.push(id);
            match origin {
                Origin::Edge { parent, slot } => {
                    let (&parent, _) = tree.get_index(parent)?;
                    self.edges.try_reserve(1).ok()?;
                    self.edges.push((parent, slot, id));
                }
                Origin::Root(root) => {
                    self.anchors.try_reserve(1).ok()?;
                    self.anchors.push((root, id));
                }
            }
        }
        self.nodes.sort_unstable();
        Some(())
    }

    fn search_due(&mut self) -> bool {
        if self.retry > 0 {
            self.retry -= 1;
            return false;
        }
        self.retry = SEARCH_BACKOFF;
        true
    }
}

impl Heap {
    pub(super) fn cached_weak_roots(&self, globals: [Id; 2]) -> bool {
        self.weak_roots.valid(self, globals)
            && self.weak.iter().all(|(target, refs)| {
                self.weak_roots.contains(*target)
                    && refs.iter().all(|id| self.weak_roots.contains(*id))
            })
    }

    pub(in crate::dynamic) fn learn_weak_roots(&mut self, modules: Id, reference: Id) {
        let globals = [modules, reference];
        if self.weak.is_empty() || self.weak_values_kept_by(modules, reference) {
            return;
        }
        let from_frames = self.finalizer_roots.valid(self, globals);
        let endpoints = self
            .weak
            .iter()
            .flat_map(|(target, refs)| std::iter::once(*target).chain(refs.iter().copied()));
        if !from_frames
            && endpoints.clone().take(MAX_WEAK_ENDPOINTS + 1).count() > MAX_WEAK_ENDPOINTS
        {
            return;
        }
        if !self.weak_roots.search_due() {
            return;
        }
        let dict_keys = self.weak_roots.retry_with_keys;
        if let Some(paths) = RootPaths::discover(
            self,
            globals,
            endpoints,
            from_frames || dict_keys,
            dict_keys,
        ) {
            // A weak-key registry can hold the reference while a caller holds its target.
            // Only spend a later search budget on keys after an ordinary path was insufficient.
            let retry_with_keys = !dict_keys
                && self.weak.values().flatten().any(|&id| {
                    !paths.contains(id)
                        && self
                            .get(id)
                            .weak_reference()
                            .is_some_and(|wr| wr.hash.is_some())
                });
            self.weak_roots = RootPaths {
                retry: SEARCH_BACKOFF,
                retry_with_keys,
                ..paths
            };
        }
    }

    pub(in crate::dynamic) fn learn_finalizer_paths(
        &mut self,
        globals: [Id; 2],
        stdin: Option<&[Id; 3]>,
    ) -> bool {
        // Retry from the current roots when a previously proven path loses an anchor or edge.
        if !self.finalizer_roots.nodes.is_empty() && !self.finalizer_roots.valid(self, globals) {
            self.finalizer_roots = RootPaths::default();
        }
        if !self.finalizer_roots.search_due() {
            return false;
        }
        let endpoints = self
            .ordinary_finalizers
            .iter()
            .chain(&self.descriptor_owners)
            .copied()
            .filter(|id| {
                !stdin.is_some_and(|input| input.contains(id))
                    && !self.operand_keeps_finalizer(*id, 0)
            });
        let Some(paths) = RootPaths::discover(self, globals, endpoints, true, false) else {
            return false;
        };
        self.finalizer_roots = RootPaths {
            retry: SEARCH_BACKOFF,
            ..paths
        };
        self.learn_weak_roots(globals[0], globals[1]);
        !self.finalizer_roots.nodes.is_empty()
    }

    /// Check published operands in the caller frames and the surviving prefix of the active frame.
    fn operand_keeps_finalizer(&self, owner: Id, retained_current: usize) -> bool {
        let Some(active) = self.frames.len().checked_sub(1) else {
            return false;
        };
        // The operand and its children can change between releases.
        let cached = &self.finalizer_operands[owner as usize % self.finalizer_operands.len()];
        if let Some((last_owner, frame, operand)) = cached.get()
            && last_owner == owner
            && (frame < active || operand < retained_current)
            && let Some(&root) = self.frames.get(frame).and_then(|f| f.live.get(operand))
            && self.short_path_keeps_owner(root, owner)
        {
            return true;
        }

        const MAX_CALLER_OPERANDS: usize = 192;
        let active_limit = self.frames[active].live.len().min(retained_current).min(8);
        let mut operands = self
            .frames
            .iter()
            .enumerate()
            .rev()
            .take(MAX_CALLER_OPERANDS + 1)
            .flat_map(|(index, frame)| {
                let limit = if index == active { active_limit } else { 8 };
                frame
                    .live
                    .iter()
                    .take(limit)
                    .enumerate()
                    .map(move |(slot, &root)| (index, slot, root))
            })
            .take(active_limit + MAX_CALLER_OPERANDS);
        let found = operands
            .clone()
            .find(|&(_, _, root)| {
                root == owner
                    || matches!(self.get(root), Value::LoopIterator { source, peeked }
                if *source == owner || *peeked == Some(owner))
            })
            .or_else(|| operands.find(|&(_, _, root)| self.short_path_keeps_owner(root, owner)));
        if let Some((frame, slot, _)) = found {
            cached.set(Some((owner, frame, slot)));
            return true;
        }
        false
    }

    /// A pending discard preserves the active frame's operands below its mark. It must also
    /// preserve descriptor owners and weak references before a release can skip its sweep.
    pub(in crate::dynamic) fn cached_finalizers_kept(
        &self,
        globals: [Id; 2],
        stdin: Option<&[Id; 3]>,
        discard_from: Option<usize>,
    ) -> bool {
        let paths_valid = self.finalizer_roots.valid(self, globals);
        let kept = |owner: &Id| {
            (paths_valid && self.finalizer_roots.contains(*owner))
                || stdin.is_some_and(|input| input.contains(owner))
                || self.operand_keeps_finalizer(*owner, discard_from.unwrap_or(usize::MAX))
                || self.bound_in_active_frame(*owner)
        };
        self.ordinary_finalizers.iter().all(&kept)
            && (discard_from.is_none()
                || (self.descriptor_owners.iter().all(kept)
                    && (!self.has_weak_values || self.weak_values_kept_by(globals[0], globals[1]))))
    }
}

#[cfg(test)]
mod cohort_tests;

#[cfg(test)]
mod tests {
    use super::super::{Heap, NONE, TRUE, Value};
    use crate::dynamic::weakref::Kind;

    #[test]
    fn weak_paths_recheck_both_endpoints_and_every_strong_edge() {
        let mut heap = Heap::new();
        let target = heap.alloc(Value::List(Vec::new()));
        let reference = heap
            .new_weak_reference(target, NONE, Kind::Reference, None)
            .unwrap();
        let holder = heap.alloc(Value::List(vec![target, reference]));
        let modules = heap.alloc(Value::Dict(vec![(NONE, holder)].into()));
        heap.learn_weak_roots(modules, NONE);
        assert!(heap.weak_values_kept_by(modules, NONE));

        for slot in [0, 1] {
            let Value::List(values) = heap.get_mut(holder) else {
                unreachable!()
            };
            let old = std::mem::replace(&mut values[slot], NONE);
            assert!(!heap.weak_values_kept_by(modules, NONE));
            let Value::List(values) = heap.get_mut(holder) else {
                unreachable!()
            };
            values[slot] = old;
        }
        assert!(heap.weak_values_kept_by(modules, NONE));
        assert!(!heap.weak_values_kept_by(holder, NONE));
        heap.new_weak_reference(target, TRUE, Kind::Reference, None)
            .unwrap();
        assert!(!heap.weak_values_kept_by(modules, NONE));
    }
}
