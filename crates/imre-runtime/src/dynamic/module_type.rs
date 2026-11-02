use super::{Context, Error, Id, NONE, Result, Value, builtins};

pub(super) const DOC: &str = "Create a module object.\n\nThe name must be a string; the optional doc argument can have any type.";

impl Context {
    /// Copy live exports from a module in their original order. A failing lookup
    /// leaves earlier bindings visible, just like successive assignments.
    pub(super) fn import_star(&mut self, scope: Id, module: Id, custom_hook: bool) -> Result<Id> {
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(module);
        let (names, implicit) = match self.lookup_attr(module, "__all__") {
            Ok(names) => {
                // Custom import hooks have already handled the fromlist. The native importer
                // does not yet preload package children named by __all__.
                if !custom_hook
                    && let Value::Module { scope, .. } = *self.heap.get(module)
                    && self.scope_name(scope, "__path__").is_some()
                {
                    return Err(Error::new(
                        "NotImplementedError",
                        "wildcard imports from package __all__",
                    ));
                }
                (names, false)
            }
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
            self.put_item(locals, key, value)?;
            self.heap.release_temporaries_since(mark);
            if replaced && self.heap.has_unbound_finalizer(NONE) {
                self.collect_unreachable();
                self.heap.release_unbound_finalizers();
            }
        }
        Ok(NONE)
    }

    /// Module attributes use the live namespace and the concrete module subclass's descriptors.
    pub(super) fn module_instance_attribute(
        &mut self,
        module: Id,
        class: Id,
        scope: Id,
        name: &str,
    ) -> Result<Id> {
        let field = self.scope_name(scope, name);
        if field.is_some() || self.class_attr(class, name).is_some() {
            return self.instance_attribute(module, class, field, name);
        }
        match name {
            "__init__" => {
                let init = self.builtin("module.__init__");
                return self.bind_attribute(init, module, class);
            }
            "__doc__" => return Ok(self.text(DOC)),
            "__annotate__" => {
                self.bind(scope, name, NONE);
                return Ok(NONE);
            }
            "__annotations__" => {
                let value = match self.scope_name(scope, "__annotate__") {
                    Some(annotate) if annotate != NONE => {
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
                self.bind(scope, name, value);
                return Ok(value);
            }
            _ => {}
        }
        if let Some(hook) = self.scope_name(scope, "__getattr__") {
            let attr = self.text(name);
            return self.call(hook, vec![attr], Vec::new());
        }
        if self.class_attr(class, "__getattr__").is_some() {
            return self.instance_attribute(module, class, None, name);
        }
        let prefix = self
            .scope_name(scope, "__name__")
            .and_then(|value| self.str_value(value).ok());
        let kind = prefix.map_or_else(
            || "module".to_owned(),
            |prefix| format!("module '{prefix}'"),
        );
        Err(Error::new(
            "AttributeError",
            format!("{kind} has no attribute '{name}'"),
        ))
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

    /// Allocate an empty module namespace without publishing the module in sys.modules.
    pub(super) fn allocate_module(&mut self, class: Id) -> Id {
        let scope = self.scope("", None);
        self.builtin("module.__init__");
        self.alloc(Value::Module {
            name: String::new(),
            scope,
            origin: None,
            initializing: false,
            constructor: Some(class),
        })
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
                format!("module.{name} requires a receiver"),
            ));
        };
        if name == "__init__" {
            return self.init_module(receiver, rest, kwargs);
        }
        let class = self.builtin("module");
        if matches!(name, "__new__") && self.subclass(receiver, class) {
            Ok(self.allocate_module(receiver))
        } else {
            Err(Error::new(
                "TypeError",
                "module.__new__ requires a module subtype",
            ))
        }
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
