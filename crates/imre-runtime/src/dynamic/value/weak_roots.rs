use super::{Heap, Id, SlotShape, Value};
use indexmap::IndexMap;
use std::collections::HashSet;

// Retain the small weak-only search when no finalizer paths have been learned.
const MAX_WEAK_ENDPOINTS: usize = 16;
const MAX_NODES: usize = 8192;
const MAX_EDGES: usize = 65536;
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
    nodes: HashSet<Id>,
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
    Names(&'a IndexMap<String, Id>),
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
                Members::Names(&scope.names),
            ),
            Value::Dict(entries) if dict_keys => ([None; 2], Members::Entries(entries)),
            Value::Dict(entries) => ([None; 2], Members::Values(entries)),
            Value::Set { entries, .. } => ([None; 2], Members::Keys(entries)),
            Value::List(items) | Value::Tuple(items) => ([None; 2], Members::Sequence(items)),
            Value::Instance {
                fields,
                namespace,
                dictionary,
                ..
            } => ([*namespace, *dictionary], Members::Names(fields)),
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
                Members::Names(names) => names.len(),
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
            Members::Names(names) => names.get_index(index).map(|(_, id)| *id),
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
        if from_frames && !wanted.is_empty() {
            let roots = heap
                .frames
                .iter()
                .enumerate()
                .rev()
                .map(|(i, frame)| (frame.scope, Root::Frame(i)));
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
            let roots = globals
                .into_iter()
                .enumerate()
                .map(|(i, id)| (id, Root::Global(i)));
            paths.search(
                heap,
                roots,
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
        // An unrepresented owner can consume the search budget. Keep the paths already
        // found; the caller must still account for every current owner before skipping a trace.
        'search: while !wanted.is_empty()
            && front < tree.len()
            && *edges_left != 0
            && *nodes_left != 0
        {
            let (&parent, _) = tree.get_index(front)?;
            let index = front;
            front += 1;
            let value = heap.get(parent);
            if !follow_modules && matches!(value, Value::Module { .. }) {
                continue;
            }
            let links = Links::from(value, self.dict_keys);
            for slot in 0..links.len().min(*edges_left) {
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
                    && (heap.slot_shapes[child as usize] == SlotShape::ScalarLeaf
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
        for (&id, &(origin, keep)) in &tree {
            if !keep || self.nodes.contains(&id) {
                continue;
            }
            self.nodes.try_reserve(1).ok()?;
            self.nodes.insert(id);
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
                self.weak_roots.nodes.contains(target)
                    && refs.iter().all(|id| self.weak_roots.nodes.contains(id))
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
                    !paths.nodes.contains(&id)
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
                    && !self.operand_keeps_finalizer(*id, true)
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

    fn operand_keeps_finalizer(&self, owner: Id, before_discard: bool) -> bool {
        self.frames
            .iter()
            .rev()
            .skip(usize::from(before_discard))
            .take(24)
            .any(|frame| {
                frame
                    .live
                    .iter()
                    .take(8)
                    .any(|root| self.short_path_keeps_owner(*root, owner))
            })
    }

    /// Scopes and globals survive dropping the current frame's temporaries. During the
    /// acyclic-finalizer check the current frame's published operands are also roots.
    pub(in crate::dynamic) fn cached_finalizers_kept(
        &self,
        globals: [Id; 2],
        stdin: Option<&[Id; 3]>,
        before_discard: bool,
    ) -> bool {
        if !self.finalizer_roots.valid(self, globals) {
            return false;
        }
        let kept = |owner: &Id| {
            self.finalizer_roots.nodes.contains(owner)
                || stdin.is_some_and(|input| input.contains(owner))
                || self.operand_keeps_finalizer(*owner, before_discard)
                || self.bound_in_active_frame(*owner)
        };
        self.ordinary_finalizers.iter().all(&kept)
            && (!before_discard
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
