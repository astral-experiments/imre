//! Construct heap types from a supplied namespace.

use super::*;

impl Context {
    pub(super) fn type_from_namespace(
        &mut self,
        meta: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
        direct_new: bool,
    ) -> Result<Id> {
        let type_class = self.builtin("type");
        if !self.is_type_object(meta) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type.__new__(X): X is not a type object ({})",
                    self.type_name(meta)
                ),
            ));
        }
        if !self.direct_subclass(meta, type_class) {
            let name = self.class_qualname(meta);
            return Err(Error::new(
                "TypeError",
                format!("type.__new__({name}): {name} is not a subtype of type"),
            ));
        }
        let &[name, bases, namespace] = args else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type.__new__() takes exactly 3 arguments ({} given)",
                    args.len()
                ),
            ));
        };
        let argument = |n, wanted, actual: &str| {
            Error::new(
                "TypeError",
                format!("type.__new__() argument {n} must be {wanted}, not {actual}"),
            )
        };
        if self.string_storage(name).is_none() {
            return Err(argument(1, "str", self.type_name(name)));
        }
        let label = self.str_value(name)?.to_owned();
        let bases = self
            .tuple_items(bases)?
            .ok_or_else(|| argument(2, "tuple", self.type_name(bases)))?;
        let entries = self
            .dict_entries(namespace)
            .ok_or_else(|| argument(3, "dict", self.type_name(namespace)))?
            .to_vec();
        for &base in &bases {
            if !self.is_type_object(base) {
                match self.lookup_attr(base, "__mro_entries__") {
                    Ok(_) => {
                        return Err(Error::new(
                            "TypeError",
                            "type() doesn't support MRO entry resolution; use types.new_class()",
                        ));
                    }
                    Err(error) if self.is_attribute_error(&error) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let winner = self.select_class_metaclass(&bases, Some(meta), None, false, direct_new)?;
        if label.contains('\0') {
            return Err(Error::new(
                "ValueError",
                "type name must not contain null characters",
            ));
        }
        let parent = self.heap.current_scope();
        let module = self.scope_data(parent).module.clone();
        let scope = self.scope(&module, None);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_body = Some(Vec::new());
        let mut cell = None;
        let mut annotation_cell = None;
        let body = entries.iter().any(|&(key, _)| self.string_storage(key).is_none())
            .then(|| self.dict(entries.clone()));
        for (key, value) in entries {
            if self.string_storage(key).is_none() {
                continue;
            }
            let attr = self.str_value(key)?.to_owned();
            match attr.as_str() {
                "__classcell__" | "__classdictcell__" => {
                    if !matches!(self.heap.get(value), Value::Cell(_)) {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "{attr} must be a nonlocal cell, not <class '{}'>",
                                self.type_name(value)
                            ),
                        ));
                    }
                    if attr == "__classcell__" {
                        cell = Some(value);
                    } else {
                        annotation_cell = Some(value);
                    }
                }
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
                    let staticmethod = self.builtin("staticmethod");
                    let wrapped = self.call(staticmethod, vec![value], Vec::new())?;
                    self.bind_local(scope, &attr, wrapped);
                }
                _ => self.bind_local(scope, &attr, value),
            }
        }
        if self.scope_name(scope, "__module__").is_none() {
            let globals = self.module_globals(parent)?;
            if let Some(module_name) = self.dict_named_item(globals, "__name__") {
                self.bind_local(scope, "__module__", module_name);
            }
        }
        if self.scope_name(scope, "__qualname__").is_none() {
            self.bind_local(scope, "__qualname__", name);
        }
        let cell = cell.unwrap_or_else(|| self.alloc(Value::Cell(None)));
        let Value::Scope(frame) = self.heap.get_mut(scope) else { unreachable!() };
        frame.class_cell = Some(cell);
        let object = self.builtin("object");
        let slots = bases
            .iter()
            .all(|&base| base == object || self.allocation_base(base) == Some(object));
        let class = self.allocate_class_with_slots(label.clone(), scope, bases, slots)?;
        let Value::Class { metaclass, .. } = self.heap.get_mut(class) else {
            unreachable!()
        };
        *metaclass = winner;
        let exposed = (body.is_some() || annotation_cell.is_some()).then_some(ExposedClassBody {
            mapping: body.unwrap_or(namespace),
            annotation_cell,
        });
        self.install_class_attributes(class, scope, &label, cell, exposed)?;
        let hook = self.super_attribute(class, class, class, "__init_subclass__")?;
        self.call(hook, Vec::new(), kwargs.to_vec())?;
        // ABCMeta.__new__ calls type.__new__ and only then collects abstract methods.
        if !direct_new || winner != Some(meta) {
            self.init_abstract_class(class, scope)?;
        }
        Ok(class)
    }
}
