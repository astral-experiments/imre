use super::{Heap, Id, Value};
use indexmap::IndexMap;
use std::collections::HashMap;

const MAX_ENDPOINTS: usize = 16;
const MAX_DEPTH: usize = 12;
const MAX_NODES: usize = 8192;
const MAX_EDGES: usize = 65536;

#[derive(Default)]
pub(super) struct WeakRoots {
    roots: [Id; 2],
    endpoints: Vec<Id>,
    edges: Vec<(Id, usize, Id)>,
    retry: u8,
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
}

impl<'a> Links<'a> {
    fn from(value: &'a Value) -> Self {
        let (direct, members) = match value {
            Value::Module { scope, .. } => ([Some(*scope), None], Members::None),
            Value::Scope(scope) => (
                [scope.namespace, scope.exec_globals.map(|g| g.dict)],
                Members::Names(&scope.names),
            ),
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
            }
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
        }
    }
}

impl WeakRoots {
    /// Recheck parents before children so a removed link also protects against reused heap slots.
    pub(super) fn keeps_all(&self, heap: &Heap, roots: [Id; 2]) -> bool {
        self.roots == roots
            && !self.endpoints.is_empty()
            && heap.weak.len() <= MAX_ENDPOINTS
            && heap.weak.iter().all(|(target, refs)| {
                self.endpoints.contains(target)
                    && refs.len() <= MAX_ENDPOINTS
                    && refs.iter().all(|id| self.endpoints.contains(id))
            })
            && self.edges.iter().all(|&(parent, slot, child)| {
                Links::from(heap.get(parent)).get(slot) == Some(child)
            })
    }

    fn discover(heap: &Heap, roots: [Id; 2]) -> Option<Self> {
        let mut endpoints = Vec::new();
        endpoints.try_reserve_exact(MAX_ENDPOINTS).ok()?;
        for (&target, refs) in &heap.weak {
            if refs.len() >= MAX_ENDPOINTS - endpoints.len() {
                return None;
            }
            endpoints.push(target);
            endpoints.extend_from_slice(refs);
        }
        let limit = heap.slots.len().min(MAX_NODES);
        let mut parents = HashMap::new();
        let mut queue = Vec::new();
        parents.try_reserve(limit).ok()?;
        queue.try_reserve_exact(limit).ok()?;
        for root in roots {
            if parents.insert(root, (root, 0)).is_none() {
                queue.push(root);
            }
        }
        let mut front = 0;
        let mut budget = MAX_EDGES;
        while !endpoints.iter().all(|id| parents.contains_key(id)) {
            let parent = *queue.get(front)?;
            front += 1;
            let links = Links::from(heap.get(parent));
            budget = budget.checked_sub(links.len())?;
            for slot in 0..links.len() {
                if let Some(child) = links.get(slot)
                    && (!heap.scalar_leaves[child as usize] || endpoints.contains(&child))
                    && let std::collections::hash_map::Entry::Vacant(entry) = parents.entry(child)
                {
                    if queue.len() >= limit {
                        return None;
                    }
                    entry.insert((parent, slot));
                    queue.push(child);
                }
            }
        }

        let mut edges = Vec::new();
        edges.try_reserve_exact(endpoints.len() * MAX_DEPTH).ok()?;
        for &endpoint in &endpoints {
            let mut child = endpoint;
            let mut path = smallvec::SmallVec::<[_; MAX_DEPTH]>::new();
            while !roots.contains(&child) {
                if path.len() == MAX_DEPTH {
                    return None;
                }
                let &(parent, slot) = parents.get(&child)?;
                path.push((parent, slot, child));
                child = parent;
            }
            for edge in path.into_iter().rev() {
                if !edges.contains(&edge) {
                    edges.push(edge);
                }
            }
        }
        Some(Self {
            roots,
            endpoints,
            edges,
            retry: 0,
        })
    }
}

impl Heap {
    /// Learn bounded ownership paths after a prompt collection, when weak callbacks have settled.
    pub(in crate::dynamic) fn learn_weak_roots(&mut self, modules: Id, reference: Id) {
        if self.weak.is_empty()
            || self.weak.len() > MAX_ENDPOINTS
            || self.weak_values_kept_by(modules, reference)
        {
            return;
        }
        if self.weak_roots.retry > 0 {
            self.weak_roots.retry -= 1;
            return;
        }
        self.weak_roots = WeakRoots::discover(self, [modules, reference]).unwrap_or_default();
        // Limit unsuccessful or repeatedly invalidated searches to one per 64 prompt collections.
        self.weak_roots.retry = 63;
    }
}

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
