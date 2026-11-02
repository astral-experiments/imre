//! Object-valued members declared by a dynamically constructed slotted class.

use super::{Context, Error, Id, Result, Value};

impl Context {
    /// Construct the replacement for an ordinary object-root dataclass.
    pub(super) fn slotted_dataclass(&mut self, old: Id, namespace: Id) -> Result<Id> {
        let object = self.builtin("object");
        let Value::Class {
            scope: original,
            metaclass: None,
            ref ancestors,
            ..
        } = *self.heap.get(old)
        else {
            return Err(Error::new(
                "TypeError",
                "expected an ordinary dataclass type",
            ));
        };
        if ancestors.as_slice() != [object] {
            return Err(Error::new(
                "NotImplementedError",
                "boxed slotted dataclasses require an ordinary object-based class",
            ));
        }
        let name = self.class_name(old).to_owned();
        let module = self.scope_data(original).module.clone();
        let provider_annotations = self.has_class_annotation_provider(original);
        let class = self.dataclass_from_namespace(
            name.clone(),
            namespace,
            &module,
            true,
            provider_annotations,
        )?;
        if self.classes.get(&name) == Some(&old) {
            // This object-root type does not participate in exception-name dispatch.
            self.classes.remove(&name);
            self.heap.weak_edge_removed(old);
        }
        Ok(class)
    }

    /// Build a fresh object-root type and initialize its member descriptors.
    pub(super) fn dataclass_from_namespace(
        &mut self,
        name: String,
        namespace: Id,
        module: &str,
        member_slots: bool,
        provider_annotations: bool,
    ) -> Result<Id> {
        let object = self.builtin("object");
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| Error::new("TypeError", "expected a class namespace dictionary"))?
            .to_vec();
        let scope = self.scope(module, None);
        let cell = self.alloc(Value::ClassCell(None));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_body = Some(Vec::new());
        frame.class_cell = Some(cell);
        let layout = frame.class_namespace.get_or_insert_with(Default::default);
        layout.member_slots = member_slots;
        layout.provider_annotations = provider_annotations;
        for (key, value) in entries {
            let key = self.str_value(key)?.to_owned();
            self.bind_local(scope, &key, value);
        }
        let class = self.allocate_class(name, scope, vec![object])?;
        *self.heap.get_mut(cell) = Value::ClassCell(Some(class));
        self.install_weakref_descriptor(class, scope);
        self.initialize_class_members(class, scope)?;
        Ok(class)
    }

    /// A layout adding real object-valued slots must have one most-derived storage base.
    pub(super) fn check_member_slot_bases(&mut self, bases: &[Id], ancestors: &[Id]) -> Result<()> {
        if bases.len() < 2
            || !ancestors.iter().any(|&base| {
                matches!(self.heap.get(base), Value::Class {scope,..}
            if self.scope_data(*scope).class_namespace.as_ref().is_some_and(|ns| ns.member_slots))
            })
        {
            return Ok(());
        }
        let object = self.builtin("object");
        let mut solid = object;
        for &base in bases {
            let lineage = self.lineage(base);
            let candidate = lineage
                .into_iter()
                .find(|&owner| match self.heap.get(owner) {
                    Value::Class { scope, .. } => self
                        .scope_data(*scope)
                        .class_namespace
                        .as_ref()
                        .and_then(|ns| ns.slot_names.as_ref())
                        .is_some_and(|slots| {
                            slots.iter().any(|s| s != "__dict__" && s != "__weakref__")
                        }),
                    Value::Native(_) => owner != object,
                    _ => false,
                })
                .unwrap_or(object);
            if self.subclass(candidate, solid) {
                solid = candidate;
            } else if !self.subclass(solid, candidate) {
                return Err(Error::new(
                    "TypeError",
                    "multiple bases have instance lay-out conflict",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn prepare_slot_members(
        &self,
        class_name: &str,
        scope: Id,
        bases: &[Id],
        slots: &[String],
    ) -> Result<Vec<String>> {
        let mut weak = bases
            .iter()
            .any(|&base| self.class_instances_support_weakref(base));
        let mut members = Vec::new();
        for original in slots {
            if original == "__dict__" {
                return Err(Error::new(
                    "NotImplementedError",
                    "dynamic __dict__ slot descriptors are unsupported",
                ));
            }
            if original == "__weakref__" {
                if weak {
                    return Err(Error::new(
                        "TypeError",
                        "__weakref__ slot disallowed: either we already got one, or __itemsize__ != 0",
                    ));
                }
                weak = true;
                continue;
            }
            let owner = class_name.trim_start_matches('_');
            let name =
                if original.starts_with("__") && !original.ends_with("__") && !owner.is_empty() {
                    format!("_{owner}{original}")
                } else {
                    original.clone()
                };
            if self.scope_data(scope).names.contains_key(&name) {
                return Err(Error::new(
                    "ValueError",
                    format!("'{name}' in __slots__ conflicts with class variable"),
                ));
            }
            members.push(name);
        }
        members.sort();
        members.dedup();
        Ok(members)
    }

    fn check_slot_receiver(&self, member: Id, receiver: Id) -> Result<Id> {
        let Value::SlotMember { owner, name } = *self.heap.get(member) else {
            unreachable!()
        };
        if let Value::Instance { class, .. } = *self.heap.get(receiver)
            && self.subclass(class, owner)
        {
            return Ok(name);
        }
        Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                self.str_value(name)?,
                self.class_name(owner),
                self.type_name(receiver)
            ),
        ))
    }

    pub(super) fn read_slot_member(&self, member: Id, receiver: Id) -> Result<Id> {
        let name = self.check_slot_receiver(member, receiver)?;
        let Value::Instance { member_values, .. } = self.heap.get(receiver) else {
            unreachable!()
        };
        let name = self.str_value(name)?;
        member_values
            .as_deref()
            .and_then(|values| values.get(&member))
            .copied()
            .ok_or_else(|| self.missing_attribute(receiver, name))
    }

    pub(super) fn write_slot_member(
        &mut self,
        member: Id,
        receiver: Id,
        value: Option<Id>,
    ) -> Result<()> {
        let name = self.check_slot_receiver(member, receiver)?;
        let Value::Instance { member_values, .. } = self.heap.get_mut(receiver) else {
            unreachable!()
        };
        let previous = if let Some(value) = value {
            member_values
                .get_or_insert_with(Default::default)
                .insert(member, value)
        } else {
            let previous = member_values
                .as_deref_mut()
                .and_then(|values| values.remove(&member));
            if previous.is_none() {
                return Err(Error::new(
                    "AttributeError",
                    self.str_value(name)?.to_owned(),
                ));
            }
            self.heap.weak_edge_removed(member);
            previous
        };
        if let Some(old) = previous {
            self.heap.weak_edge_removed(old);
        }
        Ok(())
    }
}
