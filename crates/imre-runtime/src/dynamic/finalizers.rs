use super::{Context, Error, Id, Result, Value};

impl Context {
    /// Release the interpreter's reference to the script before collecting its globals.
    /// Functions and classes can keep the namespace in a cycle. The normal collector
    /// clears weak references and calls the finalizers while that namespace is intact.
    pub(super) fn release_main_at_shutdown(&mut self) -> Result<()> {
        // A colliding key in sys.modules can run Python during a dictionary lookup.
        if let Some(index) = self.stored_entries(self.modules).iter().position(|&(key, module)| {
            matches!(self.heap.get(key), Value::Str(name) if name == "__main__")
                && matches!(self.heap.get(module), Value::Module { name, .. } if name == "__main__")
        }) {
            super::builtins::remove_table_entry(self, self.modules, index)?;
        }
        self.collect_generation(2)?;
        Ok(())
    }

    fn is_shutdown_resource(&self, owner: Id) -> bool {
        let Value::Instance { class, .. } = self.heap.get(owner) else {
            return false;
        };
        self.find_in_mro(*class, |base| {
            let Value::Class { scope, .. } = self.heap.get(base) else {
                return None;
            };
            self.scope_data(*scope)
                .class_namespace
                .as_ref()?
                .finalize_at_shutdown
                .then_some(())
        })
        .is_some()
    }

    /// Finish the current owners before dispatching closers created by their callbacks.
    pub(super) fn finalize_shutdown_resources(&mut self) -> Result<()> {
        let mut owners = Vec::new();
        loop {
            for &owner in self.heap.pending_finalizers() {
                if self.is_shutdown_resource(owner) {
                    owners
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    owners.push(owner);
                }
            }
            if owners.is_empty() {
                return Ok(());
            }
            // Running Python can collect or finalize another owner in this group.
            let mark = self.heap.temporary_mark();
            self.heap.reserve_temporaries(owners.len())?;
            for &owner in &owners {
                self.heap.keep(owner);
            }
            for owner in owners.drain(..).rev() {
                if self.heap.is_pending_finalizer(owner) {
                    self.dispatch_finalizer(owner);
                }
            }
            self.heap.release_temporaries_since(mark);
        }
    }

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
        let mut siblings = Vec::new();
        while self.heap.has_ordinary_finalizers() {
            let stdin = super::stdlib::io::rooted_input_chain(self);
            let (heap, roots) = self.heap_and_roots();
            let Some(owner) = heap.acyclic_finalizer(roots, stdin.as_ref(), &mut siblings)? else {
                break;
            };
            // Independent owners released by one boundary are finalized from that caller.
            // A nested checkpoint inside __del__ may still finalize owners it actually drops.
            let mark = self.heap.temporary_mark();
            self.heap.reserve_temporaries(siblings.len())?;
            for sibling in siblings.drain(..) {
                self.heap.keep(sibling);
            }
            let result = super::stdlib::io::deallocate_exact_pair(self, owner);
            if matches!(result, Ok(false)) {
                self.dispatch_finalizer(owner);
            }
            self.heap.release_temporaries_since(mark);
            result?;
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
