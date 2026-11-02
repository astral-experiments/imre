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
            let stdin = super::stdlib::io::rooted_input_chain(self);
            let (heap, roots) = self.heap_and_roots();
            let Some(owner) = heap.acyclic_finalizer(roots, stdin.as_ref())? else {
                break;
            };
            self.dispatch_finalizer(owner);
            finalized = true;
        }
        Ok(finalized)
    }

    pub(super) fn dispatch_finalizer(&mut self, owner: Id) {
        let scan = match self.heap.get(owner) {
            Value::Scandir(scan) => Some(std::rc::Rc::clone(scan)),
            _ => None,
        };
        let generator = matches!(self.heap.get(owner), Value::Generator { .. });
        let found = scan.is_some() || generator || self.slot(owner, "__del__").is_some();
        self.heap.take_finalizer(owner, found);
        super::stdlib::io::note_writer_finalizer(self, owner);
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
        if let Some(scan) = scan {
            super::stdlib::os::scandir::finalize(self, owner, &scan);
        } else if generator {
            if let Err(error) = self.generator_close(owner) {
                eprintln!("Exception ignored while calling __del__: {error}");
            }
        } else if let Some(slot) = self.slot(owner, "__del__") {
            self.heap.keep(slot);
            // CPython calls method descriptors unbound, including ordinary
            // functions. Only custom descriptors resolve __get__ before the call.
            let unbound = matches!(self.heap.get(slot), Value::Function { .. })
                || matches!(self.heap.get(slot), Value::Native(name)
                    if super::wrapper_descriptor::BuiltinDescriptor::lookup(name).is_some());
            let target = if unbound {
                Some((slot, vec![owner]))
            } else {
                self.bound_slot(owner, "__del__")
                    .ok()
                    .flatten()
                    .map(|method| (method, Vec::new()))
            };
            if let Some((method, args)) = target {
                self.heap.keep(method);
                if let Err(error) = self.call(method, args, Vec::new()) {
                    super::unraisable::deallocator(self, error, method);
                }
            }
        }
        self.pending = pending;
        self.heap.release_temporaries_since(mark);
    }
}
