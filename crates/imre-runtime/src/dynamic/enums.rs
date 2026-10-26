//! Build enum members from source declarations and run scalar members' original constructors.

use super::*;

impl Context {
    pub(super) fn scalar_value(&self, id: Id) -> Id {
        match self.heap.get(id) {
            Value::Instance {
                scalar: Some(value),
                ..
            } => *value,
            _ => id,
        }
    }

    pub(super) fn enum_values(&self, class: Id) -> Option<Id> {
        let Value::Class {
            scope, ancestors, ..
        } = self.heap.get(class)
        else {
            return None;
        };
        let values = self.scope_data(*scope).names.get("_value2member_map_")?;
        if !matches!(self.heap.get(*values), Value::Dict(_)) {
            return None;
        }
        ancestors
            .iter()
            .any(|&base| matches!(self.heap.get(base), Value::Native(name) if name == "enum.Enum"))
            .then_some(*values)
    }

    fn scalar_enum_base(&self, base: Id) -> bool {
        matches!(self.heap.get(base), Value::Native(name) if matches!(name.as_str(), "enum.StrEnum" | "enum.IntEnum"))
    }

    pub(super) fn new_int(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&class, values)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "int.__new__(): not enough arguments",
            ));
        };
        let base = self.builtin("int");
        if !self.subclass(class, base) {
            return Err(Error::new(
                "TypeError",
                "int.__new__ requires an int subtype",
            ));
        }
        let value = builtins::call(self, "int", values, kwargs)?;
        if class == base {
            return Ok(value);
        }
        Ok(self.alloc(Value::Instance {
            class,
            fields: HashMap::new(),
            scalar: Some(value),
            dictionary: None,
            text_memory: None,
        }))
    }

    pub(super) fn enum_lookup(
        &self,
        class: Id,
        values: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Value::Class { name, .. } = self.heap.get(class) else {
            unreachable!()
        };
        let [value] = args else {
            return Err(Error::new("TypeError", format!("{name} requires a value")));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("unexpected keyword argument to {name}"),
            ));
        }
        if matches!(self.heap.get(*value), Value::Instance { class: actual, .. } if *actual == class)
        {
            return Ok(*value);
        }
        self.member_for_value(values, *value).ok_or_else(|| {
            Error::new(
                "ValueError",
                format!("{} is not a valid {name}", self.repr(*value)),
            )
        })
    }

    fn member_for_value(&self, values: Id, value: Id) -> Option<Id> {
        let Value::Dict(entries) = self.heap.get(values) else {
            unreachable!()
        };
        entries
            .iter()
            .find(|(k, _)| self.equal(*k, value))
            .map(|&(_, member)| member)
    }

    pub(super) fn enum_decorator(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let [base] = args else {
            return Err(Error::new(
                "TypeError",
                "_simple_enum requires an enum type",
            ));
        };
        if !kwargs.is_empty() || !self.scalar_enum_base(*base) {
            return Err(Error::new(
                "TypeError",
                "_simple_enum requires a scalar enum type",
            ));
        }
        let callable = self.builtin("enum._convert");
        Ok(self.alloc(Value::Method {
            callable,
            receiver: *base,
        }))
    }

    /// Execute each member's source constructor. Alias names share the first object for that value.
    pub(super) fn decorate_enum(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let [base, class] = args else {
            return Err(Error::new("TypeError", "enum decorator requires a class"));
        };
        if !kwargs.is_empty() {
            return Err(Error::new("TypeError", "unexpected enum decorator keyword"));
        }
        let lineage = self.linearized_bases(&[*base])?;
        let Value::Class { ancestors, .. } = self.heap.get_mut(*class) else {
            return Err(Error::new("TypeError", "enum decorator requires a class"));
        };
        *ancestors = lineage;
        self.populate_enum(*class, Some(*base))?;
        Ok(*class)
    }

    /// Index the first declared member for each value, retaining aliases in the name map.
    pub(super) fn populate_enum(&mut self, class: Id, base: Option<Id>) -> Result<()> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            unreachable!()
        };
        let declarations = self.scope_data(scope).names.clone();
        let names = self.dict(Vec::new());
        let values = self.dict(Vec::new());
        self.bind(scope, "_member_map_", names);
        self.bind(scope, "_value2member_map_", values);
        let proxy = self.alloc(Value::MappingProxy(names));
        self.bind(scope, "__members__", proxy);
        let new = self.class_attr(class, "__new__");
        for (name, raw) in declarations {
            if name.starts_with('_') {
                continue;
            }
            if matches!(
                self.heap.get(raw),
                Value::Property(_) | Value::Function { .. } | Value::StaticMethod { .. }
            ) {
                continue;
            }
            let (member, value) = if let Some(base) = base {
                self.scalar_member(class, base, new, raw)?
            } else {
                (self.instance(class, None), raw)
            };
            let key = self.text(&name);
            let canonical = match self.member_for_value(values, value) {
                Some(existing) => existing,
                None => {
                    self.default_set_attr(member, "_name_", key)?;
                    self.default_set_attr(member, "_value_", value)?;
                    self.put_item(values, value, member)?;
                    member
                }
            };
            self.put_item(names, key, canonical)?;
            self.bind(scope, &name, canonical);
        }
        Ok(())
    }

    fn scalar_member(&mut self, class: Id, base: Id, new: Option<Id>, raw: Id) -> Result<(Id, Id)> {
        let mut args = vec![class];
        match self.heap.get(raw) {
            Value::Tuple(items) => args.extend(items),
            _ => args.push(raw),
        }
        let member = if let Some(new) = new {
            self.call(new, args, Vec::new())?
        } else if matches!(self.heap.get(base), Value::Native(name) if name == "enum.StrEnum") {
            self.new_str(class, &args[1..], &[])?
        } else {
            self.new_int(&args, &[])?
        };
        let Value::Instance {
            fields,
            scalar: Some(scalar),
            ..
        } = self.heap.get(member)
        else {
            return Err(Error::new(
                "TypeError",
                "enum constructor must return a scalar member",
            ));
        };
        Ok((member, fields.get("_value_").copied().unwrap_or(*scalar)))
    }

    pub(super) fn enum_member(&self, member: Id) -> Option<(&str, Id, Id)> {
        let Value::Instance { class, fields, .. } = self.heap.get(member) else {
            return None;
        };
        self.enum_values(*class)?;
        let Value::Class { name, .. } = self.heap.get(*class) else {
            unreachable!()
        };
        Some((name, *fields.get("_name_")?, *fields.get("_value_")?))
    }
}
