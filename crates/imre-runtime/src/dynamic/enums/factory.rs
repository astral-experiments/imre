//! The functional Enum API creates a class with the same member namespace as a declaration.
use super::*;

impl Context {
    pub(in crate::dynamic) fn functional_enum(
        &mut self,
        base: Id,
        mut args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        let caller = self.heap.current_scope();
        let caller_module = self.scope_data(caller).module.clone();
        let prepare = self
            .module_value("enum", "_prepare_functional_enum")
            .ok_or_else(|| Error::new("RuntimeError", "enum factory is unavailable"))?;
        args.insert(0, base);
        let prepared = self.call(prepare, args, kwargs)?;
        self.heap.keep(prepared);
        let [name, members, module, qualname, boundary]: [Id; 5] = self
            .items(prepared)?
            .as_slice()
            .try_into()
            .map_err(|_| Error::new("TypeError", "invalid enum factory arguments"))?;
        let class_name = self.str_value(name)?.to_owned();
        if class_name.contains('\0') {
            return Err(Error::new(
                "ValueError",
                "type name must not contain null characters",
            ));
        }
        let module = if module == NONE {
            let globals = self.module_globals(caller)?;
            self.dict_named_item(globals, "__name__").unwrap_or(NONE)
        } else {
            module
        };
        let namespace = self.enum_namespace(&[base])?.expect("native enum base");
        let scalar = namespace.kind.scalar_base().map(|base| self.builtin(base));
        let scope = self.scope(&caller_module, None);
        self.heap.keep(scope);
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        fields.enum_namespace = Some(namespace);
        fields.in_class_body = true;
        self.bind(scope, "__qualname__", name);
        self.bind(scope, "__doc__", NONE);
        let iter = self.iterator(members)?;
        self.heap.keep(iter);
        while let Some(item) = self.next_item(iter)? {
            let [key, raw]: [Id; 2] = self
                .items(item)?
                .as_slice()
                .try_into()
                .map_err(|_| Error::new("TypeError", "expected an enum name and value"))?;
            let key = self.str_value(key)?.to_owned();
            if matches!(key.as_str(), "mro" | "") {
                return Err(Error::new(
                    "ValueError",
                    format!("invalid enum member name(s) '{key}'"),
                ));
            }
            let value = self.enum_assignment(scope, &key, raw)?;
            self.bind(scope, &key, value);
        }
        self.bind(scope, "__module__", module);
        self.bind_enum_boundary(scope, &[base], boundary)?;
        if qualname != NONE {
            self.bind(scope, "__qualname__", qualname);
        }
        self.str_value(self.scope_name(scope, "__qualname__").unwrap())?;
        let class = self.allocate_enum_class(class_name, scope, base)?;
        self.populate_enum(class, scalar)?;
        Ok(class)
    }

    /// Snapshot the controlling enum base's policy before descriptors and members see the class.
    pub(in crate::dynamic) fn bind_enum_boundary(
        &mut self,
        scope: Id,
        bases: &[Id],
        boundary: Id,
    ) -> Result<()> {
        let explicit = self.truth(boundary)?;
        let flag = self.builtin("enum.Flag");
        if bases.iter().any(|&base| self.direct_subclass(base, flag)) {
            let policy = if explicit {
                boundary
            } else {
                let base = *bases.last().expect("enum has a base");
                self.optional_attr(base, "_boundary_")?.unwrap_or(NONE)
            };
            self.bind(scope, "_boundary_", policy);
        }
        Ok(())
    }

    pub(super) fn allocate_enum_class(
        &mut self,
        class_name: String,
        scope: Id,
        base: Id,
    ) -> Result<Id> {
        let class = self.allocate_class(class_name, scope, vec![base])?;
        self.heap.keep(class);
        self.install_instance_dictionary_descriptor(class, scope, base);
        self.install_weakref_descriptor(class, scope);
        self.initialize_class_members(class, scope, None)?;
        Ok(class)
    }
}
