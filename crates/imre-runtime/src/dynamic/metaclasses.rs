//! Metaclass selection and the namespace passed through Python's type constructor.

use super::{Context, Error, Id, NONE, Result, Value};

impl Context {
    pub(super) fn stored_class_mro(&self, class: Id) -> Option<&[Id]> {
        let Value::Class { scope, .. } = self.heap.get(class) else {
            return None;
        };
        let namespace = self.scope_data(*scope).class_namespace.as_ref()?;
        match self.heap.get(namespace.mro) {
            Value::Tuple(order) => Some(order),
            _ => None,
        }
    }

    /// `type.mro(cls)` computes the default C3 order, including when a metaclass overrides it.
    pub(super) fn default_type_mro(&mut self, class: Id) -> Result<Vec<Id>> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            return Ok(self.lineage(class));
        };
        let namespace = self.scope_data(scope).class_namespace.as_ref().unwrap();
        let Value::Tuple(bases) = self.heap.get(namespace.bases) else {
            unreachable!()
        };
        let bases = bases.clone();
        let mut order = vec![class];
        order.extend(self.linearized_bases(&bases)?);
        Ok(order)
    }

    /// The closest base that contributes native storage or slots constrains a custom MRO.
    fn mro_layout(&self, class: Id) -> Id {
        self.find_in_mro(class, |base| match self.heap.get(base) {
            Value::Native(_) => Some(base),
            Value::Class { scope, .. } => self
                .scope_data(*scope)
                .class_namespace
                .as_ref()
                .and_then(|ns| ns.slot_names.as_deref())
                .is_some_and(|slots| {
                    slots
                        .iter()
                        .any(|s| !matches!(s.as_str(), "__dict__" | "__weakref__"))
                })
                .then_some(base),
            _ => None,
        })
        .unwrap_or(class)
    }

    fn apply_metaclass_mro(&mut self, class: Id, scope: Id, meta: Id) -> Result<()> {
        let default = self.builtin("type.mro");
        let Some(slot) = self.class_attr(meta, "mro").filter(|&slot| slot != default) else {
            return Ok(());
        };
        self.heap.keep(class);
        let layout = self.mro_layout(class);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_namespace.as_mut().unwrap().mro = NONE;
        let method = self.bind_attribute(slot, class, meta)?;
        let result = self.call(method, Vec::new(), Vec::new())?;
        let order = self.items(result)?;
        if order.is_empty() {
            return Err(Error::new("TypeError", "type MRO must not be empty"));
        }
        for &base in &order {
            if self.builtin_type_name(base) != Some("type") {
                return Err(Error::new(
                    "TypeError",
                    format!("mro() returned a non-class ('{}')", self.type_name(base)),
                ));
            }
            if !self.direct_subclass(layout, self.mro_layout(base)) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "mro() returned base with unsuitable layout ('{}')",
                        self.class_name(base)
                    ),
                ));
            }
        }
        self.set_class_mro(class, order);
        Ok(())
    }

    /// Keep Python's visible MRO and the bases used by native slot lookup in sync.
    pub(super) fn set_class_mro(&mut self, class: Id, order: Vec<Id>) {
        let Value::Class {
            ancestors, scope, ..
        } = self.heap.get_mut(class)
        else {
            unreachable!()
        };
        *ancestors = order
            .iter()
            .copied()
            .filter(|&base| base != class)
            .collect();
        let scope = *scope;
        let mro = self.alloc(Value::Tuple(order));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_namespace.as_mut().unwrap().mro = mro;
    }

    pub(in crate::dynamic) fn class_metaclass(&self, value: Id) -> Option<Id> {
        match self.heap.get(value) {
            Value::Class { scope, .. } => self.scope_data(*scope).metaclass,
            _ => None,
        }
    }

    /// `type.__new__` consumes these cells from the copied class dictionary.
    pub(super) fn complete_type_cells(&mut self, class: Id, scope: Id) -> Result<()> {
        for name in ["__classcell__", "__classdictcell__"] {
            let Some(cell) = self.scope_name(scope, name) else {
                continue;
            };
            if !matches!(self.heap.get(cell), Value::Cell(_)) {
                let ty = self.exact_type(cell)?;
                return Err(Error::new(
                    "TypeError",
                    format!("{name} must be a nonlocal cell, not {}", self.repr(ty)),
                ));
            }
            let key = self.text(name);
            self.remove_binding(scope, name, key)?;
            let contents = if name == "__classcell__" {
                class
            } else {
                self.class_dictionary(scope)
            };
            self.heap.replace_cell(cell, Some(contents));
        }
        Ok(())
    }

    /// The metaclass must propagate the defining class cell supplied by the class body.
    pub(super) fn check_class_cell(&self, cell: Id, class: Id, name: &str) -> Result<()> {
        match self.heap.get(cell) {
            Value::Cell(Some(contents)) if *contents == class => Ok(()),
            Value::Cell(Some(contents)) => Err(Error::new(
                "TypeError",
                format!(
                    "__class__ set to {} defining '{name}' as {}",
                    self.repr(*contents),
                    self.repr(class)
                ),
            )),
            _ => Err(Error::new(
                "RuntimeError",
                format!(
                    "__class__ not set defining '{name}' as {}. Was __classcell__ propagated to type.__new__?",
                    self.repr(class)
                ),
            )),
        }
    }

    /// Forward class keywords to the first parent hook after descriptor initialization.
    pub(super) fn init_subclass(&mut self, class: Id, keywords: &[(String, Id)]) -> Result<()> {
        let order = self.lineage(class);
        let Some(start) = order.iter().position(|&base| base == class) else {
            let name = self.class_name(class);
            return Err(Error::new(
                "TypeError",
                format!(
                    "super(type, obj): obj (type {name}) is not an instance or subtype of type ({name})."
                ),
            ));
        };
        for &base in &order[start + 1..] {
            if let Some(hook) = self.own_class_attr(base, "__init_subclass__") {
                let bound = self.bind_attribute(hook, NONE, class)?;
                self.call(bound, Vec::new(), keywords.to_vec())?;
                return Ok(());
            }
            if matches!(self.heap.get(base), Value::Native(name) if name == "object") {
                if keywords.is_empty() {
                    return Ok(());
                }
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{}.__init_subclass__() takes no keyword arguments",
                        self.class_name(class)
                    ),
                ));
            }
        }
        // A custom MRO may omit object. The super object then supplies its own inherited hook.
        if keywords.is_empty() {
            Ok(())
        } else {
            Err(Error::new(
                "TypeError",
                "super.__init_subclass__() takes no keyword arguments",
            ))
        }
    }

    /// Store compiler-created metadata locally even when the class body declares a nonlocal.
    pub(super) fn bind_class_local(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        if let Some(namespace) = self.scope_data(scope).namespace {
            let key = self.text(name);
            return self.put_item(namespace, key, value);
        }
        self.bind_local(scope, name, value);
        Ok(())
    }

    /// Class bodies observe a prepared mapping's item access, including its exceptions.
    pub(super) fn load_class_local(&mut self, scope: Id, name: &str) -> Result<Option<Id>> {
        let frame = self.scope_data(scope);
        if frame.class_body.is_some()
            && let Some(namespace) = frame.namespace
            && !matches!(self.heap.get(namespace), Value::Dict(_))
        {
            return match self.load_exec_name(namespace, name) {
                Ok(value) => Ok(Some(value)),
                Err(error) if self.is_builtin_error(&error, "KeyError") => Ok(None),
                Err(error) => Err(error),
            };
        }
        Ok(self.scope_name(scope, name))
    }

    /// Choose the most derived metaclass compatible with every direct base.
    pub(super) fn select_metaclass(&mut self, explicit: Option<Id>, bases: &[Id]) -> Result<Id> {
        let default = self.builtin("type");
        let mut winner = explicit.unwrap_or(default);
        if !self.direct_subclass(winner, default) {
            return Ok(winner);
        }
        for &base in bases {
            // Native bases such as Enum use their existing class-body provider. Only a
            // Python-defined base can carry a custom metaclass in its class scope.
            let Some(candidate) = self.class_metaclass(base) else {
                continue;
            };
            if self.direct_subclass(winner, candidate) {
                continue;
            }
            if !self.direct_subclass(candidate, winner) {
                return Err(Error::new(
                    "TypeError",
                    "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
                ));
            }
            winner = candidate;
        }
        Ok(winner)
    }

    pub(super) fn prepare_type_namespace(
        &mut self,
        meta: Id,
        name: Id,
        bases: Id,
        keywords: &[(String, Id)],
    ) -> Result<Id> {
        self.builtin("type.__prepare__");
        let prepare = match self.lookup_attr(meta, "__prepare__") {
            Ok(prepare) => prepare,
            Err(error) if error.kind == "AttributeError" => return Ok(self.dict(Vec::new())),
            Err(error) => return Err(error),
        };
        let namespace = self.call(prepare, vec![name, bases], keywords.to_vec())?;
        if !self.supports_mapping_items(namespace) {
            let owner = if self.builtin_type_name(meta) == Some("type") {
                self.class_name(meta)
            } else {
                "<metaclass>"
            };
            return Err(Error::new(
                "TypeError",
                format!(
                    "{owner}.__prepare__() must return a mapping, not {}",
                    self.type_name(namespace)
                ),
            ));
        }
        Ok(namespace)
    }

    /// A Python metaclass can return another object or delegate creation to `type.__new__`.
    pub(super) fn construct_type(
        &mut self,
        meta: Id,
        args: Vec<Id>,
        keywords: Vec<(String, Id)>,
    ) -> Result<Id> {
        let class = if let Some(new) = self.class_attr(meta, "__new__") {
            self.call_type_new(new, meta, &args, keywords.clone())?
        } else {
            let [name, bases, namespace] = args[..] else {
                return Err(Error::new("TypeError", "type() takes 1 or 3 arguments"));
            };
            self.new_type(meta, name, bases, namespace, &keywords)?
        };
        if let Some(actual) = self.class_metaclass(class)
            && self.direct_subclass(actual, meta)
            && let Some(init) = self.class_attr(actual, "__init__")
        {
            let method = self.bind_attribute(init, class, actual)?;
            let result = self.call(method, args, keywords)?;
            if result != NONE {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__init__() should return None, not '{}'",
                        self.type_name(result)
                    ),
                ));
            }
        }
        Ok(class)
    }

    fn call_type_new(
        &mut self,
        new: Id,
        meta: Id,
        args: &[Id],
        keywords: Vec<(String, Id)>,
    ) -> Result<Id> {
        let method = self.bind_attribute(new, NONE, meta)?;
        let mut arguments = vec![meta];
        arguments.extend_from_slice(args);
        self.call(method, arguments, keywords)
    }

    pub(super) fn new_type(
        &mut self,
        meta: Id,
        name: Id,
        bases: Id,
        namespace: Id,
        keywords: &[(String, Id)],
    ) -> Result<Id> {
        let type_class = self.builtin("type");
        if !self.direct_subclass(meta, type_class) {
            let message = match self.heap.get(meta) {
                Value::Class { name, .. } | Value::Native(name)
                    if self.builtin_type_name(meta) == Some("type") =>
                {
                    format!("type.__new__({name}): {name} is not a subtype of type")
                }
                _ => format!(
                    "type.__new__(X): X is not a type object ({})",
                    self.type_name(meta)
                ),
            };
            return Err(Error::new("TypeError", message));
        }
        let name_text = self
            .str_value(name)
            .map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "type.__new__() argument 1 must be str, not {}",
                        self.type_name(name)
                    ),
                )
            })?
            .to_owned();
        let Value::Tuple(bases) = self.heap.get(bases) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type.__new__() argument 2 must be tuple, not {}",
                    self.type_name(bases)
                ),
            ));
        };
        let bases = bases.clone();
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "type.__new__() argument 3 must be dict, not {}",
                        self.type_name(namespace)
                    ),
                )
            })?
            .to_vec();
        let winner = self.select_metaclass(Some(meta), &bases)?;
        if winner != meta
            && let Some(new) = self.class_attr(winner, "__new__")
        {
            let bases = self.alloc(Value::Tuple(bases));
            return self.call_type_new(new, winner, &[name, bases, namespace], keywords.to_vec());
        }
        let name = name_text;
        let caller = self.heap.current_scope();
        let module = match self.heap.get(caller) {
            Value::Scope(frame) => frame.module.clone(),
            _ => "__main__".to_owned(),
        };
        let scope = self.scope(&module, None);
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        fields.class_body = Some(Vec::new());
        fields.metaclass = (winner != type_class).then_some(winner);
        for (key, value) in entries {
            let field = self.str_value(key)?.to_owned();
            self.bind(scope, &field, value);
        }
        if self.scope_name(scope, "__module__").is_none()
            && matches!(self.heap.get(caller), Value::Scope(_))
        {
            let globals = self.module_globals(caller);
            if let Some(module) = self.dict_named_item(globals, "__name__") {
                self.bind(scope, "__module__", module);
            }
        }
        let kind = self.enum_namespace(&bases).map(|ns| ns.kind);
        let class = self.allocate_class(name.clone(), scope, bases)?;
        self.complete_type_cells(class, scope)?;
        self.apply_metaclass_mro(class, scope, winner)?;
        self.finish_class(class, name, scope, kind, keywords)
    }
}
