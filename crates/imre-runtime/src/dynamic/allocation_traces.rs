//! Allocation-time Python locations shared by the boxed heaps of one interpreter.

use super::value::Id;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type Trace = Arc<[(Box<str>, u32)]>;

#[derive(Default)]
struct HeapTraces {
    objects: HashMap<Id, Trace>,
    // Objects allocated at the same location can share their immutable stack.
    previous: Option<Trace>,
}

struct State {
    frame_limit: usize,
    linked_constants: bool,
    next_heap: usize,
    heaps: HashMap<usize, HeapTraces>,
}

struct Registry {
    active: AtomicBool,
    state: Mutex<State>,
}

pub(super) struct AllocationTraces {
    registry: Arc<Registry>,
    // Object IDs are local to a heap. A worker receives its own namespace.
    heap: usize,
}

impl Default for AllocationTraces {
    fn default() -> Self {
        Self {
            registry: Arc::new(Registry {
                active: AtomicBool::new(false),
                state: Mutex::new(State {
                    frame_limit: 1,
                    linked_constants: false,
                    next_heap: 1,
                    heaps: HashMap::new(),
                }),
            }),
            heap: 0,
        }
    }
}

impl AllocationTraces {
    pub(super) fn worker(&self) -> Self {
        let mut state = self.registry.state.lock().unwrap();
        let heap = state.next_heap;
        state.next_heap += 1;
        Self {
            registry: Arc::clone(&self.registry),
            heap,
        }
    }

    pub(super) fn is_tracing(&self) -> bool {
        self.registry.active.load(Ordering::Acquire)
    }

    pub(super) fn limit(&self) -> usize {
        self.registry.state.lock().unwrap().frame_limit
    }

    pub(super) fn start(&self, limit: usize) {
        self.start_session(limit, false);
    }

    pub(super) fn start_at_startup(&self, limit: usize) {
        self.start_session(limit, true);
    }

    fn start_session(&self, limit: usize, linked_constants: bool) {
        let mut state = self.registry.state.lock().unwrap();
        if !self.is_tracing() {
            state.frame_limit = limit;
            state.linked_constants = linked_constants;
            self.registry.active.store(true, Ordering::Release);
        }
    }

    pub(super) fn stop(&self) {
        let mut state = self.registry.state.lock().unwrap();
        self.registry.active.store(false, Ordering::Release);
        state.heaps.clear();
    }

    pub(super) fn clear(&self) {
        let mut state = self.registry.state.lock().unwrap();
        state.linked_constants = false;
        state.heaps.clear();
    }

    pub(super) fn get(&self, id: Id) -> Option<Trace> {
        let state = self.registry.state.lock().unwrap();
        state.heaps.get(&self.heap)?.objects.get(&id).cloned()
    }

    pub(super) fn record_linked(&self, id: Id) {
        // Linked code was compiled before user code ran. Its constants have no Python caller.
        self.record_frames(id, true, std::iter::empty());
    }

    pub(super) fn record<'a>(&self, id: Id, frames: impl Iterator<Item = (&'a str, u32)> + Clone) {
        self.record_frames(id, false, frames);
    }

    fn record_frames<'a>(
        &self,
        id: Id,
        linked: bool,
        frames: impl Iterator<Item = (&'a str, u32)> + Clone,
    ) {
        if !self.is_tracing() {
            return;
        }
        let mut state = self.registry.state.lock().unwrap();
        if !self.is_tracing() || (linked && !state.linked_constants) {
            return;
        }
        let frames = frames.take(state.frame_limit);
        let empty = frames.clone().next().is_none().then_some(("<unknown>", 0));
        let frames = frames.chain(empty);
        let heap = state.heaps.entry(self.heap).or_default();
        let same = heap.previous.as_ref().is_some_and(|previous| {
            let mut current = frames.clone();
            previous.iter().all(|(file, line)| {
                current
                    .next()
                    .is_some_and(|(name, n)| name == file.as_ref() && n == *line)
            }) && current.next().is_none()
        });
        if !same {
            let captured: Vec<_> = frames.map(|(file, line)| (Box::from(file), line)).collect();
            heap.previous = Some(captured.into());
        }
        heap.objects
            .insert(id, Arc::clone(heap.previous.as_ref().unwrap()));
    }

    pub(super) fn retain_live(&self, live: impl Fn(Id) -> bool) {
        if self.is_tracing()
            && let Some(heap) = self
                .registry
                .state
                .lock()
                .unwrap()
                .heaps
                .get_mut(&self.heap)
        {
            heap.objects.retain(|id, _| live(*id));
        }
    }
}

impl Drop for AllocationTraces {
    fn drop(&mut self) {
        self.registry.state.lock().unwrap().heaps.remove(&self.heap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_constants_belong_to_the_startup_session() {
        let traces = AllocationTraces::default();
        traces.start_at_startup(2);
        traces.record_linked(4);
        assert_eq!(traces.get(4).unwrap()[0], ("<unknown>".into(), 0));
        traces.clear();
        traces.record_linked(5);
        assert!(traces.get(5).is_none());
        traces.stop();
        traces.start(2);
        traces.record_linked(6);
        assert!(traces.get(6).is_none());
    }

    #[test]
    fn workers_share_controls_and_keep_object_handles_separate() {
        let main = AllocationTraces::default();
        let worker = main.worker();
        assert_eq!(main.limit(), 1);
        main.start(2);
        worker.start(1);
        let frames = [("file.py", 3), ("file.py", 7), ("outside.py", 1)];
        main.record(9, frames.into_iter());
        assert!(worker.get(9).is_none());
        assert_eq!(main.get(9).unwrap().len(), 2);
        worker.record(9, [("worker.py", 8)].into_iter());
        assert_eq!(main.get(9).unwrap()[0].1, 3);
        worker.clear();
        assert!(main.get(9).is_none() && worker.get(9).is_none());
        main.record(9, frames.into_iter());
        worker.stop();
        assert!(!main.is_tracing() && main.get(9).is_none());
        assert_eq!(worker.limit(), 2);
        worker.start(1);
        assert!(main.get(9).is_none());
        main.record(9, frames.into_iter());
        assert_eq!(main.get(9).unwrap().len(), 1);
    }
}
