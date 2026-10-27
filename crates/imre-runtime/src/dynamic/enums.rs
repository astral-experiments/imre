//! Build enum members from source declarations and run scalar members' original constructors.

use super::*;

pub(super) mod auto;

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
        matches!(self.heap.get(base), Value::Native(name) if matches!(name.as_str(), "enum.StrEnum" | "enum.IntEnum" | "enum.IntFlag"))
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
            memory_stream: None,
        }))
    }

    pub(super) fn enum_lookup(
        &mut self,
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
        if let Some(member) = self.member_for_value(values, *value) {
            return Ok(member);
        }
        if self.has_base(class, "enum.IntFlag")
            && let Ok(bits) = self.as_int(*value)
        {
            return self.flag_combination(class, values, bits);
        }
        Err(Error::new(
            "ValueError",
            format!("{} is not a valid {name}", self.repr(*value)),
        ))
    }

    /// Intern a combination without adding a new declaration to __members__.
    fn flag_combination(&mut self, class: Id, values: Id, bits: crate::Int) -> Result<Id> {
        use num_bigint::BigInt;
        use num_traits::{One, Signed, Zero};

        let names = self
            .class_attr(class, "_member_map_")
            .expect("enum name map");
        let Value::Dict(entries) = self.heap.get(names) else {
            unreachable!()
        };
        let mut declared = Vec::new();
        let mut mask = BigInt::zero();
        for &(key, member) in entries {
            let Some((_, canonical, value)) = self.enum_member(member) else {
                continue;
            };
            if canonical != key {
                continue;
            }
            let bit = self.as_int(value)?.big().into_owned();
            mask |= &bit;
            if bit.is_positive() && (&bit & (&bit - 1u8)).is_zero() {
                declared.push((self.str_value(key)?.to_owned(), bit));
            }
        }
        let mut value = bits.big().into_owned();
        if value.is_negative() {
            value += BigInt::one() << mask.bits().max(value.bits());
        }
        let key = self.alloc(Value::Int(crate::Int::new(value.clone())));
        if let Some(member) = self.member_for_value(values, key) {
            return Ok(member);
        }
        let mut names = Vec::new();
        let mut rest = value;
        for (name, bit) in declared {
            if (&rest & &bit) == bit {
                rest ^= bit;
                names.push(name);
            }
        }
        let name = if names.is_empty() {
            NONE
        } else {
            if !rest.is_zero() {
                names.push(if self.classes.get("re.RegexFlag") == Some(&class) {
                    format!("{rest:#x}")
                } else {
                    rest.to_string()
                });
            }
            self.text(names.join("|"))
        };
        let member = self.new_int(&[class, key], &[])?;
        self.default_set_attr(member, "_name_", name)?;
        self.default_set_attr(member, "_value_", key)?;
        self.put_item(values, key, member)?;
        Ok(member)
    }

    pub(super) fn flag_class(&self, member: Id) -> Option<Id> {
        let Value::Instance { class, .. } = *self.heap.get(member) else {
            return None;
        };
        self.has_base(class, "enum.IntFlag").then_some(class)
    }

    pub(super) fn flag_or_integer(&mut self, class: Option<Id>, bits: crate::Int) -> Result<Id> {
        let value = self.alloc(Value::Int(bits));
        if let Some(class) = class
            && let Some(values) = self.enum_values(class)
        {
            self.enum_lookup(class, values, &[value], &[])
        } else {
            Ok(value)
        }
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
            explicit: false,
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
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        let state = fields.enum_namespace.take();
        let classified = state.is_some();
        let declarations: Vec<_> = match state {
            Some(state) => state
                .members
                .into_iter()
                .map(|name| {
                    let raw = fields.names[&name];
                    (name, raw)
                })
                .collect(),
            None => fields.names.clone().into_iter().collect(),
        };
        let names = self.dict(Vec::new());
        let values = self.dict(Vec::new());
        self.bind(scope, "_member_map_", names);
        self.bind(scope, "_value2member_map_", values);
        let proxy = self.alloc(Value::MappingProxy(names));
        self.bind(scope, "__members__", proxy);
        let new = self.class_attr(class, "__new__");
        for (name, raw) in declarations {
            if !classified && (name.starts_with('_') || self.is_enum_descriptor(raw)?) {
                continue;
            }
            let (member, value) = if let Some(base) = base {
                self.scalar_member(class, base, new, raw)?
            } else if let Some(new) = new {
                let args = self.enum_new_args(class, raw);
                let member = self.call(new, args, Vec::new())?;
                let value = self.lookup_attr(member, "_value_").or_else(|error| {
                    if self.is_attribute_error(&error) {
                        Ok(raw)
                    } else {
                        Err(error)
                    }
                })?;
                (member, value)
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

    /// Enum checks for descriptor attributes on the value itself, in Python lookup order.
    fn is_enum_descriptor(&mut self, value: Id) -> Result<bool> {
        if matches!(
            self.heap.get(value),
            Value::Property(_)
                | Value::Function { .. }
                | Value::StaticMethod { .. }
                | Value::ClassMethod(_)
        ) {
            return Ok(true);
        }
        for name in ["__get__", "__set__", "__delete__"] {
            match self.lookup_attr(value, name) {
                Ok(_) => return Ok(true),
                Err(error) if self.is_attribute_error(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    fn enum_new_args(&self, class: Id, raw: Id) -> Vec<Id> {
        let mut args = vec![class];
        match self.heap.get(raw) {
            Value::Tuple(items) => args.extend(items),
            _ => args.push(raw),
        }
        args
    }

    fn scalar_member(&mut self, class: Id, base: Id, new: Option<Id>, raw: Id) -> Result<(Id, Id)> {
        let args = self.enum_new_args(class, raw);
        let member = if let Some(new) = new {
            self.call(new, args, Vec::new())?
        } else if matches!(self.heap.get(base), Value::Native(name) if name == "enum.StrEnum") {
            self.new_string("str", class, &args[1..], &[])?
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
