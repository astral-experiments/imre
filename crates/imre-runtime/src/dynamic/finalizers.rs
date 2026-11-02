use super::{Context, Id, Result, Value};

impl Context {
    /// A newly assigned class slot also applies to existing instances of subclasses.
    pub(super) fn register_class_finalizers(&mut self) {
        for index in 0..self.heap.slot_count() {
            let id = index as Id;
            if self.heap.ordinary_instance(id) && self.slot(id, "__del__").is_some() {
                self.heap.register_finalizer(id);
            }
        }
    }

    /// Recheck roots after each finalizer. An upstream owner may resurrect a descendant.
    pub(super) fn drain_acyclic_finalizers(&mut self) -> Result<bool> {
        let mut finalized = false;
        while self.heap.has_ordinary_finalizers() {
            let (heap, roots) = self.heap_and_roots();
            let Some(owner) = heap.acyclic_finalizer(roots)? else {
                break;
            };
            self.dispatch_finalizer(owner);
            finalized = true;
        }
        Ok(finalized)
    }

    pub(super) fn dispatch_finalizer(&mut self, owner: Id) {
        if let Value::Scandir(scan) = self.heap.get(owner) {
            let scan = std::rc::Rc::clone(scan);
            self.heap.take_finalizer(owner, true);
            super::stdlib::os::scandir::finalize(self, &scan);
            return;
        }
        let generator = matches!(self.heap.get(owner), Value::Generator { .. });
        let found = generator || self.slot(owner, "__del__").is_some();
        self.heap.take_finalizer(owner, found);
        if !found {
            return;
        }
        // Binding a descriptor can itself run Python. Install the once-only bit and
        // root the receiver and current error before resolving the current class slot.
        let mark = self.heap.temporary_mark();
        self.heap.keep(owner);
        let pending = self.pending.take();
        if let Some(error) = &pending {
            for root in error.roots() {
                self.heap.keep(root);
            }
        }
        if generator {
            if let Err(error) = self.generator_close(owner) {
                eprintln!("Exception ignored while calling __del__: {error}");
            }
        } else if let Some(slot) = self.slot(owner, "__del__") {
            self.heap.keep(slot);
            if let Ok(Some(method)) = self.bound_slot(owner, "__del__") {
                self.heap.keep(method);
                // CPython calls ordinary function slots unbound. Other descriptors
                // report the callable produced by descriptor binding.
                let deallocator = if matches!(self.heap.get(slot), Value::Function { .. }) {
                    slot
                } else {
                    method
                };
                if let Err(error) = self.call(method, Vec::new(), Vec::new()) {
                    super::unraisable::deallocator(self, error, deallocator);
                }
            }
        }
        self.pending = pending;
        self.heap.release_temporaries_since(mark);
    }
}
