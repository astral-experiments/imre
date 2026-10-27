use super::*;

impl Context {
    fn class_name(&self, class: Id) -> &str {
        match self.heap.get(class) {
            Value::Class { name, .. } | Value::Native(name) => name,
            _ => self.type_name(class),
        }
    }

    pub(super) fn lineage(&mut self, class: Id) -> Vec<Id> {
        let mut lineage = vec![class];
        if let Value::Class { ancestors, .. } = self.heap.get(class) {
            lineage.extend(ancestors);
        } else {
            if let Value::Native(name) = self.heap.get(class) {
                match name.as_str() {
                    "bool" => lineage.push(self.builtin("int")),
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

    /// Merge the base lineages in C3 order so cooperative methods visit each base once.
    pub(super) fn linearized_bases(&mut self, bases: &[Id]) -> Result<Vec<Id>> {
        if bases.is_empty() {
            return Ok(vec![self.builtin("object")]);
        }
        if let [base] = bases {
            return Ok(self.lineage(*base));
        }
        for (index, base) in bases.iter().enumerate() {
            if bases[..index].contains(base) {
                return Err(Error::new(
                    "TypeError",
                    format!("duplicate base class {}", self.class_name(*base)),
                ));
            }
        }
        let lineages: Vec<_> = bases.iter().map(|&base| self.lineage(base)).collect();
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
                let Value::ClassCell(Some(class)) = self.heap.get(cell) else {
                    return Err(Error::new("RuntimeError", "super(): empty __class__ cell"));
                };
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
            let lineage = self.lineage(owner);
            let after = lineage.iter().skip_while(|&&class| class != start).skip(1);
            let instance = if owner == receiver { NONE } else { receiver };
            for &class in after {
                if let Some(value) = self.own_class_attr(class, name) {
                    return self.bind_attribute(value, instance, owner);
                }
                if matches!(self.heap.get(class), Value::Native(_)) {
                    match self.lookup_attr(class, name) {
                        Ok(value) => return self.bind_attribute(value, instance, owner),
                        Err(error) if error.kind == "AttributeError" => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        Err(Error::new(
            "AttributeError",
            format!("'super' object has no attribute '{name}'"),
        ))
    }
}
