use super::{CycleSet, END, FindReferences, Heap, Id, MarkMode, Scope, UNMARKED, Value};
use crate::dynamic::stdlib::{BinaryFile, io::MemoryStream};
use indexmap::{IndexMap, map::Entry};
use std::cell::{Ref, RefCell};
use std::collections::{HashSet, VecDeque};

type IdSet = HashSet<Id, hashbrown::DefaultHashBuilder>;

mod retained;
pub(super) use retained::RetainedPaths;

// Retain the small weak-only search when no finalizer paths have been learned.
const MAX_WEAK_ENDPOINTS: usize = 16;
const MAX_NODES: usize = 8192;
const MAX_EDGES: usize = 65536;
const FRAME_PROBE_NODES: usize = 256;
const FRAME_PROBE_EDGES: usize = 2048;
const CHILDREN_PER_VISIT: usize = 256;
const SEARCH_BACKOFF: u8 = 63;

#[derive(Clone, Copy)]
enum Root {
    Global(usize),
    Frame(usize),
    Surviving,
}

#[derive(Clone, Copy)]
enum Origin {
    Root(Root),
    Edge { parent: usize, slot: Option<usize> },
}

#[derive(Default)]
pub(super) struct RootPaths {
    /// The search found every requested endpoint. A new weak reference invalidates this result.
    pub(super) covers_endpoints: bool,
    anchors: Vec<(Root, Id)>,
    // Kept sorted between searches for membership checks at collection points.
    nodes: Vec<Id>,
    edges: Vec<(Id, Option<usize>, Id)>,
    // Indexed once for partial validation when independent finalizer paths change.
    edge_indices: Vec<(usize, usize)>,
    live: RefCell<Vec<bool>>,
    retry: u8,
    // Dictionary slots must be interpreted the same way during discovery and validation.
    dict_keys: bool,
    retry_with_keys: bool,
    // Set after a complete weak-root search. Unproven cohorts use the normal interval.
    weak_backoff: Option<u8>,
}

/// Strong incoming edges that keep pending owners alive under ordinary reference decrements.
#[derive(Default)]
pub(super) struct CycleRetention {
    pub(super) owners: Vec<Id>,
    edges: Vec<CycleEdge>,
    retry: u8,
}

struct CycleEdge {
    parent: Id,
    child: Id,
    slot: Option<usize>,
}

impl CycleRetention {
    /// A declined proof still uses the ordinary classifier on every release.
    pub(super) fn from_residual(heap: &Heap, cycles: &mut CycleSet) -> Self {
        let retry = heap.cyclic_finalizers.retry;
        if retry != 0 {
            return Self {
                retry: retry - 1,
                ..Self::default()
            };
        }
        Self::build(heap, cycles).unwrap_or_else(|| Self {
            retry: SEARCH_BACKOFF,
            ..Self::default()
        })
    }

