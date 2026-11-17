use super::{Heap, Id, IdSet, Links, Root, RootPaths, Search, Value, has_strong_edge};

/// One previously learned tree of strong paths, revalidated against the current heap.
#[derive(Default)]
struct RetainedProof {
    paths: RootPaths,
    live: Vec<bool>,
    edge_indices: Vec<(u32, u32)>,
}

impl RetainedProof {
    fn refresh(&mut self, heap: &Heap, anchors: &[Id]) {
        self.live.fill(false);
        for id in anchors {
            if let Ok(i) = self.paths.nodes.binary_search(id) {
                self.live[i] = true;
            }
        }
        // A resumed caller may hold an interior node directly. Visit edges in parent order.
        let mut linked_parent = None;
        let mut links = None;
        for (&(parent, slot, child), &(parent_index, child_index)) in
            self.paths.edges.iter().zip(&self.edge_indices)
        {
            let i = child_index as usize;
            if !self.live[i] && self.live[parent_index as usize] {
                self.live[i] = if let Some(slot) = slot {
                    if linked_parent != Some(parent) {
                        linked_parent = Some(parent);
                        links = Some(Links::from(heap.get(parent), true));
                    }
                    links.as_ref().unwrap().get(slot) == Some(child)
                } else {
                    has_strong_edge(heap.get(parent), child, None, true)
                };
            }
        }
    }

    fn contains_live(&self, id: Id) -> bool {
        self.paths
            .nodes
            .binary_search(&id)
            .is_ok_and(|i| self.live[i])
    }

    fn learn(
        &mut self,
        heap: &Heap,
        anchors: &[Id],
        previous: Option<&RetainedProof>,
        wanted: &mut IdSet,
    ) -> Option<()> {
        let mut paths = RootPaths {
            dict_keys: true,
            ..RootPaths::default()
        };
        if let Some(previous) = previous {
            paths
                .nodes
                .try_reserve_exact(previous.paths.nodes.len())
                .ok()?;
            paths.nodes.extend(
                previous
                    .paths
                    .nodes
                    .iter()
                    .zip(&previous.live)
                    .filter_map(|(&id, &live)| live.then_some(id)),
            );
            paths
                .edges
                .try_reserve_exact(previous.paths.edges.len())
                .ok()?;
            paths.edges.extend(
                previous
                    .paths
                    .edges
                    .iter()
                    .zip(&previous.edge_indices)
                    .filter_map(|(&edge, &(parent, child))| {
                        (previous.live[parent as usize] && previous.live[child as usize])
                            .then_some(edge)
                    }),
            );
            paths
                .anchors
                .try_reserve_exact(previous.paths.anchors.len())
                .ok()?;
            paths.anchors.extend(
                previous
                    .paths
                    .anchors
                    .iter()
                    .copied()
                    .filter(|&(_, id)| previous.contains_live(id)),
            );
        }
        let mut nodes = heap.slots.len();
        let mut edges = usize::MAX;
        paths.search(
            heap,
            previous
                .into_iter()
                .flat_map(|proof| proof.paths.nodes.iter().zip(&proof.live))
                .filter_map(|(&id, &live)| live.then_some((id, Root::Surviving)))
                .chain(anchors.iter().map(|&id| (id, Root::Surviving))),
            wanted,
            &mut nodes,
            &mut edges,
            Search::Strong,
        )?;
        let mut edge_indices = Vec::new();
        edge_indices.try_reserve_exact(paths.edges.len()).ok()?;
        for &(parent, _, child) in &paths.edges {
            let parent = u32::try_from(paths.nodes.binary_search(&parent).ok()?).ok()?;
            let child = u32::try_from(paths.nodes.binary_search(&child).ok()?).ok()?;
            edge_indices.push((parent, child));
        }
        self.live
            .try_reserve(paths.nodes.len().saturating_sub(self.live.len()))
            .ok()?;
        self.live.resize(paths.nodes.len(), false);
        self.paths = paths;
        self.edge_indices = edge_indices;
        Some(())
    }
}

/// Strong paths validated against roots that survive the last release. Coroutine
/// frames can alternate among several root sets, so keep a few independent proofs.
#[derive(Default)]
pub(in crate::dynamic::value) struct RetainedPaths {
    proofs: [RetainedProof; 4],
    anchors: Vec<Id>,
}

