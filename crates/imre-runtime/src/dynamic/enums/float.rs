//! Construct float-mixin Enum objects with distinct payload and lookup values.
use super::*;

impl Context {
    /// Select the float data type, following the member type of an inherited Enum.
    pub(in crate::dynamic) fn float_enum_type(&self, bases: &[Id]) -> Option<(Id, Id)> {
        let &first_enum = bases.last()?;
        let base = bases
            .iter()
            .copied()
            .find(|&base| self.has_base(base, "float"))?;
        let data_type = if self.has_base(base, "enum.Enum") {
            self.class_attr(base, "_member_type_")?
        } else {
            base
        };
        Some((data_type, first_enum))
    }

    /// A float Enum member has a native payload and an independently constructed lookup value.
    fn float_enum_member(
        &mut self,
        class: Id,
        data_type: Id,
        new: Option<Id>,
        raw: Id,
        name: Id,
    ) -> Result<(Id, Id)> {
        let args = self.enum_new_args(class, raw);
        let member = if let Some(new) = new {
            self.call(new, copy_ids(&args)?, Vec::new())?
        } else {
            self.new_float(&args, &[])?
        };
        if let Err(error) = self.lookup_attr(member, "_value_") {
            if !self.is_attribute_error(&error) {
                return Err(error);
            }
            let make_value = self
                .call(data_type, copy_ids(&args[1..])?, Vec::new())
                .and_then(|value| self.set_attr(member, "_value_", value));
            if let Err(error) = make_value {
                if !self.is_builtin_error(&error, "Exception") {
                    return Err(error);
                }
                let cause = self.raised_instance(error);
                let mut error = Error::new(
                    "TypeError",
                    "_value_ not set in __new__, unable to create it",
                );
                error.tracing().cause = Some(cause);
                error.context = cause;
                return Err(error);
            }
        }
        let value = self.lookup_attr(member, "_value_")?;
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(value);
        self.set_attr(member, "_name_", name)?;
        self.set_attr(member, "__objclass__", class)?;
        match self.lookup_attr(member, "__init__") {
            Ok(init) => {
                self.call(init, copy_ids(&args[1..])?, Vec::new())?;
            }
            Err(error)
                if self.is_attribute_error(&error)
                    && self.class_attr(class, "__init__").is_none() => {}
            Err(error) => return Err(error),
        }
        Ok((member, value))
    }

    pub(in crate::dynamic) fn float_enum_names(&self, class: Id) -> Option<Id> {
        if self.has_base(class, "float") {
            self.class_attr(class, "_member_names_")
        } else {
            None
        }
    }

    pub(super) fn float_enum_lookup(
        &mut self,
        class: Id,
        values: Id,
        value: Id,
    ) -> Result<Option<Id>> {
        let names = self.lookup_attr(class, "_member_map_")?;
        let unhashable = self.lookup_attr(class, "_unhashable_values_map_")?;
        self.match_float_enum_value(names, values, value, Some(unhashable))
    }

    /// Alias creation compares the resident value first. Public lookup also recognizes each
    /// declaration's original unhashable values and compares the requested value first on fallback.
    fn match_float_enum_value(
        &mut self,
        names: Id,
        values: Id,
        value: Id,
        unhashable: Option<Id>,
    ) -> Result<Option<Id>> {
        match self.find_dictionary_key(values, value, None) {
            Ok((_, found)) => return Ok(found.map(|i| self.stored_entries(values)[i].1)),
            Err(error) if self.is_builtin_error(&error, "KeyError") => return Ok(None),
            Err(error) if self.is_builtin_error(&error, "TypeError") => {}
            Err(error) => return Err(error),
        }
        if let Some(map) = unhashable {
            let keys = self.iterator(map)?;
            while let Some(name) = self.next_item(keys)? {
                let declared = self.get_item(map, name)?;
                if self.contains(value, declared)? {
                    return self.get_item(names, name).map(Some);
                }
            }
        }
        let keys = self.iterator(names)?;
        while let Some(name) = self.next_item(keys)? {
            let member = self.get_item(names, name)?;
            let existing = self.lookup_attr(member, "_value_")?;
            let equal = if unhashable.is_some() {
                self.binary("==", value, existing)?
            } else {
                self.binary("==", existing, value)?
            };
            if self.truth(equal)? {
                return Ok(Some(member));
            }
        }
        Ok(None)
    }

    pub(super) fn populate_float_enum(
        &mut self,
        class: Id,
        scope: Id,
        data_type: Id,
        names: Id,
        values: Id,
        declarations: Vec<(String, Id)>,
    ) -> Result<()> {
        let new = self.class_attr(class, "__new__");
        let ordered = self.alloc(Value::List(Vec::new()));
        self.bind(scope, "_member_names_", ordered);
        let hashable = self.alloc(Value::List(Vec::new()));
        self.bind(scope, "_hashable_values_", hashable);
        let unhashable = self.alloc(Value::List(Vec::new()));
        self.bind(scope, "_unhashable_values_", unhashable);
        let unhashable_map = self.dict(Vec::new());
        self.bind(scope, "_unhashable_values_map_", unhashable_map);
        let append = self.builtin("list.append");
        self.heap.reserve_temporaries(declarations.len())?;
        for &(_, raw) in &declarations {
            self.heap.keep(raw);
        }
        for (name, raw) in declarations {
            let mark = self.heap.temporary_mark();
            let key = self.text(&name);
            self.remove_binding(scope, &name, key)?;
            let (member, value) = self.float_enum_member(class, data_type, new, raw, key)?;
            let Value::List(declared) = self.heap.get(ordered) else {
                unreachable!()
            };
            let position = self.alloc(Value::Int((declared.len() as i64).into()));
            self.set_attr(member, "_sort_order_", position)?;
            let existing = self.match_float_enum_value(names, values, value, None)?;
            if existing.is_none() {
                self.call(append, vec![ordered, key], Vec::new())?;
            }
            let canonical = existing.unwrap_or(member);
            self.put_item(names, key, canonical)?;
            self.bind(scope, &name, canonical);
            let registered = self
                .set_dictionary_key(values, value, canonical, None, false)
                .and_then(|_| {
                    if !self.contains(value, hashable)? {
                        self.call(append, vec![hashable, value], Vec::new())?;
                    }
                    Ok(())
                });
            if let Err(error) = registered {
                if !self.is_builtin_error(&error, "TypeError") {
                    return Err(error);
                }
                self.call(append, vec![unhashable, value], Vec::new())?;
                let empty = self.alloc(Value::List(Vec::new()));
                let declared = self.set_dictionary_key(unhashable_map, key, empty, None, false)?;
                self.call(append, vec![declared, value], Vec::new())?;
            }
            self.heap.release_temporaries_since(mark);
            if member != canonical {
                self.drain_acyclic_finalizers()?;
            }
        }
        Ok(())
    }
}
