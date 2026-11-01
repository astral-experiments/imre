//! Runtime class factories and inherited metaclass initialization.

use super::*;

impl Context {
    pub(super) fn custom_metaclass(&self, class: Id) -> Option<Id> {
        let Value::Class { scope, .. } = self.heap.get(class) else {
            return None;
        };
        self.scope_data(*scope).class_namespace.as_ref()?.metaclass
    }

    /// A subclass inherits the most derived metaclass of its direct bases.
    pub(super) fn select_metaclass(
        &self,
        mut winner: Option<Id>,
        bases: &[Id],
    ) -> Result<Option<Id>> {
        for meta in bases.iter().filter_map(|&base| self.custom_metaclass(base)) {
            match winner {
                Some(previous) if self.direct_subclass(previous, meta) => {}
                Some(previous) if !self.direct_subclass(meta, previous) => {
                    return Err(Error::new(
                        "TypeError",
                        "metaclass conflict: the metaclass of a derived class must be a (non-strict) subclass of the metaclasses of all its bases",
                    ));
                }
                _ => winner = Some(meta),
            }
        }
        if let Some(meta) = winner {
            for hook in ["__prepare__", "__new__", "__call__"] {
                if self.class_attr(meta, hook).is_some() {
                    return Err(Error::new(
                        "NotImplementedError",
                        format!("custom metaclass {hook} is unsupported"),
                    ));
                }
            }
        }
        Ok(winner)
    }

    pub(super) fn construct_type(
        &mut self,
        requested: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let &[name, bases, namespace] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type.__new__() takes exactly 3 arguments ({} given)",
                    args.len()
                ),
            ));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "NotImplementedError",
                "class keyword arguments are unsupported",
            ));
        }
        fn bad_argument(index: u8, expected: &str, actual: &str) -> Error {
            Error::new(
                "TypeError",
                format!("type.__new__() argument {index} must be {expected}, not {actual}"),
            )
        }
        let class_name = self
            .str_value(name)
            .map_err(|_| bad_argument(1, "str", self.type_name(name)))?
            .to_owned();
        if class_name.contains('\0') {
            return Err(Error::new(
                "ValueError",
                "type name must not contain null characters",
            ));
        }
        let Value::Tuple(parents) = self.heap.get(self.scalar_value(bases)) else {
            return Err(bad_argument(2, "tuple", self.type_name(bases)));
        };
        let parents = parents.clone();
        for &parent in &parents {
            if self.builtin_type_name(parent) != Some("type") {
                return Err(Error::new("TypeError", "bases must be types"));
            }
        }
        let builtin = self.builtin("type");
        let meta = self.select_metaclass((requested != builtin).then_some(requested), &parents)?;
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| bad_argument(3, "dict", self.type_name(namespace)))?
            .to_vec();
        let globals = self.root_scope(self.heap.current_scope());
        let module = self.scope_data(globals).module.clone();
        let scope = self.scope(&module, Some(globals));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_body = Some(Vec::new());
        for (key, value) in entries {
            let key = self
                .str_value(key)
                .map_err(|_| {
                    Error::new(
                        "NotImplementedError",
                        "non-string class namespace keys are unsupported",
                    )
                })?
                .to_owned();
            if key == "__qualname__" && self.string_storage(value).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "type __qualname__ must be a str, not {}",
                        self.type_name(value)
                    ),
                ));
            }
            if key == "__classcell__" {
                return Err(Error::new(
                    "NotImplementedError",
                    "explicit class cells in type() are unsupported",
                ));
            }
            self.bind_local(scope, &key, value);
        }
        if self.scope_name(scope, "__module__").is_none() {
            let name = self.function_module(globals);
            self.bind_local(scope, "__module__", name);
        }
        if self.scope_name(scope, "__qualname__").is_none() {
            self.bind_local(scope, "__qualname__", name);
        }
        let class = self.finish_class(&class_name, scope, parents, meta)?;
        if let Some(meta) = meta {
            self.run_metaclass_init(meta, class, [name, bases, namespace])?;
        }
        Ok(class)
    }

    pub(super) fn run_metaclass_init(&mut self, meta: Id, class: Id, args: [Id; 3]) -> Result<()> {
        if let Some(init) = self.class_attr(meta, "__init__") {
            let method = self.bind_attribute(init, class, meta)?;
            let result = self.call(method, args.to_vec(), Vec::new())?;
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
        Ok(())
    }
}