    fn build(heap: &Heap, cycles: &mut CycleSet) -> Option<Self> {
        // The peeled graph has a predecessor inside the residual set for every survivor.
        // Its queue already has capacity for one position per slot.
        let parents = &mut cycles.work;
        parents.resize(cycles.incoming.len(), END);
        for (parent, &incoming) in cycles.incoming.iter().enumerate() {
            if incoming != 0 {
                heap.unreachable_children(heap.get(parent as Id), MarkMode::CycleEdges, |child| {
                    if cycles.incoming[child] != 0 && parents[child] == END {
                        parents[child] = parent;
                    }
                });
            }
        }

        let mut proof = Self::default();
        for &owner in &heap.ordinary_finalizers {
            let mut child = owner as usize;
            if cycles.incoming[child] == 0 {
                continue;
            }
            proof.owners.try_reserve(1).ok()?;
            proof.owners.push(owner);
            // Follow predecessors until the path closes a cycle or joins an earlier path.
            while parents[child] != UNMARKED {
                let parent = std::mem::replace(&mut parents[child], UNMARKED);
                // Keep optional edge checks small enough to run at operand releases.
                if parent == END || proof.edges.len() == 256 {
                    return None;
                }
                proof.edges.try_reserve(1).ok()?;
                proof.edges.push(CycleEdge {
                    parent: parent as Id,
                    child: child as Id,
                    slot: None,
                });
                child = parent;
            }
        }
        // The predecessor table is no longer needed. Index each parent's wanted children
        // in that same space so a wide container spends its position budget only once.
        proof.edges.sort_unstable_by_key(|edge| edge.parent);
        let mut budget = MAX_EDGES;
        for edges in proof.edges.chunk_by_mut(|a, b| a.parent == b.parent) {
            let links = Links::from(heap.get(edges[0].parent), true);
            for (index, edge) in edges.iter().enumerate() {
                parents[edge.child as usize] = index;
            }
            let mut missing = edges.len();
            let limit = links.len().min(budget);
            for slot in 0..limit {
                budget -= 1;
                if let Some(child) = links.get(slot)
                    && let Some(edge) = edges.get_mut(parents[child as usize])
                    && edge.child == child
                    && edge.slot.is_none()
                {
                    edge.slot = Some(slot);
                    missing -= 1;
                    if missing == 0 {
                        break;
                    }
                }
            }
            // A cycle can also retain an owner through a path leaving the cycle.
            // At that junction both edges may need the full strong-child visitor.
            if missing > 2 || (missing != 0 && limit < links.len()) {
                return None;
            }
        }
        proof.owners.sort_unstable();
        Some(proof)
    }

    pub(super) fn verified_owners(&self, heap: &Heap) -> &[Id] {
        for &CycleEdge {
            parent,
            child,
            slot,
        } in &self.edges
        {
            let Some(value) = heap
                .slots
                .get(parent as usize)
                .and_then(|slot| slot.value.as_ref())
            else {
                return &[];
            };
            if !has_strong_edge(value, child, slot, true) {
                return &[];
            }
        }
        &self.owners
    }
}

fn has_strong_edge(parent: &Value, child: Id, slot: Option<usize>, dict_keys: bool) -> bool {
    if let Some(slot) = slot {
        return Links::from(parent, dict_keys).get(slot) == Some(child);
    }
    let mut edges = FindReferences {
        targets: [child; 2],
        found: false,
    };
    parent.try_children(&mut edges).is_ok() && edges.found
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Search {
    SkipModules,
    Indexed,
    Strong,
}

/// Strong edges in ordinary containers and namespaces. A missing edge merely loses the proof.
struct Links<'a> {
    direct: [Option<Id>; 2],
    members: Members<'a>,
}

