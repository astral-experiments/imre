//! Replacing the direct bases of a completed Python class.

use super::{Context, Error, HashMap, Id, Result, Value};

impl Context {
    /// Only slot-free Python classes over object or dict share the layouts modeled here.
    fn supports_rebase(&self, class: Id) -> bool {
        self.find_in_mro(class, |owner| {
            let supported = match self.heap.get(owner) {
                Value::Native(name) => matches!(name.as_str(), "object" | "dict"),
                Value::Class {
                    scope,
                    metaclass,
                    namedtuple_fields: None,
                    io_kind: None,
                    heap_type: None,
                    ..
                } if metaclass.is_none() || *metaclass == self.builtins.get("type").copied() => {
                    let frame = self.scope_data(*scope);
                    frame.class_cell.is_some()
                        && frame.class_namespace.as_ref().is_some_and(|ns| {
                            !ns.immutable_type
                                && ns.slot_names.is_none()
                                && ns.itertools_layout.is_none()
                        })
                }
                _ => false,
            };
            (!supported).then_some(())
        })
        .is_none()
    }

    pub(super) fn assign_class_bases(&mut self, class: Id, value: Id) -> Result<()> {
        if !self.audit_hooks.is_empty() {
            let name = self.text("__bases__");
            super::stdlib::audit::event(self, "object.__setattr__", &[class, name, value])?;
        }
        let storage = self.scalar_value(value);
        let Value::Tuple(new_bases) = self.heap.get(storage) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "can only assign tuple to {}.__bases__, not {}",
                    self.class_name(class),
                    self.type_name(value)
                ),
            ));
        };
        if new_bases.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "can only assign non-empty tuple to {}.__bases__, not ()",
                    self.class_name(class)
                ),
            ));
        }
        for &base in new_bases {
            if !self.is_type_object(base) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{}.__bases__ must be tuple of classes, not '{}'",
                        self.class_name(class),
                        self.type_name(base)
                    ),
                ));
            }
            if self
                .find_in_mro(base, |owner| (owner == class).then_some(()))
                .is_some()
            {
                return Err(Error::new(
                    "TypeError",
                    "a __bases__ item causes an inheritance cycle",
                ));
            }
        }
        let new_bases =
            self.validate_class_bases(new_bases.clone(), super::typing::ClassLayout::Specialized)?;
        let unsupported = || {
            Error::new(
                "NotImplementedError",
                "__bases__ assignment requires ordinary slot-free classes over object or dict",
            )
        };
        let old_bases = self.class_bases_tuple(class).ok_or_else(unsupported)?;
        let old_base = self.member_layout_base(&old_bases)?;
        let new_base = self.member_layout_base(&new_bases)?;
        let layout = |base| {
            (
                matches!(self.heap.get(base), Value::Class { .. }),
                self.allocation_base(base),
                self.class_instances_have_dict(base),
                self.class_instances_support_weakref(base),
                self.has_declared_storage_slots(base),
            )
        };
        if layout(old_base) != layout(new_base) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__bases__ assignment: '{}' object layout differs from '{}'",
                    self.class_name(new_base),
                    self.class_name(old_base)
                ),
            ));
        }
        if !new_bases.iter().all(|&base| self.supports_rebase(base)) {
            return Err(unsupported());
        }

        // An inconsistent MRO in any descendant must leave every original tuple intact.
        let affected = self.heap.class_hierarchy(class);
        let mut proposed: HashMap<Id, Vec<Id>> = HashMap::new();
        for &owner in &affected {
            if !self.supports_rebase(owner) {
                return Err(unsupported());
            }
            let bases = if owner == class {
                &new_bases
            } else {
                &self.class_bases_tuple(owner).ok_or_else(unsupported)?
            };
            let ancestors = self.linearize_with(bases, |ctx, base| {
                proposed
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| ctx.lineage(base))
            })?;
            let mut mro = vec![owner];
            mro.extend(ancestors);
            proposed.insert(owner, mro);
        }
        let mut finalizers = false;
        for owner in affected {
            let mro = proposed.remove(&owner).unwrap();
            let ancestors = mro[1..].to_vec();
            let mro = self.alloc(Value::Tuple(mro));
            let Value::Class {
                scope,
                ancestors: old,
                ..
            } = self.heap.get_mut(owner)
            else {
                unreachable!()
            };
            let scope = *scope;
            for old in std::mem::replace(old, ancestors) {
                self.heap.weak_edge_removed(old);
            }
            let Value::Scope(frame) = self.heap.get_mut(scope) else {
                unreachable!()
            };
            let ns = frame.class_namespace.as_mut().unwrap();
            let old_mro = std::mem::replace(&mut ns.mro, mro);
            if owner == class {
                let old_bases = std::mem::replace(&mut ns.bases, storage);
                let supplied =
                    std::mem::replace(&mut ns.supplied_bases, (storage != value).then_some(value));
                self.heap.weak_edge_removed(old_bases);
                if let Some(old) = supplied {
                    self.heap.weak_edge_removed(old);
                }
            }
            self.heap.weak_edge_removed(old_mro);
            finalizers |= self.class_attr(owner, "__del__").is_some();
        }
        // A successful assignment reattaches every direct base, including retained bases.
        self.heap.reattach_class(class);
        if finalizers {
            self.register_class_finalizers();
        }
        Ok(())
    }

    pub(super) fn type_subclasses(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let class =
            super::builtins::noargs_method_receiver(self, "type", "__subclasses__", args, kwargs)?;
        if !matches!(self.heap.get(class), Value::Class { .. }) {
            // The weak class registry does not track the edges between static native types.
            return Err(Error::new(
                "NotImplementedError",
                "__subclasses__ is only supported for heap-defined classes",
            ));
        }
        let children = self.heap.immediate_subclasses(class);
        Ok(self.alloc(Value::List(children)))
    }

    pub(super) fn class_bases_tuple(&self, class: Id) -> Option<Vec<Id>> {
        let Value::Class { scope, .. } = self.heap.get(class) else {
            return None;
        };
        let ns = self.scope_data(*scope).class_namespace.as_ref()?;
        let Value::Tuple(bases) = self.heap.get(ns.bases) else {
            return None;
        };
        Some(bases.clone())
    }
}