impl RetainedPaths {
    fn directly_held(&self, heap: &Heap, id: Id) -> bool {
        heap.bound_in_active_frame(id)
            || heap.operand_keeps_finalizer(id, usize::MAX)
            || self.generator_kept_by_anchors(heap, id)
    }

    fn endpoints_held(&self, heap: &Heap, operand_release: bool, proofs: usize) -> bool {
        let held = |id| {
            self.proofs[0].contains_live(id)
                || self.proofs[1..proofs]
                    .iter()
                    .any(|proof| proof.contains_live(id))
                || self.directly_held(heap, id)
        };
        if operand_release {
            heap.registered_endpoints().all(held)
        } else {
            heap.ordinary_finalizers.iter().copied().all(held)
        }
    }

    fn keeps(&mut self, heap: &Heap, operand_release: bool) -> bool {
        let mut checked = false;
        for index in 0..self.proofs.len() {
            if self.proofs[index].paths.nodes.is_empty() {
                continue;
            }
            checked = true;
            self.proofs[index].refresh(heap, &self.anchors);
            if self.endpoints_held(heap, operand_release, index + 1) {
                return true;
            }
        }
        !checked && self.endpoints_held(heap, operand_release, 1)
    }

    /// A resumed generator can move between surviving operands and local bindings.
    fn generator_kept_by_anchors(&self, heap: &Heap, owner: Id) -> bool {
        matches!(heap.get(owner), Value::Generator { .. })
            && (self
                .anchors
                .iter()
                .rev()
                .any(|&root| heap.short_path_keeps_owner(root, owner))
                || heap.frame_keeps_owner(owner))
    }

    fn learn(&mut self, heap: &Heap) -> Option<()> {
        // Reuse only live paths, and keep their edges before newly discovered edges.
        // A reused search root is therefore live only while the old path still validates.
        const MAX_REUSED_NODES: usize = 4096;
        let previous =
            (self.proofs[0].paths.nodes.len() <= MAX_REUSED_NODES).then_some(&self.proofs[0]);
        let mut wanted = IdSet::with_hasher(hashbrown::DefaultHashBuilder::default());
        for id in heap.registered_endpoints() {
            if !previous.is_some_and(|proof| proof.contains_live(id))
                && !self.directly_held(heap, id)
            {
                wanted.try_reserve(1).ok()?;
                wanted.insert(id);
            }
        }
        let mut next = RetainedProof::default();
        next.learn(heap, &self.anchors, previous, &mut wanted)?;
        next.paths.retry = self.proofs[0].paths.retry;
        self.proofs.rotate_right(1);
        self.proofs[0] = next;
        Some(())
    }
}

impl Heap {
    pub(in crate::dynamic) fn has_finalizer_cohort(&self) -> bool {
        self.ordinary_finalizers.len() > super::super::MAX_FRAME_FINALIZERS
            && !self.full_collection_active()
    }

    fn registered_endpoints(&self) -> impl Iterator<Item = Id> + '_ {
        self.ordinary_finalizers
            .iter()
            .chain(&self.descriptor_owners)
            .copied()
            .chain(
                self.weak.iter().flat_map(|(&target, refs)| {
                    std::iter::once(target).chain(refs.iter().copied())
                }),
            )
    }

    pub(in crate::dynamic) fn retained_paths_keep_owners(
        &mut self,
        roots: impl Iterator<Item = Id>,
        operand_release: bool,
    ) -> bool {
        let mut cache = std::mem::take(&mut self.retained_paths);
        cache.anchors.clear();
        let anchored = roots
            .chain(self.execution_roots())
            .filter(|&id| !self.slot_shapes[id as usize].is_scalar_leaf())
            .try_for_each(|id| {
                cache.anchors.try_reserve(1)?;
                cache.anchors.push(id);
                Ok::<_, std::collections::TryReserveError>(())
            });
        if anchored.is_err() {
            self.retained_paths = cache;
            return false;
        }
        let mut kept = cache.keeps(self, operand_release);
        if !kept && cache.proofs[0].paths.search_due() && cache.learn(self).is_some() {
            kept = cache.keeps(self, operand_release);
            if kept {
                cache.proofs[0].paths.retry = 0;
            }
        }
        self.retained_paths = cache;
        kept
    }
}