enum Members<'a> {
    None,
    Sequence(&'a [Id]),
    Generator(&'a crate::dynamic::native_suspension::Saved),
    Names(&'a IndexMap<String, Id>, Option<Id>),
    Scope(&'a Scope),
    Keys(&'a [(Id, Id)]),
    Values(&'a [(Id, Id)]),
    Entries(&'a [(Id, Id)]),
}

/// Inspect a bounded set of strong edges without traversing a frame holder's entire payload.
pub(super) fn indexed_children(value: &Value) -> impl Iterator<Item = Id> + '_ {
    let links = Links::from(value, false);
    (0..links.len().min(CHILDREN_PER_VISIT)).filter_map(move |index| links.get(index))
}

impl<'a> Links<'a> {
    #[inline(always)]
    fn from(value: &'a Value, dict_keys: bool) -> Self {
        let (direct, members) = match value {
            Value::Module { scope, .. } => ([Some(*scope), None], Members::None),
            Value::Scope(scope) => (
                [scope.namespace(), scope.exec_globals.map(|g| g.dict)],
                Members::Scope(scope),
            ),
            Value::Cell(contents) => ([*contents, None], Members::None),
            Value::Traceback { frame, next, .. } => ([Some(*frame), Some(*next)], Members::None),
            Value::PythonFrame { scope, back, .. } => ([Some(*scope), *back], Members::None),
            Value::Dict(entries) if dict_keys => ([None; 2], Members::Entries(entries)),
            Value::Dict(entries) => ([None; 2], Members::Values(entries)),
            Value::Set { entries, .. } => ([None; 2], Members::Keys(entries)),
            Value::List(items) | Value::Tuple(items) => ([None; 2], Members::Sequence(items)),
            Value::Map { inputs, .. } | Value::Zip { inputs, .. } => {
                ([None; 2], Members::Sequence(inputs))
            }
            Value::Coroutine { .. } | Value::CoroutineWrapper(_) => {
                ([value.suspended_owner(), None], Members::None)
            }
            Value::Generator { native, .. } => (
                [value.suspended_owner(), None],
                native.as_ref().map_or(Members::None, Members::Generator),
            ),
            Value::Method {
                receiver, callable, ..
            } => ([Some(*receiver), Some(*callable)], Members::None),
            Value::Iterator { sequence, .. } => ([*sequence, None], Members::None),
            Value::LoopIterator { source } => ([Some(*source), None], Members::None),
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
                let extra = stream.or_else(|| {
                    // A sparse singleton can still have a large bucket allocation.
                    value
                        .stored_member_edges(|slots| {
                            slots.len() == 1 && slots.allocation_size() <= 256
                        })
                        .next()
                        .map(|[_, value]| value)
                });
                ([*namespace, *dictionary], Members::Names(fields, extra))
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
                Members::Generator(frame) => frame.root_count(),
                Members::Scope(scope) => scope.names().len() + scope.closure().cells.len(),
                Members::Names(names, extra) => names.len() + usize::from(extra.is_some()),
                Members::Keys(entries) | Members::Values(entries) => entries.len(),
                Members::Entries(entries) => entries.len() * 2,
            }
    }

    fn is_dict_key(&self, slot: usize) -> bool {
        matches!(self.members, Members::Entries(_))
            && slot >= self.direct.len()
            && (slot - self.direct.len()).is_multiple_of(2)
    }

    #[inline(always)]
    fn get(&self, index: usize) -> Option<Id> {
        if let Some(&edge) = self.direct.get(index) {
            return edge;
        }
        let index = index - self.direct.len();
        match self.members {
            Members::None => None,
            Members::Sequence(items) => items.get(index).copied(),
            Members::Generator(frame) => frame.root_at(index),
            Members::Scope(scope) => {
                if index < scope.names().len() {
                    scope.names().get_index(index).map(|(_, id)| *id)
                } else {
                    scope
                        .closure()
                        .cells
                        .get_index(index - scope.names().len())
                        .map(|(_, id)| *id)
                }
            }
            Members::Names(names, extra) if index == names.len() => extra,
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
                    .execution
                    .frames
                    .get(index)
                    .is_some_and(|frame| frame.scope == root),
                Root::Global(index) => globals[index] == root,
                Root::Surviving => false, // Rechecked against the surviving operands at release.
            })
            && self.edges.iter().all(|&(parent, slot, child)| {
                has_strong_edge(heap.get(parent), child, slot, self.dict_keys)
            })
    }

    /// Prepare parent/child positions after discovery has sorted the path nodes.
    fn index_edges(&mut self) -> Option<()> {
        self.edge_indices.clear();
        self.edge_indices.try_reserve_exact(self.edges.len()).ok()?;
        for &(parent, _, child) in &self.edges {
            self.edge_indices.push((
                self.nodes.binary_search(&parent).ok()?,
                self.nodes.binary_search(&child).ok()?,
            ));
        }
        self.live
            .get_mut()
            .try_reserve_exact(self.nodes.len())
            .ok()?;
        self.live.get_mut().resize(self.nodes.len(), false);
        Some(())
    }

    /// A resumed generator can invalidate one path without invalidating other rooted owners.
    /// Validate each edge only after its parent has been proven live.
    fn live_nodes(&self, heap: &Heap, globals: [Id; 2]) -> Option<Ref<'_, Vec<bool>>> {
        if self.nodes.is_empty() || self.edge_indices.len() != self.edges.len() {
            return None;
        }
        let mut live = self.live.borrow_mut();
        if live.len() != self.nodes.len() {
            return None;
        }
        live.fill(false);
        for &(origin, root) in &self.anchors {
            let valid = match origin {
                Root::Frame(index) => heap
                    .execution
                    .frames
                    .get(index)
                    .is_some_and(|frame| frame.scope == root),
                Root::Global(index) => globals[index] == root,
                Root::Surviving => false,
            };
            if valid {
                live[self.nodes.binary_search(&root).ok()?] = true;
            }
        }
        for (&(parent, slot, child), &(parent_index, child_index)) in
            self.edges.iter().zip(&self.edge_indices)
        {
            if live[parent_index]
                && !live[child_index]
                && has_strong_edge(heap.get(parent), child, slot, self.dict_keys)
            {
                live[child_index] = true;
            }
        }
        drop(live);
        Some(self.live.borrow())
    }

    /// Keep validated branches leading to current endpoints, dropping obsolete paths.
    fn retain_live(
        &self,
        heap: &Heap,
        globals: [Id; 2],
        endpoints: impl Iterator<Item = Id>,
    ) -> Option<Self> {
        let live = self.live_nodes(heap, globals)?;
        let mut needed = Vec::new();
        needed.try_reserve_exact(self.nodes.len()).ok()?;
        needed.resize(self.nodes.len(), false);
        for id in endpoints {
            if let Ok(i) = self.nodes.binary_search(&id) {
                needed[i] = live[i];
            }
        }
        let mut linked = Vec::new();
        linked.try_reserve_exact(self.edges.len()).ok()?;
        for (&(parent, slot, child), &(p, c)) in self.edges.iter().zip(&self.edge_indices) {
            linked.push(
                live[p]
                    && live[c]
                    && has_strong_edge(heap.get(parent), child, slot, self.dict_keys),
            );
        }
        for (&(p, c), &valid) in self.edge_indices.iter().zip(&linked).rev() {
            if valid && needed[c] {
                needed[p] = true;
            }
        }
        let selected = |id: Id| self.nodes.binary_search(&id).is_ok_and(|i| needed[i]);
        let mut paths = Self {
            dict_keys: self.dict_keys,
            ..Self::default()
        };
        paths.nodes.try_reserve(self.nodes.len()).ok()?;
        paths
            .nodes
            .extend(self.nodes.iter().copied().filter(|&id| selected(id)));
        paths.anchors.try_reserve(self.anchors.len()).ok()?;
        paths
            .anchors
            .extend(self.anchors.iter().copied().filter(|&(origin, id)| {
                selected(id)
                    && match origin {
                        Root::Frame(index) => heap
                            .execution
                            .frames
                            .get(index)
                            .is_some_and(|frame| frame.scope == id),
                        Root::Global(index) => globals[index] == id,
                        Root::Surviving => false,
                    }
            }));
        paths.edges.try_reserve(self.edges.len()).ok()?;
        paths
            .edges
            .extend(self.edges.iter().zip(linked).filter_map(|(&edge, valid)| {
                (valid && selected(edge.0) && selected(edge.2)).then_some(edge)
            }));
        Some(paths)
    }

    fn discover(
        heap: &Heap,
        globals: [Id; 2],
        endpoints: impl Iterator<Item = Id>,
        full_frame_search: bool,
        dict_keys: bool,
    ) -> Option<Self> {
        Self::extend(
            Self {
                dict_keys,
                ..Self::default()
            },
            heap,
            globals,
            endpoints,
            full_frame_search,
        )
    }

    /// Extend a still-rooted proof without searching again for its covered endpoints.
    fn extend(
        mut paths: Self,
        heap: &Heap,
        globals: [Id; 2],
        endpoints: impl Iterator<Item = Id>,
        full_frame_search: bool,
    ) -> Option<Self> {
        let mut edges_left = MAX_EDGES.saturating_sub(paths.edges.len());
        let mut nodes_left = heap
            .slots
            .len()
            .min(MAX_NODES)
            .saturating_sub(paths.nodes.len());
        let mut wanted = IdSet::with_hasher(hashbrown::DefaultHashBuilder::default());
        for id in endpoints {
            edges_left = edges_left.checked_sub(1)?;
            wanted.try_reserve(1).ok()?;
            wanted.insert(id);
        }
        let global_roots = globals
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, Root::Global(i)));
        if !wanted.is_empty() && (full_frame_search || !heap.execution.frames.is_empty()) {
            // Small weak-reference cohorts may have frame-held targets without a finalizer.
            // Reserve the rest of the search for paths through global modules.
            let reserved_nodes = if full_frame_search {
                0
            } else {
                nodes_left.saturating_sub(FRAME_PROBE_NODES)
            };
            let reserved_edges = if full_frame_search {
                0
            } else {
                edges_left.saturating_sub(FRAME_PROBE_EDGES)
            };
            nodes_left -= reserved_nodes;
            edges_left -= reserved_edges;
            let roots = heap
                .execution
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
                Search::SkipModules,
            )?;
            nodes_left += reserved_nodes;
            edges_left += reserved_edges;
        }
        if !wanted.is_empty() && nodes_left != 0 && edges_left != 0 {
            paths.search(
                heap,
                global_roots,
                &mut wanted,
                &mut nodes_left,
                &mut edges_left,
                Search::Indexed,
            )?;
        }
        paths.covers_endpoints = wanted.is_empty();
        Some(paths)
    }

    /// The insertion-ordered tree is also the BFS queue. Propagate required endpoints to
    /// their ancestors in one reverse pass, then emit the surviving edges parent-first.
    fn search(
        &mut self,
        heap: &Heap,
        roots: impl Iterator<Item = (Id, Root)>,
        wanted: &mut IdSet,
        nodes_left: &mut usize,
        edges_left: &mut usize,
        search: Search,
    ) -> Option<()> {
        if search == Search::Strong {
            return self.search_strong(heap, roots, wanted, nodes_left, edges_left);
        }
        let mut tree = IndexMap::<Id, (Origin, bool), hashbrown::DefaultHashBuilder>::with_hasher(
            hashbrown::DefaultHashBuilder::default(),
        );
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
            if search == Search::SkipModules && matches!(value, Value::Module { .. }) {
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
                // A registry key can prove an endpoint without traversing unrelated keys.
                if heap.slot_shapes[child as usize].is_scalar_leaf() || links.is_dict_key(slot) {
                    if *nodes_left == 0 {
                        break 'search;
                    }
                    if !wanted.remove(&child) {
                        continue;
                    }
                    if tree.len() == tree.capacity() {
                        tree.try_reserve(1).ok()?;
                    }
                    *nodes_left -= 1;
                    tree.insert(
                        child,
                        (
                            Origin::Edge {
                                parent: index,
                                slot: Some(slot),
                            },
                            true,
                        ),
                    );
                } else {
                    // Reserve before borrowing the vacant entry so insertion remains fallible.
                    if tree.len() == tree.capacity() {
                        tree.try_reserve(1).ok()?;
                    }
                    let Entry::Vacant(entry) = tree.entry(child) else {
                        continue;
                    };
                    if *nodes_left == 0 {
                        break 'search;
                    }
                    *nodes_left -= 1;
                    entry.insert((
                        Origin::Edge {
                            parent: index,
                            slot: Some(slot),
                        },
                        wanted.remove(&child),
                    ));
                }
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
                    let slot = slot.or_else(|| {
                        let links = Links::from(heap.get(parent), self.dict_keys);
                        (0..links.len().min(MAX_EDGES)).find(|&i| links.get(i) == Some(id))
                    });
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

    /// Heap IDs are compact slot indices. A dense index avoids hashing every edge
    /// in the unbounded strong search while keeping the BFS and proof order intact.
    fn search_strong(
        &mut self,
        heap: &Heap,
        roots: impl Iterator<Item = (Id, Root)>,
        wanted: &mut IdSet,
        nodes_left: &mut usize,
        edges_left: &mut usize,
    ) -> Option<()> {
        if heap.slots.len() >= u32::MAX as usize {
            return None;
        }
        let mut positions = Vec::new();
        positions.try_reserve_exact(heap.slots.len()).ok()?;
        positions.resize(heap.slots.len(), 0u32);
        for &id in wanted.iter() {
            *positions.get_mut(id as usize)? = u32::MAX;
        }
        let mut tree: Vec<(Id, Origin, bool)> = Vec::new();
        tree.try_reserve((*nodes_left).min(4096)).ok()?;
        for (id, origin) in roots {
            if *edges_left == 0 || *nodes_left == 0 {
                break;
            }
            *edges_left -= 1;
            let position = positions.get_mut(id as usize)?;
            if *position == 0 || *position == u32::MAX {
                let found = *position == u32::MAX;
                tree.try_reserve(1).ok()?;
                *position = (tree.len() + 1) as u32;
                *nodes_left -= 1;
                if found {
                    wanted.remove(&id);
                }
                tree.push((id, Origin::Root(origin), found));
            }
        }
        let mut front = 0;
        while !wanted.is_empty() && *edges_left != 0 && *nodes_left != 0 && front < tree.len() {
            let index = front;
            front += 1;
            let parent = tree[index].0;
            let mut allocated = true;
            // A missed edge only reduces coverage; retained edges are validated again.
            let _ = heap.get(parent).try_children(&mut |child: Id| {
                if !allocated || *edges_left == 0 || *nodes_left == 0 {
                    return;
                }
                *edges_left -= 1;
                if heap.slot_shapes[child as usize].is_scalar_leaf() {
                    return;
                }
                let position = &mut positions[child as usize];
                if *position == 0 || *position == u32::MAX {
                    let found = *position == u32::MAX;
                    if tree.try_reserve(1).is_err() {
                        allocated = false;
                        return;
                    }
                    *position = (tree.len() + 1) as u32;
                    *nodes_left -= 1;
                    if found {
                        wanted.remove(&child);
                    }
                    tree.push((
                        child,
                        Origin::Edge {
                            parent: index,
                            slot: None,
                        },
                        found,
                    ));
                }
            });
            if !allocated {
                return None;
            }
        }
        for index in (0..tree.len()).rev() {
            if let (_, Origin::Edge { parent, .. }, true) = tree[index] {
                tree[parent].2 = true;
            }
        }
        let known = self.nodes.len();
        for &(id, origin, keep) in &tree {
            if !keep || self.nodes[..known].binary_search(&id).is_ok() {
                continue;
            }
            self.nodes.try_reserve(1).ok()?;
            self.nodes.push(id);
            match origin {
                Origin::Root(root) => {
                    self.anchors.try_reserve(1).ok()?;
                    self.anchors.push((root, id));
                }
                Origin::Edge { parent, .. } => {
                    let parent = tree[parent].0;
                    let links = Links::from(heap.get(parent), self.dict_keys);
                    let slot = (0..links.len().min(MAX_EDGES)).find(|&i| links.get(i) == Some(id));
                    self.edges.try_reserve(1).ok()?;
                    self.edges.push((parent, slot, id));
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
            && (self.weak_roots.covers_endpoints
                || self.weak.iter().all(|(target, refs)| {
                    self.weak_roots.contains(*target)
                        && refs.iter().all(|id| self.weak_roots.contains(*id))
                }))
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
        // A complete proof went stale. Look for the new path before backing off again.
        let refresh = std::mem::take(&mut self.weak_roots.covers_endpoints);
        if !refresh && !self.weak_roots.search_due() {
            return;
        }
        let dict_keys = self.weak_roots.retry_with_keys || (refresh && self.weak_roots.dict_keys);
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
            // New targets can become reachable once their constructors return. After
            // complete coverage, retry early and space out successive partial searches.
            let weak_backoff = if paths.covers_endpoints {
                Some(0)
            } else {
                self.weak_roots
                    .weak_backoff
                    .map(|delay| (2 * delay + 1).min(SEARCH_BACKOFF))
            };
            self.weak_roots = RootPaths {
                retry: weak_backoff.unwrap_or(SEARCH_BACKOFF),
                weak_backoff,
                retry_with_keys,
                ..paths
            };
        }
    }

    pub(in crate::dynamic) fn learn_finalizer_paths(
        &mut self,
        globals: [Id; 2],
        streams: Option<&crate::dynamic::stdlib::io::StandardStreamRoots>,
    ) -> bool {
        let mut paths = std::mem::take(&mut self.finalizer_roots);
        if !paths.nodes.is_empty() && !paths.valid(self, globals) {
            // One resumed frame can break a single branch while the rest of the
            // cohort remains rooted. Preserve those independently verified paths.
            paths = paths
                .retain_live(
                    self,
                    globals,
                    self.ordinary_finalizers
                        .iter()
                        .chain(&self.descriptor_owners)
                        .copied(),
                )
                .unwrap_or_default();
        }
        if !paths.search_due() {
            self.finalizer_roots = paths;
            return false;
        }
        let mut endpoints = Vec::new();
        for id in self
            .ordinary_finalizers
            .iter()
            .chain(&self.descriptor_owners)
            .copied()
        {
            if !streams.is_some_and(|roots| roots.contains(&id))
                && !paths.contains(id)
                && !self.operand_keeps_finalizer(id, 0)
            {
                if endpoints.try_reserve(1).is_err() {
                    self.finalizer_roots = paths;
                    return false;
                }
                endpoints.push(id);
            }
        }
        let Some(mut paths) = RootPaths::extend(paths, self, globals, endpoints.into_iter(), true)
        else {
            return false;
        };
        // Failure leaves the ordinary all-or-nothing proof available.
        let _ = paths.index_edges();
        self.finalizer_roots = RootPaths {
            retry: SEARCH_BACKOFF,
            ..paths
        };
        self.learn_weak_roots(globals[0], globals[1]);
        !self.finalizer_roots.nodes.is_empty()
    }

    /// Check the previously observed operand against the current frame and the edges it holds now.
    fn cached_operand_keeps_finalizer(&self, owner: Id, retained_current: usize) -> bool {
        let cached = &self.finalizer_operands[owner as usize % self.finalizer_operands.len()];
        let Some((last_owner, frame, operand)) = cached.get() else {
            return false;
        };
        last_owner == owner
            && (frame + 1 < self.execution.frames.len() || operand < retained_current)
            && self
                .execution
                .frames
                .get(frame)
                .and_then(|f| f.live.get(operand))
                .is_some_and(|&root| self.short_path_keeps_owner(root, owner))
    }

    pub(super) fn operand_keeps_finalizer(&self, owner: Id, retained_current: usize) -> bool {
        self.cached_operand_keeps_finalizer(owner, retained_current)
            || self.find_finalizer_operand(owner, retained_current)
    }

    /// Search published caller operands and the surviving prefix of the active frame.
    fn find_finalizer_operand(&self, owner: Id, retained_current: usize) -> bool {
        let Some(active) = self.execution.frames.len().checked_sub(1) else {
            return false;
        };
        const MAX_CALLER_OPERANDS: usize = 192;
        let active_limit = self.execution.frames[active]
            .live
            .len()
            .min(retained_current)
            .min(8);
        let mut operands = self
            .execution
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
                    || matches!(self.get(root), Value::LoopIterator { source }
                if self.short_path_keeps_owner(*source, owner))
            })
            .or_else(|| operands.find(|&(_, _, root)| self.short_path_keeps_owner(root, owner)));
        if let Some((frame, slot, _)) = found {
            self.finalizer_operands[owner as usize % self.finalizer_operands.len()]
                .set(Some((owner, frame, slot)));
            return true;
        }
        false
    }

    /// Learn a newly opened descriptor's path even when all ordinary finalizers are already rooted.
    pub(in crate::dynamic) fn learn_descriptor_paths(
        &mut self,
        globals: [Id; 2],
        streams: Option<&crate::dynamic::stdlib::io::StandardStreamRoots>,
    ) {
        if self.descriptor_owners.is_empty() {
            return;
        }
        let live = self.finalizer_roots.live_nodes(self, globals);
        let paths_valid = live.is_none() && self.finalizer_roots.valid(self, globals);
        let retained = self.temporary_mark();
        let missing = self.descriptor_owners.iter().any(|&owner| {
            let path_keeps_owner = live.as_ref().is_some_and(|nodes| {
                self.finalizer_roots
                    .nodes
                    .binary_search(&owner)
                    .is_ok_and(|i| nodes[i])
            }) || (paths_valid && self.finalizer_roots.contains(owner));
            !self.root_keeps_owner(owner, path_keeps_owner, streams, retained)
        });
        drop(live);
        if missing {
            self.learn_finalizer_paths(globals, streams);
        }
    }

    /// Recheck the cached path or a surviving frame binding for a registered owner.
    fn root_keeps_owner(
        &self,
        owner: Id,
        path_keeps_owner: bool,
        streams: Option<&crate::dynamic::stdlib::io::StandardStreamRoots>,
        retained_current: usize,
    ) -> bool {
        path_keeps_owner
            || streams.is_some_and(|roots| roots.contains(&owner))
            || self.cached_operand_keeps_finalizer(owner, retained_current)
            || self.bound_in_active_frame(owner)
            || self.find_finalizer_operand(owner, retained_current)
    }

    /// A pending discard preserves the active frame's operands below its mark. It must also
    /// preserve descriptor owners and weak references before a release can skip its sweep.
    pub(in crate::dynamic) fn cached_finalizers_kept(
        &self,
        globals: [Id; 2],
        streams: Option<&crate::dynamic::stdlib::io::StandardStreamRoots>,
        discard_from: Option<usize>,
    ) -> bool {
        self.cached_finalizers_kept_except(globals, streams, discard_from, &[])
    }

    /// Check all pending owners except those covered by a separate reachability proof.
    pub(in crate::dynamic) fn cached_finalizers_kept_except(
        &self,
        globals: [Id; 2],
        streams: Option<&crate::dynamic::stdlib::io::StandardStreamRoots>,
        discard_from: Option<usize>,
        except: &[Id],
    ) -> bool {
        let live = self.finalizer_roots.live_nodes(self, globals);
        let paths_valid = live.is_none() && self.finalizer_roots.valid(self, globals);
        let kept = |owner: &Id| {
            except.contains(owner)
                || self.root_keeps_owner(
                    *owner,
                    live.as_ref().is_some_and(|nodes| {
                        self.finalizer_roots
                            .nodes
                            .binary_search(owner)
                            .is_ok_and(|i| nodes[i])
                    }) || (paths_valid && self.finalizer_roots.contains(*owner)),
                    streams,
                    discard_from.unwrap_or(usize::MAX),
                )
        };
        // Truncating the operand stack cannot break an edge within a heap cycle.
        // Descriptor and weak-reference checks below still require their original proofs.
        let ordinary_kept = if self.cyclic_finalizers.owners.is_empty() {
            self.ordinary_finalizers.iter().all(&kept)
        } else {
            let mut cycle_valid = None;
            self.ordinary_finalizers.iter().all(|owner| {
                (self.cyclic_finalizers.owners.binary_search(owner).is_ok()
                    && *cycle_valid.get_or_insert_with(|| {
                        !self.cyclic_finalizers.verified_owners(self).is_empty()
                    }))
                    || kept(owner)
            })
        };
        ordinary_kept
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

        heap.release_temporaries_since(0);
        heap.collect([modules].into_iter());
        assert_eq!(heap.weak_references(target), &[reference]);
        assert!(heap.cached_weak_roots([modules, NONE]));
    }
}
