//! Object-valued members declared by a dynamically constructed slotted class.

use super::{Context, Error, Id, Result, Value};
use ruff_python_stdlib::identifiers::is_identifier;

impl Context {
    /// Follow stored direct bases without invoking Python before field validation.
    fn dataclass_slot_ancestors(&mut self, mut class: Id) -> Result<Vec<Id>> {
        let object = self.builtin("object");
        let mut ancestors = Vec::new();
        while class != object {
            let unsupported = || {
                Error::new(
                    "NotImplementedError",
                    "boxed slotted dataclasses require a single ordinary Python base",
                )
            };
            let Value::Class {
                scope,
                metaclass: None,
                namedtuple_fields: None,
                ..
            } = self.heap.get(class)
            else {
                return Err(unsupported());
            };
            let frame = self.scope_data(*scope);
            // Assigning __bases__ in boxed classes can shadow, but cannot rebuild, their layout.
            if self.namespace_binding(frame, "__bases__").is_some() {
                return Err(unsupported());
            }
            let layout = frame
                .class_namespace
                .as_deref()
                .filter(|_| frame.class_cell.is_some())
                .ok_or_else(unsupported)?;
            let Value::Tuple(bases) = self.heap.get(layout.bases) else {
                return Err(unsupported());
            };
            let [base] = bases.as_slice() else {
                return Err(unsupported());
            };
            class = *base;
            if class != object {
                ancestors.push(class);
            }
        }
        Ok(ancestors)
    }

    fn check_inherited_member_storage(&self, bases: &[Id]) -> Result<()> {
        for &base in bases {
            let Value::Class { scope, .. } = self.heap.get(base) else {
                unreachable!()
            };
            let layout = self.scope_data(*scope).class_namespace.as_deref().unwrap();
            if !layout.member_slots
                && layout.slot_names.as_deref().is_some_and(|slots| {
                    slots
                        .iter()
                        .any(|s| !matches!(s.as_str(), "__dict__" | "__weakref__"))
                })
            {
                return Err(Error::new(
                    "NotImplementedError",
                    "inherited named slots require owner-bound member descriptors",
                ));
            }
        }
        Ok(())
    }

    /// Supply the physical dict/weakref layout for bases that lack a slots declaration.
    pub(super) fn dataclass_slot_bases(&mut self, old: Id) -> Result<Id> {
        let bases = self.dataclass_slot_ancestors(old)?;
        // Plain declarations cannot raise the late iterator error. Decline unsupported
        // physical layouts here without modifying the class. Custom declarations must
        // run only after field validation, and may have observable iterator semantics.
        if bases.iter().all(|&base| {
            let Value::Class { scope, .. } = self.heap.get(base) else {
                return false;
            };
            self.namespace_binding(self.scope_data(*scope), "__slots__")
                .is_none_or(|slots| {
                    matches!(
                        self.heap.get(slots),
                        Value::None
                            | Value::Str(_)
                            | Value::List(_)
                            | Value::Tuple(_)
                            | Value::Set { .. }
                            | Value::Dict(_)
                    )
                })
        }) {
            self.check_inherited_member_storage(&bases)?;
        }
        let mut entries = Vec::new();
        for base in bases {
            let mut implicit = Vec::new();
            if self.class_instances_have_dict(base) {
                implicit.push(self.text("__dict__"));
            }
            if self.class_instances_support_weakref(base) {
                implicit.push(self.text("__weakref__"));
            }
            let implicit = self.alloc(Value::Tuple(implicit));
            entries.push(self.alloc(Value::Tuple(vec![base, implicit])));
        }
        // An ordinary object-based class cannot participate in exception-name dispatch.
        // A subsequent annotation, descriptor, or base-class hook can still raise.
        let Value::Class { name, .. } = self.heap.get(old) else {
            return Err(Error::new(
                "TypeError",
                "expected an ordinary dataclass type",
            ));
        };
        let name = name.clone();
        if self.classes.get(&name) == Some(&old) {
            self.classes.remove(&name);
            self.heap.weak_edge_removed(old);
        }
        Ok(self.alloc(Value::Tuple(entries)))
    }

