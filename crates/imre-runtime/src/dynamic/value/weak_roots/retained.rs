use super::{Heap, Id, Root, RootPaths, Search, Value, has_strong_edge};
use std::collections::HashSet;

/// Strong paths validated against the roots that survive the last release.
#[derive(Default)]
pub(in crate::dynamic::value) struct RetainedPaths {
    paths: RootPaths,
    anchors: Vec<Id>,
    live: Vec<bool>,
}

impl RetainedPaths {
    fn keeps(&mut self, heap: &Heap, operand_release: bool) -> bool {
        let paths = &self.paths;
        if paths.nodes.is_empty() {
            return false;
        }
        self.live.fill(false);
        for id in &self.anchors {
            if let Ok(i) = paths.nodes.binary_search(id) {
                self.live[i] = true;
            }
        }
        // A resumed caller may hold an interior node directly. Visit edges in parent order.
        for &(parent, slot, child) in &paths.edges {
            let i = paths.nodes.binary_search(&child).unwrap();
            if !self.live[i] && self.live[paths.nodes.binary_search(&parent).unwrap()] {
                self.live[i] = has_strong_edge(heap.get(parent), child, slot, true);
            }
        }
        let held = |id| {
            paths.nodes.binary_search(&id).is_ok_and(|i| self.live[i])
                || heap.bound_in_active_frame(id)
                || heap.operand_keeps_finalizer(id, usize::MAX)
                || self.generator_kept_by_anchors(heap, id)
        };
        if operand_release {
            heap.registered_endpoints().all(held)
        } else {
            heap.ordinary_finalizers.iter().copied().all(held)
        }
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
        let mut wanted = HashSet::new();
        for id in heap.registered_endpoints() {
            wanted.try_reserve(1).ok()?;
            wanted.insert(id);
        }
        let mut paths = RootPaths {
            dict_keys: true,
            ..RootPaths::default()
        };
        let mut nodes = heap.slots.len();
        let mut edges = usize::MAX;
        paths.search(
            heap,
            self.anchors.iter().map(|&id| (id, Root::Surviving)),
            &mut wanted,
            &mut nodes,
            &mut edges,
            Search::Strong,
        )?;
        self.live
            .try_reserve(paths.nodes.len().saturating_sub(self.live.len()))
            .ok()?;
        self.live.resize(paths.nodes.len(), false);
        paths.retry = self.paths.retry;
        self.paths = paths;
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
        for id in roots
            .chain(self.execution.roots())
            .filter(|&id| !self.slot_shapes[id as usize].is_scalar_leaf())
        {
            if cache.anchors.try_reserve(1).is_err() {
                self.retained_paths = cache;
                return false;
            }
            cache.anchors.push(id);
        }
        let mut kept = cache.keeps(self, operand_release);
        if !kept && cache.paths.search_due() && cache.learn(self).is_some() {
            kept = cache.keeps(self, operand_release);
            if kept {
                cache.paths.retry = 0;
            }
        }
        self.retained_paths = cache;
        kept
    }
}
