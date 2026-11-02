use super::*;

impl Context {
    pub(super) fn class_name(&self, class: Id) -> &str {
        match self.heap.get(class) {
            Value::Class { name, scope, .. } => self
                .scope_data(*scope)
                .class_namespace
                .as_ref()
                .and_then(|namespace| namespace.name_override)
                .map_or(name.as_str(), |id| {
                    self.str_value(id).expect("class name is a string")
                }),
            Value::Native(name) => name,
            _ => self.type_name(class),
        }
    }

    pub(super) fn class_qualname(&self, class: Id) -> &str {
        self.own_class_attr(class, "__qualname__")
            .and_then(|qualname| self.str_value(qualname).ok())
            .unwrap_or_else(|| self.class_name(class).rsplit('.').next().unwrap())
    }

    /// Borrow the immutable MRO tuple retained by a completed Python class.
    pub(super) fn published_mro(&self, class: Id) -> Option<&[Id]> {
        let Value::Class { scope, .. } = self.heap.get(class) else {
            return None;
        };
        let namespace = self.scope_data(*scope).class_namespace.as_ref()?;
        match self.heap.get(namespace.mro) {
            Value::Tuple(bases) => Some(bases),
            _ => None,
        }
    }

    pub(super) fn lineage(&mut self, class: Id) -> Vec<Id> {
        self.base_lineage(class, false)
    }

    pub(super) fn base_lineage(&mut self, class: Id, published: bool) -> Vec<Id> {
        if published && let Some(mro) = self.published_mro(class) {
            return mro.to_vec();
        }
        let mut lineage = vec![class];
        if let Value::Class { ancestors, .. } = self.heap.get(class) {
            lineage.extend(ancestors);
        } else {
            if let Value::Native(name) = self.heap.get(class) {
                match name.as_str() {
                    name if published && let Some(kind) = builtins::exception_kind(name) => {
                        for base in builtins::exception_ancestors(kind) {
                            lineage.push(self.builtin(base));
                        }
                    }
                    name if let Some(parent) = Self::native_base(name) => {
                        lineage.push(self.builtin(parent));
                    }
                    "enum.IntEnum" | "enum.IntFlag" | "enum.StrEnum" => {
                        let scalar = if name == "enum.StrEnum" { "str" } else { "int" };
                        lineage.push(self.builtin(scalar));
                        lineage.push(self.builtin("enum.Enum"));
                    }
                    _ => {}
                }
            }
            let object = self.builtin("object");
            if class != object {
                lineage.push(object);
            }
        }
        lineage
    }