    /// Construct the replacement using only layouts with real member storage.
    pub(super) fn slotted_dataclass(&mut self, old: Id, namespace: Id) -> Result<Id> {
        let ancestors = self.dataclass_slot_ancestors(old)?;
        self.check_inherited_member_storage(&ancestors)?;
        let Value::Class {
            scope: original, ..
        } = *self.heap.get(old)
        else {
            return Err(Error::new(
                "TypeError",
                "expected an ordinary dataclass type",
            ));
        };
        let name = self.class_name(old).to_owned();
        let module = self.scope_data(original).module.clone();
        let provider_annotations = self.has_class_annotation_provider(original);
        self.dataclass_from_namespace(
            name,
            ancestors.into_iter().take(1).collect(),
            namespace,
            &module,
            true,
            provider_annotations,
        )
    }

    pub(super) fn make_dataclass_type(
        &mut self,
        name: String,
        bases: Vec<Id>,
        namespace: Id,
    ) -> Result<Id> {
        let object = self.builtin("object");
        for &base in &bases {
            if base == object {
                continue;
            }
            let ordinary = match self.heap.get(base) {
                Value::Class {
                    scope,
                    metaclass: None,
                    ancestors,
                    ..
                } if ancestors.as_slice() == [object] => {
                    let frame = self.scope_data(*scope);
                    frame.class_cell.is_some()
                        && frame
                            .class_namespace
                            .as_ref()
                            .is_some_and(|layout| layout.slot_names.is_none())
                }
                _ => false,
            };
            if !ordinary {
                return Err(Error::new(
                    "NotImplementedError",
                    "boxed make_dataclass requires plain unslotted object-root bases",
                ));
            }
        }
        self.dataclass_from_namespace(name, bases, namespace, "types", false, true)
    }

    /// Build a plain dataclass type, initializing descriptors before inherited subclass hooks.
    fn dataclass_from_namespace(
        &mut self,
        name: String,
        bases: Vec<Id>,
        namespace: Id,
        module: &str,
        member_slots: bool,
        provider_annotations: bool,
    ) -> Result<Id> {
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| Error::new("TypeError", "expected a class namespace dictionary"))?
            .to_vec();
        let scope = self.scope(module, None);
        let cell = self.alloc(Value::Cell(None));
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
        let class = self.allocate_class(name, scope, bases)?;
        self.heap.replace_cell(cell, Some(class));
        self.install_weakref_descriptor(class, scope);
        self.initialize_class_members(class, scope)?;
        let hook = self.super_attribute(class, class, class, "__init_subclass__")?;
        self.call(hook, Vec::new(), Vec::new())?;
        Ok(class)
    }

    /// Calendar payloads and object-valued slots require one most-derived storage base.
    pub(super) fn check_member_slot_bases(&mut self, bases: &[Id], ancestors: &[Id]) -> Result<()> {
        if bases.len() < 2
            || !ancestors.iter().any(|&base| match self.heap.get(base) {
                Value::Native(name) => super::stdlib::datetime::members(name).is_some()
                    || matches!(name.as_str(), "int" | "str" | "bytes" | "bytearray" | "tuple" | "list" | "dict" | "set" | "frozenset" | "float"),
                Value::Class { scope, .. } => self
                    .scope_data(*scope)
                    .class_namespace
                    .as_ref()
                    .is_some_and(|ns| ns.member_slots),
                _ => false,
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
                    // SimpleNamespace stores its dictionary in the object itself.
                    Value::Class { .. }
                        if self.classes.get(super::namespace::CLASS) == Some(&owner) =>
                    {
                        true
                    }
                    Value::Class { scope, .. } => self
                        .scope_data(*scope)
                        .class_namespace
                        .as_ref()
                        .and_then(|ns| ns.slot_names.as_ref())
                        .is_some_and(|slots| {
                            slots.iter().any(|s| s != "__dict__" && s != "__weakref__")
                        }),
                    Value::Native(name) => {
                        !matches!(name.as_str(), "object" | "enum.Enum" | "datetime.tzinfo")
                    }
                    _ => false,
                })
                .unwrap_or(object);
            if self.direct_subclass(candidate, solid) {
                solid = candidate;
            } else if !self.direct_subclass(solid, candidate) {
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
        ordinary_slots: bool,
    ) -> Result<Vec<String>> {
        let mut weak = bases
            .iter()
            .any(|&base| self.class_instances_support_weakref(base));
        let mut members = Vec::new();
        for original in slots {
            if !is_identifier(original) {
                return Err(Error::new("TypeError", "__slots__ must be identifiers"));
            }
            if original == "__dict__" {
                if ordinary_slots {
                    // Ordinary instances already supply dictionary storage through their layout.
                    continue;
                }
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
            if self.namespace_binding(self.scope_data(scope), &name).is_some() {
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
            && self.direct_subclass(class, owner)
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
