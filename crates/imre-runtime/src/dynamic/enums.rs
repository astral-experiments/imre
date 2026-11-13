//! Build enum members from source declarations and run scalar members' original constructors.

use super::*;

pub(super) mod auto;
mod bases;
mod display;
mod factory;
mod float;

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
        let values = self.namespace_binding(self.scope_data(*scope), "_value2member_map_")?;
        if !matches!(self.heap.get(values), Value::Dict(_)) {
            return None;
        }
        ancestors
            .iter()
            .any(|&base| matches!(self.heap.get(base), Value::Native(name) if name == "enum.Enum"))
            .then_some(values)
    }

    fn scalar_enum_base(&self, base: Id) -> bool {
        matches!(self.heap.get(base), Value::Native(name) if matches!(name.as_str(), "enum.StrEnum" | "enum.IntEnum" | "enum.IntFlag"))
    }

    pub(super) fn enum_lookup(
        &mut self,
        class: Id,
        values: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let name = self.class_name(class);
        let [value] = args else {
            return Err(Error::new("TypeError", format!("{name} requires a value")));
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("unexpected keyword argument to {name}"),
            ));
        }
        let flag = self.has_base(class, "enum.Flag");
        if flag {
            let names = self.lookup_attr(class, "_member_map_")?;
            if !self.truth(names)? {
                let name = self.class_name(class);
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "<flag '{name}'> has no members; specify `names=()` if you meant to create a new, empty, enum"
                    ),
                ));
            }
        }
        if matches!(self.heap.get(*value), Value::Instance { class: actual, .. } if *actual == class)
        {
            return Ok(*value);
        }
        if flag || self.float_enum_names(class).is_some() {
            if let Some(member) = self.indexed_enum_lookup(class, values, *value)? {
                return Ok(member);
            }
        } else if let Some(member) = self.member_for_value(values, *value) {
            return Ok(member);
        }
        if flag {
            let missing = self.lookup_attr(class, "_missing_")?;
            let result = self.call(missing, vec![*value], Vec::new());
            return self.check_flag_missing(class, *value, result);
        }
        let name = if self.float_enum_names(class).is_some() {
            self.class_qualname(class)
        } else {
            self.class_name(class)
        };
        Err(Error::new(
            "ValueError",
            format!("{} is not a valid {name}", self.repr(*value)),
        ))
    }

    /// Flag._missing_ supplies the default construction policy; subclasses can override it.
    pub(in crate::dynamic) fn flag_missing(&mut self, class: Id, value: Id) -> Result<Id> {
        let bits = self
            .as_int(value)
            .map_err(|_| self.invalid_enum_value(class, value))?;
        self.flag_combination(class, value, bits)
    }

    fn invalid_enum_value(&self, class: Id, value: Id) -> Error {
        Error::new(
            "ValueError",
            format!(
                "{} is not a valid {}",
                self.repr(value),
                self.class_qualname(class)
            ),
        )
    }

    fn check_flag_missing(&mut self, class: Id, value: Id, result: Result<Id>) -> Result<Id> {
        let mut error = match result {
            Ok(member) if self.instance_of(member, class) => return Ok(member),
            Ok(member)
                if self.module_value("enum", "EJECT") == self.class_attr(class, "_boundary_")
                    && self.as_int(member).is_ok() =>
            {
                return Ok(member);
            }
            Ok(NONE) => return Err(self.invalid_enum_value(class, value)),
            Ok(other) => Error::new(
                "TypeError",
                format!(
                    "error in {}._missing_: returned {} instead of None or a valid member",
                    self.class_name(class),
                    self.repr(other)
                ),
            ),
            Err(error)
                if self.is_builtin_error(&error, "ValueError")
                    || !self.is_builtin_error(&error, "Exception") =>
            {
                return Err(error);
            }
            Err(error) => error,
        };
        error.context = self.raised_instance(self.invalid_enum_value(class, value));
        Err(error)
    }

    /// Enum class membership includes its declared aliases and constructible flag combinations.
    pub(super) fn flag_contains(&mut self, class: Id, item: Id) -> Result<bool> {
        if self.instance_of(item, class) {
            return Ok(true);
        }
        let missing = self.lookup_attr(class, "_missing_")?;
        let result = self.call(missing, vec![item], Vec::new());
        match result {
            Ok(member) => Ok(self.instance_of(member, class)),
            Err(error) if self.is_builtin_error(&error, "ValueError") => {
                self.declared_flag_value(class, item)
            }
            Err(error) => Err(error),
        }
    }

    fn declared_flag_value(&mut self, class: Id, item: Id) -> Result<bool> {
        if let Some(unhashable) = self.class_attr(class, "_unhashable_values_")
            && self.contains(item, unhashable)?
        {
            return Ok(true);
        }
        let declared = self.lookup_attr(class, "_hashable_values_")?;
        self.contains(item, declared)
    }

    /// Intern a combination without adding a new declaration to __members__.
    fn flag_combination(&mut self, class: Id, original: Id, bits: crate::Int) -> Result<Id> {
        use num_bigint::BigInt;
        use num_traits::{One, Signed, Zero};

        // populate_enum retains the original declarations in __members__. The
        // single-bit mask and order do not depend on a later name-map replacement.
        let declared = self.lookup_attr(class, "__members__")?;
        let declared = self
            .dict_entries(self.mapping_proxy_target(declared))
            .ok_or_else(|| Error::new("TypeError", "expected enum member dictionary"))?;
        let mut singles = Vec::new();
        let mut mask = BigInt::zero();
        for &(key, member) in declared {
            let Some((_, canonical, value)) = self.enum_member(member) else {
                continue;
            };
            if canonical != key {
                continue;
            }
            let Ok(bits) = self.as_int(value) else {
                continue;
            };
            let bit = bits.big().into_owned();
            mask |= &bit;
            if bit.is_positive() && (&bit & (&bit - 1u8)).is_zero() {
                singles.push((member, bit));
            }
        }
        let boundary = self.class_attr(class, "_boundary_");
        let policy = |name| boundary.is_some() && self.module_value("enum", name) == boundary;
        let strict = if boundary.is_none() {
            !self.has_base(class, "enum.IntFlag")
        } else {
            policy("STRICT")
        };
        // A pseudo-member keeps the supplied object as its .value until a
        // boundary operation produces a different Python value.
        let mut key = original;
        let mut value = bits.big().into_owned();
        let limit = BigInt::one() << mask.bits();
        let all_bits = &limit - 1u8;
        if value < -&limit || value > all_bits || !(&value & (&all_bits ^ &mask)).is_zero() {
            if strict {
                let name = self.class_qualname(class);
                return Err(Error::new(
                    "ValueError",
                    format!("{value} is not a valid {name}"),
                ));
            } else if policy("EJECT") {
                return Ok(original);
            } else if policy("CONFORM") {
                let declared = self.alloc(Value::Int(crate::Int::new(mask.clone())));
                key = self.binary("&", key, declared)?;
                value = self.as_int(key)?.big().into_owned();
            } else if let Some(boundary) = boundary
                && !policy("KEEP")
            {
                return Err(Error::new(
                    "ValueError",
                    format!(
                        "{} unknown flag boundary {}",
                        self.repr(class),
                        self.repr(boundary)
                    ),
                ));
            } else if value.is_negative() {
                let limit = BigInt::one() << mask.bits().max(value.bits());
                let high = self.alloc(Value::Int(crate::Int::new(limit)));
                key = self.binary("+", high, key)?;
                value = self.as_int(key)?.big().into_owned();
            }
        }
        let mut negative_alias = None;
        if value.is_negative() {
            negative_alias = Some(key);
            let high = self.alloc(Value::Int(crate::Int::new(limit)));
            key = self.binary("+", high, key)?;
            value = self.as_int(key)?.big().into_owned();
        }

        let member = if !self.has_base(class, "int") {
            self.instance(class, None)
        } else {
            self.new_immutable_scalar("int", class, &[key], &[])?
        };
        self.default_set_attr(member, "_value_", key)?;
        let mut members = Vec::new();
        let mut combined = BigInt::zero();
        for (member, bit) in singles {
            if (&value & &bit) == bit {
                combined |= bit;
                members.push(member);
            }
        }
        // Flag._missing_ consults the live name map only for bits outside the
        // single-bit members. Its items method may itself be overridden.
        if combined != value {
            let names = self.lookup_attr(class, "_member_map_")?;
            let entries = match self.heap.get(names) {
                Value::Dict(entries) => entries.iter().map(|&(_, member)| member).collect(),
                _ => {
                    let items = self.lookup_attr(names, "items")?;
                    let pairs = self.call(items, Vec::new(), Vec::new())?;
                    let mut entries = Vec::new();
                    for pair in self.items(pairs)? {
                        entries.push(self.unpack_items(pair, 2, None)?[1]);
                    }
                    entries
                }
            };
            for member in entries {
                if members.contains(&member) {
                    continue;
                }
                let bits = self.lookup_attr(member, "_value_")?;
                let bits = self.as_int(bits)?.big().into_owned();
                if !bits.is_zero() && (&value & &bits) == bits {
                    combined |= bits;
                    members.push(member);
                }
            }
        }
        let name = if members.is_empty() {
            NONE
        } else {
            let mut names = Vec::new();
            for member in members {
                let name = self.lookup_attr(member, "_name_")?;
                names.push(self.str_value(name)?.to_owned());
            }
            let prefix = self.text(names.join("|"));
            let unknown = value ^ combined;
            if unknown.is_zero() {
                prefix
            } else if strict {
                return Err(Error::new(
                    "ValueError",
                    format!("{}: no members with value {unknown}", self.repr(class)),
                ));
            } else {
                let formatter = self.lookup_attr(class, "_numeric_repr_")?;
                let unknown = self.alloc(Value::Int(crate::Int::new(unknown)));
                let formatted = self.call(formatter, vec![unknown], Vec::new())?;
                let suffix = builtins::percent(self, "|%s", formatted)?;
                self.binary("+", prefix, suffix)?
            }
        };

        self.default_set_attr(member, "_name_", name)?;
        // A formatter can replace the map or intern this value recursively.
        let values = self.lookup_attr(class, "_value2member_map_")?;
        let setdefault = self.lookup_attr(values, "setdefault")?;
        let member = self.call(setdefault, vec![key, member], Vec::new())?;
        if let Some(original) = negative_alias {
            let values = self.lookup_attr(class, "_value2member_map_")?;
            self.put_item(values, original, member)?;
        }
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
        let Value::Class { scope: source, .. } = *self.heap.get(*class) else {
            return Err(Error::new("TypeError", "enum decorator requires a class"));
        };
        // _simple_enum constructs a new type from the template's own namespace.
        // Give it the enum base at allocation so lookup and __mro__ agree.
        let name = self.class_name(*class).to_owned();
        let module = self.scope_data(source).module.clone();
        let dictionary = self.class_namespace_dict(source, false);
        let members = self
            .dict_entries(dictionary)
            .expect("class dictionary")
            .to_vec();
        for &(key, value) in &members {
            self.heap.keep(key);
            self.heap.keep(value);
        }
        let scope = self.scope(&module, None);
        self.heap.keep(scope);
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        fields.in_class_body = true;
        let qualname = self.text(&name);
        self.bind_local(scope, "__qualname__", qualname);
        let mut declarations = Vec::new();
        for (key, value) in members {
            let key = self.str_value(key)?.to_owned();
            if matches!(key.as_str(), "__dict__" | "__weakref__") {
                continue;
            }
            if key.starts_with('_') || self.is_enum_descriptor(value)? {
                self.bind_local(scope, &key, value);
            } else {
                declarations.push((key, value));
            }
        }
        if self.scope_name(scope, "__doc__").unwrap_or(NONE) == NONE {
            let doc = self.text("An enumeration.");
            self.bind_local(scope, "__doc__", doc);
        }
        // Descriptors see the new type and its empty member map. Each member
        // becomes visible only after its constructor returns.
        self.bind_enum_boundary(scope, &[*base], NONE)?;
        let maps = self.enum_member_maps(scope);
        let class = self.allocate_enum_class(name, scope, *base)?;
        self.install_enum_members(class, Some(*base), maps, declarations, true)?;
        Ok(class)
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
                .filter_map(|name| {
                    self.namespace_binding(self.scope_data(scope), &name)
                        .map(|value| (name, value))
                })
                .collect(),
            None => self
                .scope_bindings(scope)
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        };
        let maps = self.enum_member_maps(scope);
        self.install_enum_members(class, base, maps, declarations, classified)
    }

    fn enum_member_maps(&mut self, scope: Id) -> (Id, Id) {
        let names = self.dict(Vec::new());
        let values = self.dict(Vec::new());
        self.bind(scope, "_member_map_", names);
        self.bind(scope, "_value2member_map_", values);
        let proxy = self.alloc(Value::MappingProxy(names));
        self.bind(scope, "__members__", proxy);
        (names, values)
    }

    fn install_enum_members(
        &mut self,
        class: Id,
        base: Option<Id>,
        (names, values): (Id, Id),
        declarations: Vec<(String, Id)>,
        classified: bool,
    ) -> Result<()> {
        let Value::Class { scope, .. } = *self.heap.get(class) else {
            unreachable!()
        };
        if let Some(data_type) = base.filter(|&base| self.has_base(base, "float")) {
            return self.populate_float_enum(class, scope, data_type, names, values, declarations);
        }
        let flag = self.has_base(class, "enum.Flag");
        let mut flag_mask = num_bigint::BigInt::default();
        let mut singles_mask = num_bigint::BigInt::default();
        let mut previous_single = num_bigint::BigInt::default();
        let mut definition_order = false;
        let (ordered, declared_values) = if flag {
            let ordered = self.alloc(Value::List(Vec::new()));
            self.bind(scope, "_member_names_", ordered);
            let declared = self.alloc(Value::List(Vec::new()));
            self.bind(scope, "_hashable_values_", declared);
            let unhashable = self.dict(Vec::new());
            self.bind(scope, "_unhashable_values_map_", unhashable);
            (ordered, declared)
        } else {
            (NONE, NONE)
        };
        let mut single_count = 0;
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
            if flag && canonical == member {
                use num_traits::{Signed, Zero};
                let Value::List(declared) = self.heap.get_mut(declared_values) else {
                    unreachable!()
                };
                declared.push(value);
                let order = self.int(single_count);
                self.default_set_attr(member, "_sort_order_", order)?;
                if let Ok(bits) = self.as_int(value) {
                    let bit = bits.big();
                    flag_mask |= bit.as_ref();
                    if bit.is_positive() && (bit.as_ref() & (bit.as_ref() - 1u8)).is_zero() {
                        singles_mask |= bit.as_ref();
                        definition_order |= bit.as_ref() < &previous_single;
                        previous_single = bit.into_owned();
                        let Value::List(declared) = self.heap.get_mut(ordered) else {
                            unreachable!()
                        };
                        declared.push(key);
                        single_count += 1;
                    }
                }
            }
            self.put_item(names, key, canonical)?;
            self.bind(scope, &name, canonical);
        }
        if flag {
            // Native module providers construct their single-base enums directly.
            if !classified && let Some(boundary) = self.class_attr(class, "_boundary_") {
                self.bind(scope, "_boundary_", boundary);
            }
            let full = self.alloc(Value::Int(crate::Int::new(flag_mask)));
            self.bind(scope, "_flag_mask_", full);
            self.bind(
                scope,
                "_flag_definition_order",
                if definition_order { TRUE } else { FALSE },
            );
            let mask = self.alloc(Value::Int(crate::Int::new(singles_mask)));
            self.bind(scope, "_singles_mask_", mask);
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
                | Value::ClassMethod { .. }
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
        } else {
            let kind = if matches!(self.heap.get(base), Value::Native(name) if name == "enum.StrEnum")
            {
                "str"
            } else {
                "int"
            };
            self.new_immutable_scalar(kind, class, &args[1..], &[])?
        };
        let Value::Instance {
            scalar: Some(scalar),
            ..
        } = self.heap.get(member)
        else {
            return Err(Error::new(
                "TypeError",
                "enum constructor must return a scalar member",
            ));
        };
        Ok((
            member,
            self.raw_instance_field(member, "_value_")
                .unwrap_or(*scalar),
        ))
    }

    pub(super) fn enum_member(&self, member: Id) -> Option<(&str, Id, Id)> {
        let Value::Instance { class, .. } = self.heap.get(member) else {
            return None;
        };
        self.enum_values(*class)?;
        let Value::Class { name, .. } = self.heap.get(*class) else {
            unreachable!()
        };
        Some((
            name,
            self.raw_instance_field(member, "_name_")?,
            self.raw_instance_field(member, "_value_")?,
        ))
    }
}
