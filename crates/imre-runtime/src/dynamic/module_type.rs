use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const DOC: &str = "Create a module object.\n\nThe name must be a string; the optional doc argument can have any type.";

impl Context {
    /// Publish the module type's own descriptors in type-dictionary order.
    pub(super) fn install_module_namespace(&mut self, class: Id) {
        let mut entries = Vec::new();
        for &name in imre_ast::providers::MODULE_METHODS {
            let key = self.heap.code_name(name);
            entries.push((key, self.builtin(&format!("module.{name}"))));
        }
        let dictionary = self.readonly_member("module", "__dict__");
        entries.push((self.heap.code_name("__dict__"), dictionary));
        for name in ["__annotations__", "__annotate__"] {
            let descriptor = self.alloc(Value::LayoutDescriptor(class, name));
            entries.push((self.heap.code_name(name), descriptor));
        }
        entries.push((self.heap.code_name("__doc__"), self.text(DOC)));
        let members = self.dict(entries);
        self.native_type_dicts.insert(class, members);
    }

    /// Copy live exports from a module in their original order. A failing lookup
    /// leaves earlier bindings visible, just like successive assignments.
    pub(super) fn import_star(&mut self, scope: Id, module: Id) -> Result<Id> {
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(module);
        let (names, implicit) = match self.lookup_attr(module, "__all__") {
            Ok(names) => (names, false),
            Err(error) if self.is_builtin_error(&error, "AttributeError") => {
                // Some intrinsic namespaces synthesize names on first attribute lookup.
                // Their currently materialized dictionary is not their public API.
                if let Value::Module { name, .. } = self.heap.get(module)
                    && super::stdlib::has_lazy_attributes(name)
                {
                    return Err(Error::new(
                        "NotImplementedError",
                        format!("wildcard export enumeration for native module '{name}'"),
                    ));
                }
                let namespace = match self.lookup_attr(module, "__dict__") {
                    Err(error) if self.is_attribute_error(&error) => {
                        return Err(Error::new(
                            "ImportError",
                            "from-import-* object has no __dict__ and no __all__",
                        ));
                    }
                    result => result?,
                };
                let entries = self.dict_entries(namespace).ok_or_else(|| {
                    Error::new("TypeError", "module namespace must be a dictionary")
                })?;
                let keys = entries.iter().map(|&(key, _)| key).collect();
                (self.alloc(Value::List(keys)), true)
            }
            Err(error) => return Err(error),
        };
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(names);
        // Native dictionaries have no sequence item slot. Python subclasses do.
        if let Some(kind @ ("dict" | "collections.defaultdict")) = self.builtin_type_name(names) {
            return Err(Error::new("TypeError", format!("{kind} is not a sequence")));
        }
        // IMPORT_STAR writes directly to the current locals mapping, including when
        // exec has separate globals or the hook supplies a string subclass as a key.
        let locals = match self.scope_data(scope).exec_locals {
            Some(locals) => locals,
            None => self.module_globals(scope)?,
        };
        for index in 0.. {
            let mark = self.heap.temporary_mark();
            let index = self.int(index);
            let key = match self.get_item(names, index) {
                Ok(key) => key,
                Err(error) if self.is_builtin_error(&error, "IndexError") => break,
                Err(error) => return Err(error),
            };
            let name = match self.str_value(key) {
                Ok(name) => super::copy_label(name)?,
                Err(_) => {
                    let module_name = self.lookup_attr(module, "__name__")?;
                    let module_name = self.str_value(module_name)?;
                    let (item, attr) = if implicit {
                        ("Key", "__dict__")
                    } else {
                        ("Item", "__all__")
                    };
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "{item} in {module_name}.{attr} must be str, not {}",
                            self.type_name(key)
                        ),
                    ));
                }
            };
            if implicit && name.starts_with('_') {
                self.heap.release_temporaries_since(mark);
                continue;
            }
            // Attribute hooks can remove this key from __all__ and collect it.
            self.heap.reserve_temporaries(2)?;
            self.heap.keep(key);
            let value = self.lookup_named_attr(module, &name, Some(key))?;
            self.heap.keep(value);
            let replaced = self.replacing_finalizer(scope, &name, value);
            let displaced = self.store_binding_item(locals, key, value)?;
            self.heap.release_temporaries_since(mark);
            if (displaced || replaced) && self.heap.has_unbound_finalizer(NONE) {
                self.finish_binding_release();
            }
        }
        Ok(NONE)
    }

    /// PEP 562 consults the live module dictionary only after ordinary lookup fails.
    pub(super) fn module_get_attribute(
        &mut self,
        module: Id,
        scope: Id,
        name: &str,
        original: Option<Id>,
        report_missing: bool,
    ) -> Result<Id> {
        match self.default_get_attr(module, name) {
            Err(error) if self.is_attribute_error(&error) => {
                let Some(hook) = self.scope_name(scope, "__getattr__") else {
                    // Optional lookups suppress the diagnostic without probing a user spec.
                    if report_missing
                        && let Some(label) = self.scope_name(scope, "__name__")
                        && let Some(text) = self.string_storage(label)
                    {
                        self.heap.keep(label);
                        if let Some(origin) =
                            self.spec_file_origin(self.scope_name(scope, "__spec__"))?
                            && self.possibly_shadowing(origin)?
                            && self.stdlib_name(label)?
                        {
                            let module_name = self.display(text);
                            let path =
                                self.display(self.string_storage(origin).expect("file origin"));
                            return Err(Error::new(
                                "AttributeError",
                                format!(
                                    "module '{module_name}' has no attribute '{name}' (consider renaming '{path}' since it has the same name as the standard library module named '{module_name}' and prevents importing that standard library module)"
                                ),
                            ));
                        }
                    }
                    // ModuleType reports a failed descriptor lookup in the module's namespace.
                    if let Value::Module {
                        constructor: Some(class),
                        ..
                    } = *self.heap.get(module)
                        && self.class_attr(class, name).is_some()
                    {
                        return Err(self.missing_module_attribute(scope, name));
                    }
                    return Err(error);
                };
                let attribute = original.unwrap_or_else(|| self.text(name));
                self.call(hook, vec![attribute], Vec::new())
            }
            result => result,
        }
    }

    /// Module attributes use the live namespace and the concrete module subclass's descriptors.
    pub(super) fn module_instance_attribute(
        &mut self,
        module: Id,
        class: Id,
        scope: Id,
        name: &str,
    ) -> Result<Id> {
        if name == "builtin_module_names" {
            self.publish_builtin_module_names(scope)?;
        }
        let field = self.scope_name(scope, name);
        if field.is_some() || self.class_attr(class, name).is_some() {
            return self.instance_attribute(module, class, field, name);
        }
        match name {
            "__setattr__" | "__str__" => {
                let method = self.builtin(&format!("object.{name}"));
                return self.bind_attribute(method, module, class);
            }
            "__doc__" => return Ok(self.text(DOC)),
            _ => {}
        }
        match self.instance_attribute(module, class, None, name) {
            Err(error) if self.is_attribute_error(&error) => {}
            result => return result,
        }
        Err(self.missing_module_attribute(scope, name))
    }

    fn missing_module_attribute(&self, scope: Id, name: &str) -> Error {
        let prefix = self
            .scope_name(scope, "__name__")
            .and_then(|value| self.str_value(value).ok());
        let kind = prefix.map_or_else(
            || "module".to_owned(),
            |prefix| format!("module '{prefix}'"),
        );
        Error::new(
            "AttributeError",
            format!("{kind} has no attribute '{name}'"),
        )
    }

    pub(super) fn module_dir(&mut self, module: Id) -> Result<Id> {
        if let Value::Module { name, scope, .. } = self.heap.get(module)
            && super::stdlib::has_lazy_attributes(name)
        {
            if let Some(hook) = self.scope_name(*scope, "__dir__") {
                return self.call(hook, vec![], vec![]);
            }
            return Err(Error::new(
                "NotImplementedError",
                "dir() cannot enumerate lazy native module exports",
            ));
        }
        let dict = self.lookup_attr(module, "__dict__")?;
        let storage = self
            .dict_storage(dict)
            .ok_or_else(|| Error::new("TypeError", "<module>.__dict__ is not a dictionary"))?;
        if let Some(hook) = self.dict_named_item(storage, "__dir__") {
            return self.call(hook, vec![], vec![]);
        }
        let keys = self
            .dict_entries(storage)
            .unwrap()
            .iter()
            .map(|&(key, _)| key)
            .collect();
        Ok(self.alloc(Value::List(keys)))
    }

    pub(super) fn module_annotation(&mut self, scope: Id, name: &str) -> Result<Id> {
        if let Some(value) = self.scope_name(scope, name) {
            return Ok(value);
        }
        if name == "__annotate__" {
            self.bind(scope, name, NONE);
            return Ok(NONE);
        }
        let initializing = match self.scope_name(scope, "__spec__") {
            Some(spec) => match self.lookup_attr(spec, "_initializing") {
                Ok(flag) => self.truth(flag)?,
                Err(error) if self.is_attribute_error(&error) => false,
                Err(error) => return Err(error),
            },
            None => false,
        };
        let value = match self.scope_name(scope, "__annotate__") {
            Some(annotate) if self.callable(annotate) => {
                let format = self.int(1);
                let value = self.call(annotate, vec![format], Vec::new())?;
                if self.dict_storage(value).is_none() {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "__annotate__ returned non-dict of type '{}'",
                            self.type_name(value)
                        ),
                    ));
                }
                value
            }
            _ => self.dict(Vec::new()),
        };
        if !initializing {
            self.bind(scope, name, value);
        }
        Ok(value)
    }

    /// Replacing the annotation function invalidates its cached dictionary, and vice versa.
    pub(super) fn update_module_annotations(
        &mut self,
        scope: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        let discard = match (name, value) {
            ("__annotations__", None) if self.scope_name(scope, name).is_none() => {
                return Err(Error::new("AttributeError", "__annotations__"));
            }
            ("__annotations__", _) => "__annotate__",
            ("__annotate__", Some(NONE)) => return Ok(()),
            ("__annotate__", Some(function)) if self.callable(function) => "__annotations__",
            ("__annotate__", Some(_)) => {
                return Err(Error::new(
                    "TypeError",
                    "__annotate__ must be callable or None",
                ));
            }
            ("__annotate__", None) => {
                return Err(Error::new(
                    "TypeError",
                    "cannot delete __annotate__ attribute",
                ));
            }
            _ => return Ok(()),
        };
        let key = self.text(discard);
        self.remove_binding(scope, discard, key)?;
        Ok(())
    }

    /// Walk the effective storage bases past subclasses that add no fields.
    /// ModuleType already provides both the dictionary and weak-reference storage.
    pub(super) fn module_slot_layout(&mut self, mut class: Id) -> Result<(Id, Vec<String>)> {
        let unknown = || Error::new("TypeError", "cannot determine module subclass layout");
        while let Value::Class { name, scope, .. } = self.heap.get(class) {
            let namespace = self
                .scope_data(*scope)
                .class_namespace
                .as_ref()
                .ok_or_else(unknown)?;
            let mut slots: Vec<_> = namespace
                .slot_names
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|slot| !matches!(slot.as_str(), "__dict__" | "__weakref__"))
                .map(|slot| super::slot_member::slot_name(name, slot))
                .collect();
            let bases = self.class_bases_tuple(class).ok_or_else(unknown)?;
            class = self.member_layout_base(&bases)?;
            if !slots.is_empty() {
                slots.sort_unstable();
                return Ok((class, slots));
            }
        }
        Ok((class, Vec::new()))
    }

    /// A new module member follows all named slots in the effective storage base.
    pub(super) fn module_slot_start(&mut self, class: Id) -> Result<usize> {
        let (mut base, _) = self.module_slot_layout(class)?;
        let mut count = 0;
        while matches!(self.heap.get(base), Value::Class { .. }) {
            let (parent, slots) = self.module_slot_layout(base)?;
            count += slots.len();
            base = parent;
        }
        Ok(count)
    }

    /// Allocate an empty module namespace without publishing the module in sys.modules.
    pub(super) fn allocate_module(&mut self, class: Id) -> Id {
        let scope = self.scope("", None);
        self.alloc(Value::Module {
            name: String::new(),
            scope,
            origin: None,
            initializing: false,
            constructor: Some(class),
            module_state: None,
            module_slots: None,
        })
    }

    /// Use the bootstrap's module repr for created modules, honoring their live spec.
    pub(super) fn module_base_repr(&mut self, module: Id) -> Result<Id> {
        self.meta_call(NONE, "_module_repr", vec![module])
    }

    /// Apply ModuleType's native slots, including explicit base initialization by subclasses.
    pub(super) fn module_method(
        &mut self,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                match name {
                    "__new__" => "module.__new__(): not enough arguments".to_owned(),
                    "__dir__" => "unbound method module.__dir__() needs an argument".to_owned(),
                    _ => format!("descriptor '{name}' of 'module' object needs an argument"),
                },
            ));
        };
        if name != "__new__" && !matches!(self.heap.get(receiver), Value::Module { .. }) {
            let kind = self.type_name(receiver);
            let message = if name == "__dir__" {
                format!(
                    "descriptor '{name}' for 'module' objects doesn't apply to a '{kind}' object"
                )
            } else {
                format!("descriptor '{name}' requires a 'module' object but received a '{kind}'")
            };
            return Err(Error::new("TypeError", message));
        }
        if name == "__init__" {
            return self.init_module(receiver, rest, kwargs);
        }
        if matches!(name, "__dir__" | "__repr__" | "__getattribute__") {
            let Value::Module { scope, .. } = *self.heap.get(receiver) else {
                unreachable!("module receiver checked above")
            };
            if !kwargs.is_empty() || rest.len() != usize::from(name == "__getattribute__") {
                return Err(Error::new(
                    "TypeError",
                    format!("invalid arguments to module.{name}"),
                ));
            }
            if name == "__getattribute__" {
                let key = rest[0];
                let attr = builtins::attribute_name(self, key)?;
                return self.module_get_attribute(receiver, scope, &attr, Some(key), true);
            }
            if name == "__repr__" {
                if self.bootstrap_globals != NONE
                    && matches!(
                        self.heap.get(receiver),
                        Value::Module {
                            constructor: Some(_),
                            ..
                        }
                    )
                {
                    return self.module_base_repr(receiver);
                }
                let repr = self.checked_format(receiver, true)?;
                return Ok(self.text(repr));
            }
            return self.module_dir(receiver);
        }
        let class = self.builtin("module");
        if !self.is_type_object(receiver) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "module.__new__(X): X is not a type object ({})",
                    self.type_name(receiver)
                ),
            ));
        }
        if !self.subclass(receiver, class) {
            let actual = self.class_name(receiver);
            return Err(Error::new(
                "TypeError",
                format!("module.__new__({actual}): {actual} is not a subtype of module"),
            ));
        }
        Ok(self.allocate_module(receiver))
    }

    pub(super) fn init_module(
        &mut self,
        object: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Value::Module { scope, .. } = *self.heap.get(object) else {
            return Err(Error::new(
                "TypeError",
                "module.__init__ requires a module object",
            ));
        };
        let [name, doc] =
            builtins::bind_method_arguments("module", args, kwargs, ["name", "doc"], 2)?;
        let name = name.ok_or_else(|| {
            Error::new(
                "TypeError",
                "module() missing required argument 'name' (pos 1)",
            )
        })?;
        if self.str_value(name).is_err() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "module() argument 'name' must be str, not {}",
                    self.type_name(name)
                ),
            ));
        }
        for (key, value) in [
            ("__name__", name),
            ("__doc__", doc.unwrap_or(NONE)),
            ("__package__", NONE),
            ("__loader__", NONE),
            ("__spec__", NONE),
        ] {
            self.bind(scope, key, value);
        }
        Ok(NONE)
    }
}
