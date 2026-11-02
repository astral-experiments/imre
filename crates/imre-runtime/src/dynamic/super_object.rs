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

    /// Allocate an uninitialized super object, retaining a Python subclass's own identity.
    pub(super) fn super_new(&mut self, args: &[Id], _kwargs: &[(String, Id)]) -> Result<Id> {
        let Some(&class) = args.first() else {
            return Err(Error::new(
                "TypeError",
                "super.__new__(): not enough arguments",
            ));
        };
        let base = self.builtin("super");
        if !self.super_subtype(class, base) {
            return Err(self.invalid_new_subtype("super", class));
        }
        let storage = self.alloc(Value::Super {
            start: NONE,
            receiver: NONE,
            owner: NONE,
        });
        if class == base {
            return Ok(storage);
        }
        let instance = self.instance(class, None);
        let Value::Instance { scalar, .. } = self.heap.get_mut(instance) else {
            unreachable!()
        };
        *scalar = Some(storage);
        Ok(instance)
    }

    /// Validation can invoke user code. Replace the binding only after it succeeds.
    pub(super) fn super_init(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&proxy, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "descriptor '__init__' of 'super' object needs an argument",
            ));
        };
        let storage = self.scalar_value(proxy);
        if !matches!(self.heap.get(storage), Value::Super { .. }) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '__init__' requires a 'super' object but received a '{}'",
                    self.type_name(proxy)
                ),
            ));
        }
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "super() takes no keyword arguments",
            ));
        }
        if args.len() > 2 {
            return Err(Error::new(
                "TypeError",
                format!("super() takes at most 2 arguments ({} given)", args.len()),
            ));
        }
        if let Some(&start) = args.first()
            && !self.is_type_object(start)
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "super() argument 1 must be type, not {}",
                    self.type_name(start)
                ),
            ));
        }
        let state = self.super_state(args)?;
        let Value::Super {
            start,
            receiver,
            owner,
        } = std::mem::replace(self.heap.get_mut(storage), state)
        else {
            unreachable!()
        };
        for old in [start, receiver, owner] {
            self.heap.weak_edge_removed(old);
        }
        Ok(NONE)
    }

    pub(super) fn make_super(&mut self, args: &[Id]) -> Result<Id> {
        let state = self.super_state(args)?;
        Ok(self.alloc(state))
    }

    /// supercheck uses the real MRO, never an ABC registration or a metaclass hook.
    fn super_subtype(&mut self, class: Id, base: Id) -> bool {
        self.is_type_object(class)
            && (class == base
                || if let Some(mro) = self.published_mro(class) {
                    mro.contains(&base)
                } else {
                    self.base_lineage(class, true).contains(&base)
                })
    }

    /// Read arg[0] from the active native frame and __class__ from its lexical class cell.
    fn super_state(&mut self, args: &[Id]) -> Result<Value> {
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
                if !self.is_type_object(*class) {
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
        if !self.is_type_object(start) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "super() argument 1 must be a type, not {}",
                    self.type_name(start)
                ),
            ));
        }
        let owner = self.super_owner(start, receiver)?;
        Ok(Value::Super {
            start,
            receiver,
            owner,
        })
    }

    fn super_owner(&mut self, start: Id, receiver: Id) -> Result<Id> {
        if receiver == NONE || self.super_subtype(receiver, start) {
            return Ok(receiver);
        }
        let actual = self.exact_type(receiver)?;
        if self.super_subtype(actual, start) {
            return Ok(actual);
        }
        // A proxy may report its wrapped type without changing the bound receiver.
        match self.lookup_attr(receiver, "__class__") {
            Ok(reported) if reported != actual && self.super_subtype(reported, start) => {
                return Ok(reported);
            }
            Err(error) if !self.is_attribute_error(&error) => return Err(error),
            _ => {}
        }
        let actual = if self.is_type_object(receiver) {
            format!("type {}", self.class_name(receiver))
        } else {
            format!("instance of {}", self.type_name(receiver))
        };
        Err(Error::new(
            "TypeError",
            format!(
                "super(type, obj): obj ({actual}) is not an instance or subtype of type ({}).",
                self.class_name(start)
            ),
        ))
    }

    /// Members of a super proxy read its binding without visiting the delegated MRO.
    pub(super) fn super_field(&self, proxy: Id, field: &str) -> Result<Id> {
        let Value::Super {
            start,
            receiver,
            owner,
        } = *self.heap.get(self.scalar_value(proxy))
        else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{field}' for 'super' objects doesn't apply to a '{}' object",
                    self.type_name(proxy)
                ),
            ));
        };
        Ok(match field {
            "__thisclass__" => start,
            "__self__" => receiver,
            "__self_class__" => owner,
            _ => unreachable!(),
        })
    }

    pub(super) fn super_repr(&self, proxy: Id) -> String {
        let Value::Super { start, owner, .. } = *self.heap.get(self.scalar_value(proxy)) else {
            unreachable!()
        };
        let start = if start == NONE {
            "NULL"
        } else {
            self.class_name(start)
        };
        if owner == NONE {
            format!("<super: <class '{start}'>, NULL>")
        } else {
            format!(
                "<super: <class '{start}'>, <{} object>>",
                self.class_name(owner)
            )
        }
    }

    /// Use the proxy's own attributes only after its target MRO misses the name.
    pub(super) fn super_getattribute(&mut self, proxy: Id, name: &str) -> Result<Id> {
        let Value::Super {
            start,
            receiver,
            owner,
        } = *self.heap.get(self.scalar_value(proxy))
        else {
            unreachable!()
        };
        if name != "__class__"
            && let Some(member) = self.lookup_super_member(start, receiver, owner, name)?
        {
            return Ok(member);
        }
        self.default_get_attr(proxy, name)
    }

    pub(super) fn super_attribute(
        &mut self,
        start: Id,
        receiver: Id,
        owner: Id,
        name: &str,
    ) -> Result<Id> {
        self.lookup_super_member(start, receiver, owner, name)?
            .ok_or_else(|| {
                Error::new(
                    "AttributeError",
                    format!("'super' object has no attribute '{name}'"),
                )
            })
    }

    /// Search the delegated MRO, preserving exceptions raised by descriptors.
    pub(super) fn lookup_super_member(
        &mut self,
        start: Id,
        receiver: Id,
        owner: Id,
        name: &str,
    ) -> Result<Option<Id>> {
        if owner != NONE {
            let exception_method = builtins::base_exception_method(name).is_some();
            let lineage = self.base_lineage(owner, exception_method);
            let after = lineage.iter().skip_while(|&&class| class != start).skip(1);
            let instance = if owner == receiver { NONE } else { receiver };
            for &class in after {
                if let Some(value) = self.own_class_attr(class, name) {
                    return self.bind_attribute(value, instance, owner).map(Some);
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
                    return self.bind_attribute(member, instance, owner).map(Some);
                }
            }
        }
        Ok(None)
    }
}
