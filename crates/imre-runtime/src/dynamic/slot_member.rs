//! Namespace-based class construction and object-valued slot descriptors.

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};
use crate::dynamic::value::SlotStorage;
use ruff_python_stdlib::identifiers::is_identifier;

/// These native bases keep a variable-sized payload before a subclass instance dictionary.
pub(super) const VARIABLE_SIZE_BASES: [&str; 3] = ["int", "tuple", "bytes"];

/// Python mangles private member names with the name of the declaring class.
pub(super) fn slot_name(class_name: &str, original: &str) -> String {
    let owner = class_name.trim_start_matches('_');
    if original.starts_with("__") && !original.ends_with("__") && !owner.is_empty() {
        format!("_{owner}{original}")
    } else {
        original.to_owned()
    }
}

impl Context {
    /// Inspect inherited slot layouts in method resolution order, without invoking Python.
    fn dataclass_slot_ancestors(&mut self, class: Id) -> Result<Vec<Id>> {
        let object = self.builtin("object");
        let generic = self.classes.get("typing.Generic").copied();
        let lineage = self.lineage(class);
        for &owner in &lineage {
            // Generic is a bootstrapped base with no instance storage or Python class cell.
            if owner == object || (owner != class && Some(owner) == generic) {
                continue;
            }
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
            } = self.heap.get(owner)
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
                .filter(|layout| {
                    // A storage-free ancestor using object's allocator needs no cell of its own.
                    frame.class_cell.is_some()
                        || (owner != class
                            && layout.slot_names.as_deref() == Some(&[])
                            && self.allocation_base(owner) == Some(object))
                })
                .ok_or_else(unsupported)?;
            let Value::Tuple(bases) = self.heap.get(layout.bases) else {
                return Err(unsupported());
            };
            match bases.as_slice() {
                [_] => {}
                [left, right] if Some(*left) == generic || Some(*right) == generic => {}
                _ => return Err(unsupported()),
            }
        }
        Ok(lineage
            .into_iter()
            .skip(1)
            .filter(|&base| base != object)
            .collect())
    }

    /// Read the declared object slots from the immutable layout, even if Python rebinds __slots__.
    pub(super) fn declares_named_slots(&self, owner: Id) -> bool {
        let Value::Class { scope, .. } = self.heap.get(owner) else {
            return false;
        };
        self.scope_data(*scope)
            .class_namespace
            .as_deref()
            .and_then(|layout| layout.slot_names.as_deref())
            .is_some_and(|names| {
                names
                    .iter()
                    .any(|name| !matches!(name.as_str(), "__dict__" | "__weakref__"))
            })
    }

    fn check_inherited_member_storage(&self, bases: &[Id]) -> Result<()> {
        for &base in bases {
            let Value::Class { scope, .. } = self.heap.get(base) else {
                unreachable!()
            };
            let layout = self.scope_data(*scope).class_namespace.as_deref().unwrap();
            if !layout.member_slots && self.declares_named_slots(base) {
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
        if !matches!(self.heap.get(old), Value::Class { .. }) {
            return Err(Error::new(
                "TypeError",
                "expected an ordinary dataclass type",
            ));
        }
        Ok(self.alloc(Value::Tuple(entries)))
    }

    /// Snapshot a class namespace, removing the old type's descriptors before slot replacement.
    pub(super) fn dataclass_members(&mut self, class: Id, for_slots: bool) -> Result<Id> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            return Err(Error::new(
                "TypeError",
                "invalid arguments to _imre_dataclasses.members",
            ));
        };
        if for_slots {
            self.dataclass_slot_ancestors(class)?;
            let layout = self
                .scope_data(scope)
                .class_namespace
                .as_deref()
                .expect("ordinary class");
            if layout.immutable_type || self.heap.get(class).heap_type().is_some() {
                return Err(Error::new("TypeError", "expected a mutable dataclass type"));
            }
            for name in ["__dict__", "__weakref__"] {
                if let Some(previous) = self.namespace_binding(self.scope_data(scope), name) {
                    self.heap.keep(previous);
                }
                let key = self.heap.code_name(name);
                if self.remove_binding(scope, name, key)? {
                    let Value::Scope(frame) = self.heap.get_mut(scope) else {
                        unreachable!()
                    };
                    frame
                        .class_namespace
                        .as_mut()
                        .expect("ordinary class")
                        .layout_descriptors_cleared = true;
                }
            }
        }
        let dictionary = self.class_namespace_dict(scope, false);
        let members = self
            .dict_entries(dictionary)
            .expect("class dictionary")
            .to_vec();
        Ok(self.dict(members))
    }

    /// Cleared layout descriptors must not be synthesized again for an instance or subclass.
    pub(super) fn layout_descriptors_cleared(&self, class: Id) -> bool {
        self.find_in_mro(class, |owner| {
            let Value::Class { scope, .. } = *self.heap.get(owner) else {
                return None;
            };
            self.scope_data(scope)
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.layout_descriptors_cleared)
                .then_some(())
        })
        .is_some()
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
        let name = self.lookup_attr(old, "__name__")?;
        let name_text = self.str_value(name)?.to_owned();
        let module = self.scope_data(original).module.clone();
        let layout = super::value::ClassNamespace {
            name_override: Some(name),
            member_slots: true,
            provider_annotations: self.has_class_annotation_provider(original),
            ..Default::default()
        };
        let cell = self.alloc(Value::Cell(None));
        let scope = self.namespace_scope(&module, layout, cell);
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| Error::new("TypeError", "expected a class namespace dictionary"))?
            .to_vec();
        for (key, value) in entries {
            let key = self.str_value(key)?.to_owned();
            self.bind_local(scope, &key, value);
        }
        let bases = self
            .class_bases_tuple(old)
            .expect("validated dataclass bases");
        let (class, layout_base) = self.allocate_class_with_slots(
            name_text,
            scope,
            bases,
            super::typing::ClassLayout::Specialized,
            None,
        )?;
        // The initial qualified name is type metadata, outside the __set_name__ member walk.
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.names.insert(crate::Str::new("__qualname__"), name);
        self.finish_namespace_class(class, scope, layout_base, cell, None, Vec::new())
    }

    /// Allocate through type.__new__, validating its explicit receiver and positional arguments.
    pub(super) fn type_new(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&meta, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "type.__new__(): not enough arguments",
            ));
        };
        let type_class = self.builtin("type");
        if !self.direct_subclass(meta, type_class) {
            let owner = self.exact_type(meta)?;
            let message = if self.direct_subclass(owner, type_class) {
                let name = self.class_name(meta);
                format!("type.__new__({name}): {name} is not a subtype of type")
            } else {
                format!(
                    "type.__new__(X): X is not a type object ({})",
                    self.type_name(meta)
                )
            };
            return Err(Error::new("TypeError", message));
        }
        let [name, bases, namespace] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type.__new__() takes exactly 3 arguments ({} given)",
                    args.len()
                ),
            ));
        };
        self.type_from_namespace(Some(meta), *name, *bases, *namespace, kwargs, false)
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
            return Err(Error::new(
                "TypeError",
                "ABCMeta.__new__ requires an ABCMeta subtype",
            ));
        }
        self.type_from_namespace(Some(meta), name, bases, namespace, &keywords, true)
    }

    /// Construct a class from a copy of its namespace, including any deferred annotation cells.
    pub(super) fn type_from_namespace(
        &mut self,
        explicit_meta: Option<Id>,
        name: Id,
        base_tuple: Id,
        namespace: Id,
        kwargs: &[(String, Id)],
        abc_new: bool,
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
        let supplied_bases = (!bases.is_empty()).then_some(base_tuple);
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
            specialized = true;
        }
        let metaclass = if specialized || explicit_meta.is_some() {
            let winner = self.select_class_metaclass(
                &bases,
                Some(explicit_meta.unwrap_or(type_class)),
                self.module_value("typing", "_TypedDictMeta"),
                false,
            )?;
            if let Some(winner) = winner
                && explicit_meta != Some(winner)
                && self.class_attr(winner, "__new__").is_some()
            {
                let result =
                    self.call_metaclass_new(winner, name, base_tuple, namespace, kwargs.to_vec())?;
                if abc_new {
                    // The outer ABCMeta.__new__ also initializes a more derived __new__ result.
                    let Value::Class { scope, .. } = *self.heap.get(result) else {
                        self.lookup_attr(result, "__dict__")?;
                        return Err(Error::new(
                            "NotImplementedError",
                            "ABC initialization requires a Python class",
                        ));
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
            winner
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
        let creator = abc_new.then(|| self.abc_meta());
        let creator_scope = match creator.map(|meta| self.heap.get(meta)) {
            Some(Value::Class { scope, .. }) => *scope,
            _ => self.heap.current_scope(),
        };
        let globals = self.module_globals(creator_scope)?;
        let module = self.dict_named_item(globals, "__name__");
        let module_text = self.scope_data(creator_scope).module.clone();
        let cell = match self.type_namespace_cell(
            "__classcell__",
            self.dict_named_item(namespace, "__classcell__"),
        )? {
            Some(cell) => cell,
            None => self.alloc(Value::Cell(None)),
        };
        let dict_cell = self.dict_named_item(namespace, "__classdictcell__");
        let class_namespace = super::value::ClassNamespace {
            supplied_bases,
            name_override: Some(name),
            dictionary: Some(namespace),
            ..Default::default()
        };
        let scope = self.namespace_scope(&module_text, class_namespace, cell);
        for (key, value) in entries {
            match self.str_value(key)? {
                "__classcell__" | "__classdictcell__" => self.delete_item(namespace, key)?,
                "__qualname__" if self.string_storage(value).is_none() => {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "type __qualname__ must be a str, not {}",
                            self.type_name(value)
                        ),
                    ));
                }
                "__new__" if matches!(self.heap.get(value), Value::Function { .. }) => {
                    let wrapped = super::method_decorator::static_method(self, value)?;
                    self.bind_local(scope, "__new__", wrapped);
                }
                _ => {}
            }
        }
        if self.scope_name(scope, "__module__").is_none()
            && let Some(module) = module
        {
            self.bind_local(scope, "__module__", module);
        }
        let qualname = self.scope_name(scope, "__qualname__").unwrap_or_else(|| {
            self.bind_local(scope, "__qualname__", name);
            name
        });
        self.enable_class_annotation_provider(scope);
        let ancestors = self.linearized_bases(&bases, false)?;
        let class =
            self.allocate_prepared_class(name_text, scope, bases, ancestors, layout, slots)?;
        if self.scope_name(scope, "__doc__").is_none() {
            self.bind_local(scope, "__doc__", NONE);
        }
        // type.__qualname__ lives outside the published class dictionary.
        let key = self.heap.code_name("__qualname__");
        self.delete_item(namespace, key)?;
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.names.insert("__qualname__".into(), qualname);
        let Value::Class {
            metaclass: class_meta,
            ..
        } = self.heap.get_mut(class)
        else {
            unreachable!()
        };
        *class_meta = metaclass;
        self.finish_namespace_class(class, scope, layout_base, cell, dict_cell, kwargs.to_vec())?;
        if abc_new {
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
        frame.in_class_body = true;
        frame.class_cell = Some(cell);
        // type.__new__ has already copied the supplied mapping into the type's own dictionary.
        frame.namespace = layout.dictionary;
        frame.class_namespace = Some(Box::new(layout));
        scope
    }

    /// Bind __class__ before calling descriptor and inherited subclass hooks.
    fn finish_namespace_class(
        &mut self,
        class: Id,
        scope: Id,
        layout_base: Id,
        cell: Id,
        dict_cell: Option<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        self.heap.replace_cell(cell, Some(class));
        // A valid __classcell__ is populated even when the following dictionary cell is invalid.
        if let Some(dict_cell) = self.type_namespace_cell("__classdictcell__", dict_cell)? {
            let Value::Scope(frame) = self.heap.get_mut(scope) else {
                unreachable!()
            };
            frame
                .class_namespace
                .as_mut()
                .expect("namespace class")
                .annotation_dict_cell = Some(dict_cell);
            let dict = self.class_namespace_dict(scope, false);
            self.heap.replace_cell(dict_cell, Some(dict));
        }
        self.install_instance_dictionary_descriptor(class, scope, layout_base);
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
                                | super::stdlib::functools::PARTIAL_CLASS
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
                                || matches!(
                                    name.as_str(),
                                    "module"
                                        | super::deque::CLASS
                                        | super::stdlib::functools::PARTIAL_CLASS
                                )
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
        if VARIABLE_SIZE_BASES
            .into_iter()
            .chain(["type"])
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
        let partial_base = native == Some(super::stdlib::functools::PARTIAL_CLASS);
        let mut dictionary = self.class_instances_have_dict(base);
        let mut weakref = self.class_instances_support_weakref(base)
            || partial_base
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
    ) -> Result<Vec<String>> {
        let mut members = Vec::new();
        for original in slots {
            // Layout getsets supply the dictionary and weak references independently of named slots.
            if matches!(original.as_str(), "__dict__" | "__weakref__") {
                continue;
            }
            let name = slot_name(class_name, original);
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

    fn check_slot_receiver(&self, member: Id, receiver: Id) -> Result<(Id, bool, SlotStorage)> {
        let Value::SlotMember {
            owner,
            name,
            storage,
        } = *self.heap.get(member)
        else {
            unreachable!()
        };
        let finalizing = self.heap.get(owner).io_kind() == Some("TextIOWrapper")
            && self.str_value(name)? == "_finalizing";
        if matches!(storage, SlotStorage::Exception(_, _))
            && matches!(self.heap.get(receiver), Value::Raised(error) if self.raised_error_matches(error, owner))
            || !matches!(storage, SlotStorage::Exception(_, _))
                && matches!(self.heap.get(receiver), Value::Instance { class, .. } if self.direct_subclass(*class, owner))
            || matches!(storage, SlotStorage::Module(_))
                && matches!(self.heap.get(receiver), Value::Module { constructor: Some(class), .. } if self.direct_subclass(*class, owner))
        {
            return Ok((name, finalizing, storage));
        }
        let owner_name = if finalizing {
            "_io.TextIOWrapper"
        } else {
            self.heap
                .get(owner)
                .heap_type()
                .unwrap_or_else(|| self.class_name(owner))
        };
        let receiver_name = self.type_name(receiver);
        let prefix = if finalizing
            && matches!(self.heap.get(receiver), Value::Instance { class, .. } if self.heap.get(*class).io_kind().is_some())
        {
            "_io."
        } else {
            ""
        };
        Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{}' for '{owner_name}' objects doesn't apply to a '{prefix}{receiver_name}' object",
                self.str_value(name)?
            ),
        ))
    }

    pub(super) fn read_slot_member(&mut self, member: Id, receiver: Id) -> Result<Id> {
        let (name, finalizing, storage) = self.check_slot_receiver(member, receiver)?;
        if let SlotStorage::Exception(kind, index) = storage {
            let field = super::exception_members::names(kind)[index];
            return super::exception_members::read(self, kind, field, receiver);
        }
        if let SlotStorage::ReadOnlyText(text) = storage {
            return Ok(self.text(self.str_value(text)?.to_owned()));
        }
        if let Value::Module {
            module_slots,
            constructor: Some(class),
            ..
        } = self.heap.get(receiver)
            && let SlotStorage::Module(offset) = storage
        {
            let name = self.str_value(name)?;
            return module_slots
                .as_ref()
                .and_then(|slots| slots.get(&offset))
                .copied()
                .ok_or_else(|| {
                    let qualname = self.class_qualname(*class);
                    let module = self
                        .own_class_attr(*class, "__module__")
                        .and_then(|name| self.str_value(name).ok())
                        .filter(|name| !matches!(*name, "__main__" | "builtins"));
                    let owner = module.map_or_else(
                        || qualname.to_owned(),
                        |module| format!("{module}.{qualname}"),
                    );
                    Error::new(
                        "AttributeError",
                        format!("'{owner}' object has no attribute '{name}'"),
                    )
                });
        }
        let Value::Instance { member_values, .. } = self.heap.get(receiver) else {
            unreachable!()
        };
        let name = self.str_value(name)?;
        member_values
            .as_ref()
            .and_then(|values| values.get(&member))
            .copied()
            .or_else(|| finalizing.then_some(FALSE))
            .ok_or_else(|| self.missing_attribute(receiver, name))
    }

    pub(super) fn write_slot_member(
        &mut self,
        member: Id,
        receiver: Id,
        value: Option<Id>,
    ) -> Result<()> {
        let (name, finalizing, storage) = self.check_slot_receiver(member, receiver)?;
        if let SlotStorage::Exception(kind, index) = storage {
            let field = super::exception_members::names(kind)[index];
            return super::exception_members::write(self, kind, field, receiver, value);
        }
        if matches!(storage, SlotStorage::ReadOnlyText(_)) {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let SlotStorage::Module(offset) = storage
            && let Value::Module { module_slots, .. } = self.heap.get_mut(receiver)
        {
            let old = if let Some(value) = value {
                module_slots
                    .get_or_insert_with(Default::default)
                    .insert(offset, value)
            } else {
                let previous = module_slots
                    .as_mut()
                    .and_then(|slots| slots.remove(&offset));
                if previous.is_none() {
                    return Err(Error::new(
                        "AttributeError",
                        self.str_value(name)?.to_owned(),
                    ));
                }
                previous
            };
            if let Some(old) = old {
                self.heap.weak_edge_removed(old);
            }
            return Ok(());
        }
        if finalizing {
            let value = value
                .ok_or_else(|| Error::new("TypeError", "can't delete numeric/char attribute"))?;
            if !matches!(value, TRUE | FALSE) {
                return Err(Error::new("TypeError", "attribute value type must be bool"));
            }
        }
        let Value::Instance { member_values, .. } = self.heap.get_mut(receiver) else {
            unreachable!()
        };
        let previous = if let Some(value) = value {
            member_values
                .get_or_insert_with(Default::default)
                .insert(member, value)
        } else {
            let previous = member_values
                .as_mut()
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