    pub(super) fn type_mro(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&class, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "unbound method type.mro() needs an argument",
            ));
        };
        if !self.is_type_object(class) {
            let kind = self
                .builtin_type_name(class)
                .unwrap_or_else(|| self.type_name(class));
            return Err(Error::new(
                "TypeError",
                format!("descriptor 'mro' for 'type' objects doesn't apply to a '{kind}' object"),
            ));
        }
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}.mro() takes no keyword arguments",
                    self.class_qualname(class)
                ),
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "{}.mro() takes no arguments ({} given)",
                    self.class_qualname(class),
                    args.len()
                ),
            ));
        }
        let mro = self.base_lineage(class, true);
        Ok(self.alloc(Value::List(mro)))
    }

    /// Type metadata lives on the metaclass, outside the class's instance attributes.
    pub(super) fn builtin_hierarchy_tuple(&mut self, class: Id, name: &str) -> Id {
        let key = format!("{name}:{}", self.class_name(class));
        if let Some(&tuple) = self.builtins.get(&key) {
            return tuple;
        }
        let lineage = self.base_lineage(class, true);
        // The classes exported directly by builtins have at most one direct base.
        let types = if name == "__bases__" {
            lineage.get(1).copied().into_iter().collect()
        } else {
            lineage
        };
        let tuple = self.alloc(Value::Tuple(types));
        self.builtins.insert(key, tuple);
        tuple
    }

    /// Merge the base lineages in C3 order so cooperative methods visit each base once.
    pub(super) fn linearized_bases(&mut self, bases: &[Id], published: bool) -> Result<Vec<Id>> {
        if bases.is_empty() {
            return Ok(vec![self.builtin("object")]);
        }
        if let [base] = bases {
            return Ok(self.base_lineage(*base, published));
        }
        for (index, base) in bases.iter().enumerate() {
            if bases[..index].contains(base) {
                return Err(Error::new(
                    "TypeError",
                    format!("duplicate base class {}", self.class_name(*base)),
                ));
            }
        }
        let lineages: Vec<_> = bases
            .iter()
            .map(|&base| self.base_lineage(base, published))
            .collect();
        let mut paths: Vec<&[Id]> = lineages.iter().map(Vec::as_slice).collect();
        paths.push(bases);
        let mut order = Vec::new();
        while paths.iter().any(|p| !p.is_empty()) {
            let next = paths
                .iter()
                .filter_map(|p| p.first())
                .find(|candidate| {
                    paths
                        .iter()
                        .all(|p| !p.get(1..).unwrap_or_default().contains(candidate))
                })
                .copied()
                .ok_or_else(|| {
                    let mut remaining = Vec::new();
                    for class in paths.iter().filter_map(|p| p.first()) {
                        if !remaining.contains(class) {
                            remaining.push(*class);
                        }
                    }
                    let bases = remaining.iter().map(|&b| self.class_name(b)).collect::<Vec<_>>().join(", ");
                    Error::new("TypeError", format!("Cannot create a consistent method resolution order (MRO) for bases {bases}"))
                })?;
            order.push(next);
            for path in &mut paths {
                if path.first() == Some(&next) {
                    *path = &path[1..];
                }
            }
        }
        Ok(order)
    }

    /// Read arg[0] from the active native frame and __class__ from its lexical class cell.
    pub(super) fn make_super(&mut self, args: &[Id]) -> Result<Id> {
        let (start, receiver) = match args {
            [] => {
                let scope = self.heap.current_scope();
                let Value::Scope(frame) = self.heap.get(scope) else {
                    return Err(Error::new("RuntimeError", "super(): no arguments"));
                };
                let first = frame
                    .first_argument
                    .as_ref()
                    .ok_or_else(|| Error::new("RuntimeError", "super(): no arguments"))?;
                let receiver = self
                    .scope_name(scope, first)
                    .ok_or_else(|| Error::new("RuntimeError", "super(): arg[0] deleted"))?;
                let cell = frame.class_cell.ok_or_else(|| {
                    Error::new("RuntimeError", "super(): __class__ cell not found")
                })?;
                let Value::Cell(Some(class)) = self.heap.get(cell) else {
                    return Err(Error::new("RuntimeError", "super(): empty __class__ cell"));
                };
                if self.builtin_type_name(*class) != Some("type") {
                    return Err(Error::new(
                        "RuntimeError",
                        format!(
                            "super(): __class__ is not a type ({})",
                            self.type_name(*class)
                        ),
                    ));
                }
                (*class, receiver)
            }
            [start] => (*start, NONE),
            [start, receiver] => (*start, *receiver),
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("super() expected at most 2 arguments, got {}", args.len()),
                ));
            }
        };
        if self.builtin_type_name(start) != Some("type") {
            return Err(Error::new(
                "TypeError",
                format!(
                    "super() argument 1 must be a type, not {}",
                    self.type_name(start)
                ),
            ));
        }
        let owner = if receiver == NONE {
            NONE
        } else if matches!(
            self.heap.get(receiver),
            Value::Class { .. } | Value::Native(_)
        ) && self.subclass(receiver, start)
        {
            receiver
        } else if let Some(class) = self.heap.get(receiver).instance_class()
            && self.subclass(class, start)
        {
            class
        } else if self.instance_of(receiver, start) {
            self.exact_type(receiver)?
        } else {
            // Python proxies can report the wrapped class through a descriptor.
            match self.lookup_attr(receiver, "__class__") {
                Ok(reported) if self.subclass(reported, start) => reported,
                Err(error) if error.kind != "AttributeError" => return Err(error),
                _ => {
                    let actual = match self.heap.get(receiver) {
                        Value::Class { .. } | Value::Native(_) => {
                            format!("type {}", self.class_name(receiver))
                        }
                        _ => format!("instance of {}", self.type_name(receiver)),
                    };
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "super(type, obj): obj ({actual}) is not an instance or subtype of type ({}).",
                            self.class_name(start)
                        ),
                    ));
                }
            }
        };
        Ok(self.alloc(Value::Super {
            start,
            receiver,
            owner,
        }))
    }

    pub(super) fn super_attribute(
        &mut self,
        start: Id,
        receiver: Id,
        owner: Id,
        name: &str,
    ) -> Result<Id> {
        match name {
            "__thisclass__" => return Ok(start),
            "__self__" => return Ok(receiver),
            "__self_class__" => return Ok(owner),
            "__class__" => return Ok(self.builtin("super")),
            _ => {}
        }
        if owner != NONE {
            let exception_method = builtins::base_exception_method(name).is_some();
            let lineage = self.base_lineage(owner, exception_method);
            let after = lineage.iter().skip_while(|&&class| class != start).skip(1);
            let instance = if owner == receiver { NONE } else { receiver };
            for &class in after {
                if let Some(value) = self.own_class_attr(class, name) {
                    return self.bind_attribute(value, instance, owner);
                }
                // Resolve inherited object methods at their real position, after any Python mixin.
                if let Value::Native(kind) = self.heap.get(class)
                    && !matches!(
                        name,
                        "__init_subclass__" | "__getstate__" | "__reduce_ex__" | "mro"
                    )
                    && !(exception_method && builtins::is_exception(kind))
                {
                    let mut member = match self.lookup_attr(class, name) {
                        Ok(member) => member,
                        Err(error) if error.kind == "AttributeError" => continue,
                        Err(error) => return Err(error),
                    };
                    if let Value::Method {
                        callable,
                        receiver: bound,
                        ..
                    } = *self.heap.get(member)
                        && bound == class
                        && self.class_attr(class, name) == Some(callable)
                    {
                        // Lookup may have installed the raw descriptor on this native class.
                        // An inherited descriptor must wait for its own position in the MRO.
                        if self.own_class_attr(class, name) != Some(callable) {
                            continue;
                        }
                        member = callable;
                    }
                    return self.bind_attribute(member, instance, owner);
                }
            }
        }
        Err(Error::new(
            "AttributeError",
            format!("'super' object has no attribute '{name}'"),
        ))
    }
}
