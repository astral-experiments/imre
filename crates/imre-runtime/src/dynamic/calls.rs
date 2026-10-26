use super::*;

impl Context {
    fn class_attr(&self, class: Id, name: &str) -> Option<Id> {
        let Value::Class { scope, bases, .. } = self.heap.get(class) else {
            return None;
        };
        self.scope_data(*scope)
            .names
            .get(name)
            .copied()
            .or_else(|| bases.iter().find_map(|&base| self.class_attr(base, name)))
    }
    fn subclass(&self, class: Id, target: Id) -> bool {
        if self.equal_class(class, target) {
            return true;
        }
        if let Value::Class { bases, .. } = self.heap.get(class) {
            return bases.iter().any(|&b| self.subclass(b, target));
        }
        false
    }
    fn equal_class(&self, a: Id, b: Id) -> bool {
        a == b
            || matches!((self.heap.get(a),self.heap.get(b)),(Value::Native(x),Value::Native(y)) if x==y)
    }
    fn has_base(&self, class: Id, target: &str) -> bool {
        match self.heap.get(class) {
            Value::Class { bases, .. } => bases.iter().any(|&b| self.has_base(b, target)),
            Value::Native(name) => name == target,
            _ => false,
        }
    }
    pub(super) fn namedtuple_fields(&self, class: Id) -> Option<Vec<String>> {
        if !self.has_base(class, "typing.NamedTuple") {
            return None;
        }
        let id = self.class_attr(class, "__annotations__")?;
        let Value::Dict(fields) = self.heap.get(id) else {
            return None;
        };
        Some(
            fields
                .iter()
                .filter_map(|&(k, _)| self.str_value(k).ok().map(str::to_owned))
                .collect(),
        )
    }
    pub(super) fn instance_of(&self, value: Id, class: Id) -> bool {
        if let Value::Tuple(classes) = self.heap.get(class) {
            return classes.iter().any(|&c| self.instance_of(value, c));
        }
        if let Value::Instance { class: actual, .. } = self.heap.get(value)
            && self.subclass(*actual, class)
        {
            return true;
        }
        let name = match self.heap.get(class) {
            Value::Class { name, .. } | Value::Native(name) => name.as_str(),
            _ => return false,
        };
        match (name, self.heap.get(value)) {
            ("object", _)
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("str", Value::Str(_))
            | ("bytes", Value::Bytes(_))
            | ("list", Value::List(_))
            | ("dict", Value::Dict(_) | Value::DefaultDict { .. })
            | ("set", Value::Set(_))
            | ("range", Value::Range { .. })
            | ("contextvars.ContextVar", Value::ContextVar { .. })
            | ("contextvars.Context", Value::Context(_))
            | ("contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => true,
            ("tuple", Value::Instance { class, .. }) => self.namedtuple_fields(*class).is_some(),
            (_, Value::Raised(Error { kind, .. })) => self.error_matches(kind, class),
            _ => false,
        }
    }
    pub(super) fn error_matches(&self, kind: &str, class: Id) -> bool {
        match self.heap.get(class) {
            Value::Tuple(classes) => classes.iter().any(|&c| self.error_matches(kind, c)),
            Value::Class { name, .. } | Value::Native(name) => {
                kind == name
                    || name == "BaseException"
                    || (name == "Exception" && kind != "SystemExit")
                    || (name == "ImportError" && kind == "ModuleNotFoundError")
                    || (name == "LookupError" && matches!(kind, "KeyError" | "IndexError"))
                    || (name == "OSError"
                        && matches!(
                            kind,
                            "FileNotFoundError" | "PermissionError" | "IsADirectoryError"
                        ))
                    || self
                        .classes
                        .get(kind)
                        .is_some_and(|&actual| self.subclass(actual, class))
            }
            _ => false,
        }
    }
    pub(super) fn lookup_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        match self.heap.get(obj).clone() {
            Value::Raised(error) if name == "args" => {
                let args = error.args.unwrap_or_else(|| {
                    if error.message.is_empty() {
                        Vec::new()
                    } else {
                        vec![self.text(error.message)]
                    }
                });
                return Ok(self.alloc(Value::Tuple(args)));
            }
            Value::ContextVar { name: var_name, .. } if name == "name" => return Ok(var_name),
            Value::ContextToken { variable, .. } if name == "var" => return Ok(variable),
            Value::ContextToken { old, .. } if name == "old_value" => {
                return Ok(old.unwrap_or(self.token_missing));
            }
            Value::Native(class) if class == "contextvars.Token" && name == "MISSING" => {
                return Ok(self.token_missing);
            }
            Value::Module {
                name: module,
                scope,
            } => {
                if let Some(value) = self.scope_data(scope).names.get(name) {
                    return Ok(*value);
                }
                if stdlib::is_module(&module) {
                    let value = stdlib::attribute(self, &module, name)?;
                    self.bind(scope, name, value);
                    return Ok(value);
                }
            }
            Value::Class { .. } => {
                if let Some(value) = self.class_attr(obj, name) {
                    return Ok(value);
                }
            }
            Value::Instance { class, fields } => {
                if let Some(value) = fields.get(name) {
                    return Ok(*value);
                }
                if let Some(value) = self.class_attr(class, name) {
                    return match self.heap.get(value) {
                        Value::Property(call) => self.call(*call, vec![obj], Vec::new()),
                        Value::Function { .. } | Value::Cached { .. } => {
                            Ok(self.alloc(Value::Method {
                                callable: value,
                                receiver: obj,
                            }))
                        }
                        _ => Ok(value),
                    };
                }
            }
            Value::NamedTuple { names, values } => {
                if let Some(i) = names.iter().position(|n| n == name) {
                    return Ok(values[i]);
                }
            }
            _ => {}
        }
        if let Some(method) = builtins::method_name(self.heap.get(obj), name) {
            let fun = self.alloc(Value::Native(method));
            return Ok(self.alloc(Value::Method {
                callable: fun,
                receiver: obj,
            }));
        }
        Err(Error::new(
            "AttributeError",
            format!("{} has no attribute {name:?}", self.display(obj)),
        ))
    }
    pub(super) fn set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<()> {
        match self.heap.get_mut(obj) {
            Value::Module {
                name: module,
                scope,
            } => {
                if module == "sys" && name == "path" {
                    self.sys_path = value;
                }
                let scope = *scope;
                self.bind(scope, name, value);
            }
            Value::Instance { fields, .. } => {
                fields.insert(name.to_owned(), value);
            }
            Value::Class { scope, .. } => {
                let scope = *scope;
                self.bind(scope, name, value);
            }
            _ => {
                return Err(Error::new(
                    "AttributeError",
                    format!("cannot set attribute {name:?}"),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        callable: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        match self.heap.get(callable).clone() {
            Value::Native(name)
                if builtins::is_builtin(&name)
                    || !name.contains('.')
                    || matches!(
                        name.split('.').next(),
                        Some("str" | "dict" | "list" | "set" | "tuple" | "bytes" | "range")
                    ) =>
            {
                builtins::call(self, &name, &args, &kwargs)
            }
            Value::Native(name) => stdlib::call(self, &name, &args, &kwargs),
            Value::AttrGetter(paths) => self.apply_attrgetter(&paths, &args, &kwargs),
            Value::Method { callable, receiver } => {
                let mut all = Vec::with_capacity(args.len() + 1);
                all.push(receiver);
                all.extend(args);
                self.call(callable, all, kwargs)
            }
            Value::Cached { function, entries } => {
                if !args
                    .iter()
                    .chain(kwargs.iter().map(|(_, v)| v))
                    .all(|&id| builtins::hashable(self, id))
                {
                    return Err(Error::new("TypeError", "unhashable cache argument"));
                }
                let hit = entries.iter().position(|call| {
                    call.args.len() == args.len()
                        && call.args.iter().zip(&args).all(|(&x, &y)| self.equal(x, y))
                        && call.kwargs.len() == kwargs.len()
                        && call
                            .kwargs
                            .iter()
                            .zip(&kwargs)
                            .all(|((kx, x), (ky, y))| kx == ky && self.equal(*x, *y))
                });
                if let Some(i) = hit {
                    let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                        unreachable!()
                    };
                    let hit = entries.remove(i);
                    let result = hit.result;
                    entries.push(hit);
                    return Ok(result);
                }
                let result = self.call(function, args.clone(), kwargs.clone())?;
                let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                    unreachable!()
                };
                if entries.len() == 128 {
                    entries.remove(0);
                }
                entries.push(value::CachedCall {
                    args,
                    kwargs,
                    result,
                });
                Ok(result)
            }
            Value::Class { name, .. } => {
                if [
                    "Exception",
                    "ValueError",
                    "RuntimeError",
                    "OSError",
                    "TypeError",
                ]
                .iter()
                .any(|kind| self.has_base(callable, kind))
                {
                    let message = args.first().map(|&x| self.display(x)).unwrap_or_default();
                    return Ok(self.alloc(Value::Raised(Error::new(&name, message))));
                }
                if let Some(names) = self.namedtuple_fields(callable) {
                    if args.len() > names.len() {
                        return Err(Error::new(
                            "TypeError",
                            format!("too many arguments to {name}"),
                        ));
                    }
                    let mut fields: HashMap<_, _> = names.iter().cloned().zip(args).collect();
                    for (k, v) in kwargs {
                        if !names.contains(&k) || fields.insert(k.clone(), v).is_some() {
                            return Err(Error::new(
                                "TypeError",
                                format!("invalid argument {k:?} to {name}"),
                            ));
                        }
                    }
                    if fields.len() != names.len() {
                        return Err(Error::new("TypeError", format!("missing field for {name}")));
                    }
                    return Ok(self.alloc(Value::Instance {
                        class: callable,
                        fields,
                    }));
                }
                let instance = self.alloc(Value::Instance {
                    class: callable,
                    fields: HashMap::new(),
                });
                if self.class_attr(callable, "__init__").is_some() {
                    let init = self.lookup_attr(instance, "__init__")?;
                    if self.call(init, args, kwargs)? != NONE {
                        return Err(Error::new("TypeError", "__init__ must return None"));
                    }
                } else if !args.is_empty() || !kwargs.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        format!("{name} takes no arguments"),
                    ));
                }
                Ok(instance)
            }
            Value::Function {
                name,
                scope,
                code,
                parameters,
                defaults,
            } => {
                let module = self.scope_data(scope).module.clone();
                let local = self.scope(&module, Some(scope));
                let sig: serde_json::Value = serde_json::from_str(self.str_value(parameters)?)
                    .map_err(|e| Error::new("SystemError", e.to_string()))?;
                let names = |key: &str| -> Vec<String> {
                    sig[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|n| n.as_str().map(str::to_owned))
                        .collect()
                };
                let posonly = names("posonly");
                let pos: Vec<_> = posonly.iter().cloned().chain(names("positional")).collect();
                let kwonly = names("kwonly");
                for (n, &v) in pos.iter().zip(&args) {
                    self.bind(local, n, v);
                }
                if let Some(n) = sig["varargs"].as_str() {
                    let extra = self.alloc(Value::Tuple(
                        args.get(pos.len()..).unwrap_or_default().to_vec(),
                    ));
                    self.bind(local, n, extra);
                } else if args.len() > pos.len() {
                    return Err(Error::new(
                        "TypeError",
                        format!("too many positional arguments to {name}"),
                    ));
                }
                let mut extra = Vec::new();
                for (n, v) in kwargs {
                    if !posonly.contains(&n) && (pos.contains(&n) || kwonly.contains(&n)) {
                        if self.scope_data(local).names.contains_key(&n) {
                            return Err(Error::new(
                                "TypeError",
                                format!("multiple values for {n:?}"),
                            ));
                        }
                        self.bind(local, &n, v);
                    } else if sig["kwargs"].is_string() {
                        let k = self.text(n);
                        extra.push((k, v));
                    } else {
                        return Err(Error::new(
                            "TypeError",
                            format!("unexpected keyword {n:?} in {name}"),
                        ));
                    }
                }
                if let Some(n) = sig["kwargs"].as_str() {
                    let v = self.dict(extra);
                    self.bind(local, n, v);
                }
                for n in pos.iter().chain(&kwonly) {
                    if !self.scope_data(local).names.contains_key(n) {
                        let key = self.text(n);
                        let val = self.get_item(defaults, key).map_err(|_| {
                            Error::new("TypeError", format!("{name} requires argument {n:?}"))
                        })?;
                        self.bind(local, n, val);
                    }
                }
                if let Some(types) = sig["types"].as_object() {
                    for (param, expected) in types {
                        if let Some(&id) = self.scope_data(local).names.get(param) {
                            self.require_scalar(
                                id,
                                expected.as_str().unwrap_or_default(),
                                &format!("{name} argument {param:?}"),
                            )?;
                        }
                    }
                }
                let value = self.execute(code, local)?;
                if let Some(expected) = sig["return_type"].as_str() {
                    self.require_scalar(value, expected, &format!("{name} return value"))?;
                }
                Ok(value)
            }
            _ => Err(Error::new("TypeError", "object is not callable")),
        }
    }
    fn require_scalar(&self, value: Id, expected: &str, context: &str) -> Result<()> {
        if matches!(
            (expected, self.heap.get(value)),
            ("None", Value::None)
                | ("int", Value::Int(_))
                | ("bytes", Value::Bytes(_))
                | ("str", Value::Str(_))
                | ("bool", Value::Bool(_))
        ) {
            Ok(())
        } else {
            Err(Error::new(
                "TypeError",
                format!("{context} requires {expected}"),
            ))
        }
    }

    pub(super) fn class(&mut self, name: Id, bases: Id, body: Id) -> Result<Id> {
        let name = self.str_value(name)?.to_owned();
        let bases = self.items(bases)?;
        let Value::Function {
            scope: parent,
            code,
            ..
        } = self.heap.get(body).clone()
        else {
            return Err(Error::new("TypeError", "invalid class body"));
        };
        let module = self.scope_data(parent).module.clone();
        let scope = self.scope(&module, Some(parent));
        let Value::Scope(body_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        body_scope.class_body = true;
        let annotations = self.dict(Vec::new());
        self.bind(scope, "__annotations__", annotations);
        self.execute(code, scope)?;
        let class = self.alloc(Value::Class {
            name: name.clone(),
            scope,
            bases,
        });
        self.classes.insert(name, class);
        Ok(class)
    }
}
