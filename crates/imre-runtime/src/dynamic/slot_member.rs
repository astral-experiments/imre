//! Namespace-based class construction and object-valued slot descriptors.

use super::{Context, Error, Id, NONE, Result, Value};
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
        let layout = super::value::ClassNamespace {
            member_slots,
            provider_annotations,
            ..Default::default()
        };
        let cell = self.alloc(Value::Cell(None));
        let scope = self.namespace_scope(module, layout, cell);
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| Error::new("TypeError", "expected a class namespace dictionary"))?
            .to_vec();
        for (key, value) in entries {
            let key = self.str_value(key)?.to_owned();
            self.bind_local(scope, &key, value);
        }
        let class = self.allocate_class(name, scope, bases)?;
        self.finish_namespace_class(class, scope, cell, None, Vec::new())
    }

    /// Validate the receiver of the unbound ABCMeta.__new__ method.
    pub(super) fn abc_new_from_namespace(
        &mut self,
        meta: Id,
        name: Id,
        bases: Id,
        namespace: Id,
        keywords: Vec<(String, Id)>,
    ) -> Result<Id> {
        if !self.is_abc_metaclass(meta) {
            return Err(Error::new("TypeError", "ABCMeta.__new__ requires an ABCMeta subtype"));
        }
        self.type_from_namespace(Some(meta), name, bases, namespace, &keywords)
    }

    /// Construct a class from a copied namespace, optionally using an ABC metaclass.
    pub(super) fn type_from_namespace(
        &mut self,
        explicit_meta: Option<Id>,
        name: Id,
        base_tuple: Id,
        namespace: Id,
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let argument = |ctx: &Context, position, expected, value| {
            Error::new(
                "TypeError",
                format!(
                    "type.__new__() argument {position} must be {expected}, not {}",
                    ctx.type_name(value)
                ),
            )
        };
        let name_storage = self
            .string_storage(name)
            .ok_or_else(|| argument(self, 1, "str", name))?;
        let bases = self
            .tuple_items(base_tuple)?
            .ok_or_else(|| argument(self, 2, "tuple", base_tuple))?;
        let storage = self
            .dict_storage(namespace)
            .ok_or_else(|| argument(self, 3, "dict", namespace))?;
        let type_class = self.builtin("type");
        let mut specialized = false;
        for &base in &bases {
            let meta = self.exact_type(base)?;
            if self.direct_subclass(meta, type_class) {
                specialized |= self.direct_subclass(base, type_class)
                    || meta != type_class
                    || self.is_bundled_abc(base);
                continue;
            }
            match self.lookup_attr(base, "__mro_entries__") {
                Ok(_) => {
                    return Err(Error::new(
                        "TypeError",
                        "type() doesn't support MRO entry resolution; use types.new_class()",
                    ));
                }
                Err(error) if error.kind != "AttributeError" => return Err(error),
                Err(_) => {}
            }
            return Err(Error::new(
                "TypeError",
                "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
            ));
        }
        let metaclass = if specialized || explicit_meta.is_some() {
            let winner = self.select_class_metaclass(
                &bases,
                Some(explicit_meta.unwrap_or(type_class)),
                None,
                false,
            )?;
            let Some(winner) = winner.filter(|&meta| self.is_abc_metaclass(meta)) else {
                return Err(Error::new(
                    "NotImplementedError",
                    "type() with specialized metaclasses is unsupported",
                ));
            };
            if bases.iter().any(|&base| self.direct_subclass(base, type_class)) {
                return Err(Error::new(
                    "NotImplementedError",
                    "type() with specialized metaclasses is unsupported",
                ));
            }
            let abc = self.abc_meta();
            if explicit_meta != Some(winner)
                && self.class_attr(winner, "__new__") != self.class_attr(abc, "__new__")
            {
                let result = self.call_abc_new(winner, name, base_tuple, namespace, kwargs.to_vec())?;
                if explicit_meta.is_some() {
                    // The outer ABCMeta.__new__ also initializes a more derived __new__ result.
                    let Value::Class { scope, .. } = *self.heap.get(result) else {
                        self.lookup_attr(result, "__dict__")?;
                        return Err(Error::new("NotImplementedError", "ABC initialization requires a Python class"));
                    };
                    if !self.is_bundled_abc(result) {
                        return Err(Error::new(
                            "NotImplementedError",
                            "ABC initialization of an unrelated metaclass result is unsupported",
                        ));
                    }
                    self.init_abstract_class(result, scope)?;
                }
                return Ok(result);
            }
            Some(winner)
        } else {
            None
        };
        let layout = super::typing::ClassLayout::TypeNamespace;
        let bases = self.validate_class_bases(bases, layout)?;
        let layout_base = self.member_layout_base(&bases)?;
        let namespace = super::builtins::copy_dict(self, namespace, storage)?;
        let slots =
            self.declared_slot_names(self.dict_named_item(namespace, "__slots__"), layout_base)?;
        if matches!(self.heap.get(name_storage), Value::SurrogateStr(_)) {
            self.encode_utf8_codepoints(name_storage, false, "strict")?;
        }
        let name_text = self.str_value(name)?.to_owned();
        if name_text.contains('\0') {
            return Err(Error::new(
                "ValueError",
                "type name must not contain null characters",
            ));
        }
        let entries = self
            .dict_entries(namespace)
            .expect("copied type namespace")
            .to_vec();
        for &(key, value) in &entries {
            match self.heap.get(key) {
                Value::Str(_) => {}
                Value::SurrogateStr(_) => {
                    return Err(Error::new(
                        "NotImplementedError",
                        "surrogate-containing type namespace keys are unsupported",
                    ));
                }
                _ => {
                    return Err(Error::new(
                        "NotImplementedError",
                        "type namespace keys other than exact strings are unsupported",
                    ));
                }
            }
            self.heap.keep(key);
            self.heap.keep(value);
        }
        // Inherited ABCMeta.__new__ calls type.__new__ from abc's defining globals.
        let creator = metaclass.and(self.builtins.get("abc.ABCMeta").copied());
        let creator_scope = match creator.map(|meta| self.heap.get(meta)) {
            Some(Value::Class { scope, .. }) => *scope,
            _ => self.heap.current_scope(),
        };
        let globals = self.module_globals(creator_scope)?;
        let module = self.dict_named_item(globals, "__name__");
        let module_text = self.scope_data(creator_scope).module.clone();
        let cell = match self.type_namespace_cell("__classcell__", self.dict_named_item(namespace, "__classcell__"))? {
            Some(cell) => cell,
            None => self.alloc(Value::Cell(None)),
        };
        let dict_cell = self.dict_named_item(namespace, "__classdictcell__");
        let class_namespace = super::value::ClassNamespace {
            name_override: Some(name),
            dictionary: Some(namespace),
            ..Default::default()
        };
        let scope = self.namespace_scope(&module_text, class_namespace, cell);
        for (key_id, value) in entries {
            let key = self.str_value(key_id)?.to_owned();
            if matches!(key.as_str(), "__classcell__" | "__classdictcell__") {
                self.delete_item(namespace, key_id)?;
                continue;
            }
            if key == "__qualname__" && self.string_storage(value).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "type __qualname__ must be a str, not {}",
                        self.type_name(value)
                    ),
                ));
            }
            let value =
                if key == "__new__" && matches!(self.heap.get(value), Value::Function { .. }) {
                    super::method_decorator::static_method(self, value)?
                } else {
                    value
                };
            self.bind_local(scope, &key, value);
        }
        if self.scope_name(scope, "__module__").is_none()
            && let Some(module) = module
        {
            self.bind_local(scope, "__module__", module);
        }
        if self.scope_name(scope, "__qualname__").is_none() {
            self.bind_local(scope, "__qualname__", name);
        }
        if self.scope_name(scope, "__annotate_func__").is_some() {
            self.enable_class_annotation_provider(scope);
        }
        if metaclass.is_some() {
            let original = self.scope_name(scope, "__orig_bases__");
            super::typing::class_parameters(self, scope, original, &bases)?;
        }
        let ancestors = self.linearized_bases(&bases, false)?;
        let class =
            self.allocate_prepared_class(name_text, scope, bases, ancestors, layout, slots)?;
        if self.scope_name(scope, "__doc__").is_none() {
            self.bind_local(scope, "__doc__", NONE);
        }
        let qualname = self.heap.code_name("__qualname__");
        self.delete_item(namespace, qualname)?;
        let Value::Class {
            metaclass: class_meta,
            ..
        } = self.heap.get_mut(class)
        else {
            unreachable!()
        };
        *class_meta = metaclass;
        self.finish_namespace_class(class, scope, cell, dict_cell, kwargs.to_vec())?;
        if metaclass.is_some() {
            self.init_abstract_class(class, scope)?;
        }
        Ok(class)
    }

    fn type_namespace_cell(&self, name: &str, cell: Option<Id>) -> Result<Option<Id>> {
        if let Some(value) = cell
            && !matches!(self.heap.get(value), Value::Cell(_))
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{name} must be a nonlocal cell, not <class '{}'>",
                    self.type_name(value)
                ),
            ));
        }
        Ok(cell)
    }

    /// Keep the namespace and closure cell live while descriptors and subclass hooks can run Python.
    fn namespace_scope(
        &mut self,
        module: &str,
        layout: super::value::ClassNamespace,
        cell: Id,
    ) -> Id {
        let scope = self.scope(module, None);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_body = Some(Vec::new());
        frame.class_cell = Some(cell);
        frame.class_namespace = Some(Box::new(layout));
        scope
    }

    /// Bind __class__ before calling descriptor and inherited subclass hooks.
    fn finish_namespace_class(
        &mut self,
        class: Id,
        scope: Id,
        cell: Id,
        dict_cell: Option<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        self.heap.replace_cell(cell, Some(class));
        // A valid __classcell__ is populated even when the following dictionary cell is invalid.
        if let Some(dict_cell) = self.type_namespace_cell("__classdictcell__", dict_cell)? {
            let Value::Scope(frame) = self.heap.get_mut(scope) else { unreachable!() };
            frame.class_namespace.as_mut().expect("namespace class").annotation_dict_cell = Some(dict_cell);
            let dict = self.class_namespace_dict(scope, false);
            self.heap.replace_cell(dict_cell, Some(dict));
        }
        self.install_weakref_descriptor(class, scope);
        self.initialize_class_members(class, scope, None)?;
        let hook = self.super_attribute(class, class, class, "__init_subclass__")?;
        self.call(hook, Vec::new(), kwargs)?;
        Ok(class)
    }

    /// Extension types need their physical layout modeled before accepting a dynamic subtype.
    pub(super) fn check_namespace_layouts(&self, bases: &[Id]) -> Result<()> {
        for &base in bases {
            let owner = self.allocation_base(base).unwrap_or(base);
            let supported = match self.heap.get(owner) {
                Value::Native(name) if super::builtins::is_exception(name) => bases.len() == 1,
                Value::Native(name) => {
                    super::builtins::is_exported_type(name)
                        || matches!(
                            name.as_str(),
                            "module"
                                | "reversed"
                                | "itertools.count"
                                | "itertools.repeat"
                                | super::deque::CLASS
                        )
                }
                _ => false,
            };
            if !supported {
                return Err(Error::new(
                    "NotImplementedError",
                    format!(
                        "dynamic type() does not support the layout of base '{}'",
                        self.class_name(base)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Select the direct base that supplies instance storage, retaining the first base on a tie.
    pub(super) fn member_layout_base(&mut self, bases: &[Id]) -> Result<Id> {
        if bases.len() < 2
            || !bases.iter().any(|&base| {
                self.lineage(base)
                    .into_iter()
                    .any(|base| match self.heap.get(base) {
                        Value::Class { .. }
                            if self.classes.get(super::namespace::CLASS) == Some(&base) =>
                        {
                            true
                        }
                        Value::Native(name) => {
                            super::stdlib::datetime::members(name).is_some()
                                || matches!(name.as_str(), "module" | super::deque::CLASS)
                                || super::builtins::is_iterator_type(name)
                                || (name != "object"
                                    && super::builtins::is_exported_type(name)
                                    && !super::builtins::is_exception(name))
                        }
                        Value::Class { scope, .. } => self
                            .scope_data(*scope)
                            .class_namespace
                            .as_ref()
                            .is_some_and(|ns| ns.member_slots),
                        _ => false,
                    })
            })
        {
            return Ok(bases[0]);
        }
        let object = self.builtin("object");
        let mut winner = bases[0];
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
            if self.direct_subclass(solid, candidate) {
                continue;
            }
            if self.direct_subclass(candidate, solid) {
                solid = candidate;
                winner = base;
            } else {
                return Err(Error::new(
                    "TypeError",
                    "multiple bases have instance lay-out conflict",
                ));
            }
        }
        Ok(winner)
    }

    /// Check storage reserved by the best base after materializing the slots iterable.
    pub(super) fn checked_slot_names(&self, base: Id, slots: &[Id]) -> Result<Vec<String>> {
        if slots.is_empty() {
            return Ok(Vec::new());
        }
        if ["int", "tuple", "bytes", "type"]
            .iter()
            .any(|name| self.has_base(base, name))
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "nonempty __slots__ not supported for subtype of '{}'",
                    self.class_name(base)
                ),
            ));
        }
        let native = self
            .allocation_base(base)
            .and_then(|owner| match self.heap.get(owner) {
                Value::Native(name) => Some(name.as_str()),
                _ => None,
            });
        let mut dictionary = self.class_instances_have_dict(base)
            || matches!(native, Some("classmethod" | "staticmethod" | "module"))
            || native.is_some_and(super::builtins::is_exception);
        let mut weakref = self.class_instances_support_weakref(base)
            || matches!(
                native,
                Some("set" | "frozenset" | "module" | super::deque::CLASS)
            );
        let mut names = Vec::with_capacity(slots.len());
        for &slot in slots {
            let name = self.str_value(slot).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "__slots__ items must be strings, not '{}'",
                        self.type_name(slot)
                    ),
                )
            })?;
            if !is_identifier(name) {
                return Err(Error::new("TypeError", "__slots__ must be identifiers"));
            }
            let occupied = match name {
                "__dict__" => std::mem::replace(&mut dictionary, true),
                "__weakref__" => std::mem::replace(&mut weakref, true),
                _ => false,
            };
            if occupied {
                return Err(Error::new(
                    "TypeError",
                    format!("{name} slot disallowed: we already got one"),
                ));
            }
            names.push(name.to_owned());
        }
        Ok(names)
    }

    pub(super) fn prepare_slot_members(
        &self,
        class_name: &str,
        scope: Id,
        slots: &[String],
        ordinary_slots: bool,
    ) -> Result<Vec<String>> {
        let mut members = Vec::new();
        for original in slots {
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
                continue;
            }
            let owner = class_name.trim_start_matches('_');
            let name =
                if original.starts_with("__") && !original.ends_with("__") && !owner.is_empty() {
                    format!("_{owner}{original}")
                } else {
                    original.clone()
                };
            if self
                .namespace_binding(self.scope_data(scope), &name)
                .is_some()
            {
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
