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

    /// Register an instance if its Python class currently defines a destructor.
    pub(super) fn register_instance_finalizer(&mut self, id: Id, class: Option<Id>) {
        if let Some(class) = class
            && self.class_attr(class, "__del__").is_some()
        {
            self.heap.register_finalizer(id);
        }
    }

    /// A newly assigned class slot also applies to existing instances of subclasses.
    pub(super) fn register_class_finalizers(&mut self) {
        for index in 0..self.heap.slot_count() {
            let id = index as Id;
            self.register_instance_finalizer(id, self.heap.finalizer_class(id));
        }
    }

    /// Recheck roots after each finalizer. An upstream owner may resurrect a descendant.
    pub(super) fn drain_acyclic_finalizers(&mut self) -> Result<bool> {
        let mut finalized = false;
        while self.heap.has_ordinary_finalizers() {
            let streams = super::stdlib::io::rooted_standard_streams(self);
            let globals = self.finalizer_path_roots();
            let (heap, roots) = self.heap_and_roots();
            let Some(owner) = heap.acyclic_finalizer(globals, roots, streams.as_ref())? else {
                break;
            };
            if !super::stdlib::io::deallocate_exact_pair(self, owner)? {
                self.dispatch_finalizer(owner);
            }
            finalized = true;
        }
        Ok(finalized)
    }

    fn finalize_unstarted_coroutine(&mut self, owner: Id) -> Result<()> {
        // Cyclic GC already cleared the isolate's weakrefs. On ordinary deallocation,
        // old weakrefs must expire even when a warning recorder resurrects its source.
        if !self.heap.in_full_collection(owner) {
            let (heap, roots) = self.heap_and_roots();
            let callbacks = heap.clear_dealloc_weakrefs(owner, roots)?;
            self.run_weak_callbacks(callbacks);
        }
        self.warn_unawaited_coroutine(owner)
    }

    pub(super) fn dispatch_finalizer(&mut self, owner: Id) {
        let scan = match self.heap.get(owner) {
            Value::Scandir(scan) => Some(std::rc::Rc::clone(scan)),
            _ => None,
        };
        let async_generator = matches!(self.heap.get(owner), Value::AsyncGenerator { .. });
        let generator_frame = match *self.heap.get(owner) {
            Value::Generator { .. } => Some(owner),
            Value::Coroutine { frame, .. } => Some(frame),
            _ => None,
        };
        let unpack_iterator = matches!(
            self.heap.get(owner),
            Value::StructUnpackIterator {
                exporter: Some(_),
                ..
            }
        );
        let exact_socket = matches!(self.heap.get(owner), Value::Socket(_));
        let found = exact_socket
            || scan.is_some()
            || generator_frame.is_some()
            || async_generator
            || unpack_iterator
            || self.slot(owner, "__del__").is_some();
        self.heap.take_finalizer(owner, found);
        super::stdlib::io::note_writer_finalizer(self, owner);
        if !found {
            return;
        }
        // The selected slot owns the implicit close. An override can resurrect the
        // socket or release its fd through os.close without calling the base slot.
        if let Some(socket) = self.heap.get(owner).socket_handle() {
            crate::objects::socket::hand_finalization_to_python(socket);
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
        } else if unpack_iterator {
            super::stdlib::structure::release_iterator(self, owner);
        } else if async_generator {
            if let Err(error) = self.finalize_async_generator(owner) {
                eprintln!(
                    "Exception ignored while finalizing asynchronous generator: {}",
                    error.report(&self.heap)
                );
            }
        } else if let Some(frame) = generator_frame {
            if self.heap.is_unstarted_coroutine(owner) {
                // A warning recorder can keep its source and subsequently await it.
                if let Err(error) = self.finalize_unstarted_coroutine(owner) {
                    super::unraisable::report(
                        self,
                        error,
                        true,
                        Some(owner),
                        "Exception ignored while finalizing coroutine",
                    );
                }
            } else if let Err(error) = self.close_generator_frame(frame, owner) {
                eprintln!(
                    "Exception ignored while calling __del__: {}",
                    error.report(&self.heap)
                );
            }
        } else if let Some(slot) = if exact_socket {
            Some(self.builtin("socket.socket.__del__"))
        } else {
            self.slot(owner, "__del__")
        } {
            self.heap.keep(slot);
            // CPython calls method descriptors unbound, including ordinary
            // functions. Only custom descriptors resolve __get__ before the call.
            let unbound = exact_socket
                || matches!(self.heap.get(slot), Value::Function { .. })
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
