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

    /// Special methods are looked up on the type, even if the instance shadows them.
    fn slot(&self, object: Id, name: &str) -> Option<Id> {
        let Value::Instance { class, .. } = self.heap.get(object) else {
            return None;
        };
        self.class_attr(*class, name)
    }

    pub(super) fn bound_slot(&mut self, object: Id, name: &str) -> Result<Option<Id>> {
        let Value::Instance { class, .. } = *self.heap.get(object) else {
            return Ok(None);
        };
        self.class_attr(class, name)
            .map(|slot| self.bind_attribute(slot, object, class))
            .transpose()
    }

    pub(super) fn callable(&self, value: Id) -> bool {
        matches!(
            self.heap.get(value),
            Value::Class { .. }
                | Value::Function { .. }
                | Value::AttrGetter(_)
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::Native(_)
                | Value::IoMethod { .. }
                | Value::Method { .. }
        ) || self.slot(value, "__call__").is_some()
    }

    fn data_descriptor(&self, descriptor: Id) -> bool {
        matches!(self.heap.get(descriptor), Value::Property(_))
            || self.slot(descriptor, "__set__").is_some()
            || self.slot(descriptor, "__delete__").is_some()
    }

    pub(super) fn bind_attribute(&mut self, value: Id, receiver: Id, owner: Id) -> Result<Id> {
        if let Some(get) = self.slot(value, "__get__") {
            return self.call(get, vec![value, receiver, owner], Vec::new());
        }
        if receiver != NONE {
            let method = match self.heap.get(value) {
                Value::Property(get) => return self.call(*get, vec![receiver], Vec::new()),
                Value::Function { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::IoMethod { .. } => true,
                Value::Native(name) => name == "object.__setattr__",
                _ => false,
            };
            if method {
                return Ok(self.alloc(Value::Method {
                    callable: value,
                    receiver,
                }));
            }
        }
        Ok(value)
    }

    fn instance_attribute(
        &mut self,
        object: Id,
        class: Id,
        field: Option<Id>,
        name: &str,
    ) -> Result<Id> {
        let member = self.class_attr(class, name);
        let attribute = match (member, field) {
            (Some(member), _)
                if self.data_descriptor(member)
                    && (self.slot(member, "__get__").is_some()
                        || matches!(self.heap.get(member), Value::Property(_))) =>
            {
                self.bind_attribute(member, object, class)
            }
            (_, Some(field)) => Ok(field),
            (Some(member), None) => self.bind_attribute(member, object, class),
            (None, None) => Err(Error::new(
                "AttributeError",
                format!("{} has no attribute {name:?}", self.display(object)),
            )),
        };
        match attribute {
            Err(error) if error.kind == "AttributeError" => {
                let Some(hook) = self.class_attr(class, "__getattr__") else {
                    return Err(error);
                };
                let callable = self.bind_attribute(hook, object, class)?;
                let name = self.text(name);
                self.call(callable, vec![name], Vec::new())
            }
            value => value,
        }
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
    pub(super) fn builtin_type_name(&self, obj: Id) -> Option<&str> {
        Some(match self.heap.get(obj) {
            Value::None
            | Value::Ellipsis
            | Value::Bool(_)
            | Value::Float(_)
            | Value::Int(_)
            | Value::Str(_)
            | Value::Bytes(_)
            | Value::List(_)
            | Value::Dict(_)
            | Value::Set(_)
            | Value::Range { .. }
            | Value::Tuple(_)
            | Value::Class { .. }
            | Value::Function { .. }
            | Value::Module { .. } => self.type_name(obj),
            Value::ByteArray(_) => "bytearray",
            Value::Property(_) => "property",
            Value::Method { callable, .. } => match self.heap.get(*callable) {
                Value::Function { .. } => "method",
                Value::Native(_) => "builtin_function_or_method",
                _ => return None,
            },
            Value::Native(name) if self.builtins.get(name) == Some(&obj) => {
                if builtins::is_builtin_type(name)
                    || matches!(
                        name.as_str(),
                        "NoneType"
                            | "ellipsis"
                            | "float"
                            | "function"
                            | "module"
                            | "method"
                            | "builtin_function_or_method"
                            | "operator.attrgetter"
                    )
                {
                    "type"
                } else if builtins::is_builtin(name) {
                    "builtin_function_or_method"
                } else {
                    return None;
                }
            }
            _ => return None,
        })
    }

    /// Return the actual class independently of an instance's Python-level __class__ attribute.
    pub(super) fn exact_type(&mut self, obj: Id) -> Result<Id> {
        match self.heap.get(obj) {
            Value::Instance { class, .. } => Ok(*class),
            Value::Raised(error) => {
                let name = error.kind.clone();
                Ok(self
                    .classes
                    .get(&name)
                    .copied()
                    .unwrap_or_else(|| self.builtin(&name)))
            }
            Value::Calendar { value, .. } => {
                let class = format!("datetime.{}", value.class());
                Ok(self.builtin(&class))
            }
            _ => {
                let name = self.builtin_type_name(obj).ok_or_else(|| {
                    Error::new(
                        "NotImplementedError",
                        "type() is unsupported for this native object",
                    )
                })?;
                if let Some(&class) = self.builtins.get(name) {
                    return Ok(class);
                }
                let name = name.to_owned();
                Ok(self.builtin(&name))
            }
        }
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
            ("bytearray", Value::ByteArray(_))
            | ("object", _)
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("float", Value::Float(_))
            | ("str", Value::Str(_))
            | ("bytes", Value::Bytes(_))
            | ("hmac.HMAC", Value::Hmac(_))
            | ("list", Value::List(_))
            | ("dict", Value::Dict(_) | Value::DefaultDict { .. })
            | ("set", Value::Set(_))
            | ("range", Value::Range { .. })
            | ("selectors.PollSelector", Value::Selector(_))
            | ("contextvars.ContextVar", Value::ContextVar { .. })
            | ("contextvars.Context", Value::Context(_))
            | ("functools.partial", Value::Partial { .. })
            | ("contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => true,
            ("tuple", Value::Instance { class, .. }) => self.namedtuple_fields(*class).is_some(),
            (class, Value::Calendar { value, .. }) => {
                class.strip_prefix("datetime.").is_some_and(|c| {
                    c == value.class() || (c == "date" && value.class() == "datetime")
                })
            }
            (_, Value::Raised(Error { kind, .. })) => self.error_matches(kind, class),
            _ => {
                matches!(self.heap.get(class), Value::Native(_))
                    && self.builtin_type_name(value) == Some(name)
            }
        }
    }
    pub(super) fn error_matches(&self, kind: &str, class: Id) -> bool {
        match self.heap.get(class) {
            Value::Tuple(classes) => classes.iter().any(|&c| self.error_matches(kind, c)),
            Value::Class { name, .. } | Value::Native(name) => {
                kind == name
                    || name == "BaseException"
                    || (name == "Exception" && kind != "SystemExit")
                    || (name == "RuntimeError" && kind == "RecursionError")
                    || (name == "ValueError" && kind == "JSONDecodeError")
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
            Value::Calendar { value, tzinfo } => {
                return stdlib::datetime::attribute(self, obj, &value, tzinfo, name);
            }
            Value::Native(class) if class == "datetime.datetime" && name == "fromtimestamp" => {
                return Ok(self.builtin("datetime.datetime.fromtimestamp"));
            }
            Value::Native(class) if class == "datetime.timezone" && name == "utc" => {
                return Ok(stdlib::datetime::utc(self));
            }
            Value::Partial {
                function,
                args,
                keywords,
            } => match name {
                "func" => return Ok(function),
                "args" => return Ok(args),
                "keywords" => return Ok(keywords),
                _ => {}
            },
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
            Value::Hash(hash) => match name {
                "name" => return Ok(self.text(hash.borrow().name())),
                "digest_size" => return Ok(self.int(hash.borrow().digest_size() as i64)),
                "block_size" => return Ok(self.int(64)),
                _ => {}
            },
            Value::Hmac(mac) => match name {
                "digest_size" => return Ok(mac.digest_size),
                "block_size" => return Ok(mac.block_size),
                "name" => {
                    let name = self.lookup_attr(mac.inner, "name")?;
                    return Ok(self.text(format!("hmac-{}", self.str_value(name)?)));
                }
                _ => {}
            },
            Value::ContextVar { name: var_name, .. } if name == "name" => return Ok(var_name),
            Value::ContextToken { variable, .. } if name == "var" => return Ok(variable),
            Value::ContextToken { old, .. } if name == "old_value" => {
                return Ok(old.unwrap_or(self.token_missing));
            }
            Value::Native(builtin)
                if name == "__name__" && self.builtins.get(&builtin) == Some(&obj) =>
            {
                return Ok(self.text(builtin.rsplit('.').next().unwrap_or(&builtin)));
            }
            Value::Native(class) if class == "contextvars.Token" && name == "MISSING" => {
                return Ok(self.token_missing);
            }
            Value::Native(class) if class == "object" && name == "__setattr__" => {
                return Ok(self.builtin("object.__setattr__"));
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
            Value::Class {
                name: class_name, ..
            } => {
                if name == "__name__" {
                    return Ok(self.text(class_name));
                }
                if let Some(value) = self.class_attr(obj, name) {
                    return self.bind_attribute(value, NONE, obj);
                }
            }
            Value::Instance { class, fields } => {
                return self.instance_attribute(obj, class, fields.get(name).copied(), name);
            }
            Value::NamedTuple {
                class,
                names,
                values,
            } => {
                if let Some(i) = names.iter().position(|n| n == name) {
                    return Ok(values[i]);
                }
                if class == Some("time.struct_time") && matches!(name, "tm_zone" | "tm_gmtoff") {
                    return Ok(NONE);
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
        if let Some(hook) = self.bound_slot(obj, "__setattr__")? {
            let name = self.text(name);
            self.call(hook, vec![name, value], Vec::new())?;
            return Ok(());
        }
        self.default_set_attr(obj, name, value)
    }

    pub(super) fn default_set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<()> {
        if let Some(descriptor) = self.slot(obj, name)
            && self.data_descriptor(descriptor)
        {
            if let Some(setter) = self.slot(descriptor, "__set__") {
                self.call(setter, vec![descriptor, obj, value], Vec::new())?;
                return Ok(());
            }
            return Err(Error::new(
                "AttributeError",
                format!("attribute {name:?} cannot be set"),
            ));
        }
        if let Value::Instance { class, .. } = self.heap.get(obj)
            && self
                .namedtuple_fields(*class)
                .is_some_and(|fields| fields.iter().any(|field| field == name))
        {
            return Err(Error::new("AttributeError", "can't set attribute"));
        }
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
                let attr = self.text(name);
                return Err(Error::new(
                    "AttributeError",
                    format!(
                        "'{}' object has no attribute {} and no __dict__ for setting new attributes",
                        self.type_name(obj),
                        self.repr(attr)
                    ),
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
                        Some(
                            "str"
                                | "dict"
                                | "list"
                                | "set"
                                | "tuple"
                                | "bytes"
                                | "bytearray"
                                | "range"
                                | "function"
                                | "object"
                        )
                    ) =>
            {
                builtins::call(self, &name, &args, &kwargs)
            }
            Value::Native(name) => stdlib::call(self, &name, &args, &kwargs),
            Value::IoMethod { owner, name } => stdlib::io::call(self, owner, name, &args, &kwargs),
            Value::Partial {
                function,
                args: bound,
                keywords,
            } => {
                let mut positional = self.items(bound)?;
                positional.extend(args);
                let mut merged = self.keyword_args(keywords)?;
                for (key, value) in kwargs {
                    if let Some((_, previous)) = merged.iter_mut().find(|(k, _)| k == &key) {
                        *previous = value;
                    } else {
                        merged.push((key, value));
                    }
                }
                self.call(function, positional, merged)
            }
            Value::Method { callable, receiver } => {
                let mut all = Vec::with_capacity(args.len() + 1);
                all.push(receiver);
                all.extend(args);
                self.call(callable, all, kwargs)
            }
            Value::AttrGetter(paths) => self.apply_attrgetter(&paths, &args, &kwargs),
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
            _ => {
                if let Some(method) = self.bound_slot(callable, "__call__")? {
                    self.call(method, args, kwargs)
                } else {
                    Err(Error::new(
                        "TypeError",
                        format!("'{}' object is not callable", self.type_name(callable)),
                    ))
                }
            }
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
        body_scope.class_body = Some(Vec::new());
        let annotations = self.dict(Vec::new());
        self.bind(scope, "__annotations__", annotations);
        self.execute(code, scope)?;
        let class = self.alloc(Value::Class {
            name: name.clone(),
            scope,
            bases,
        });
        let Value::Scope(class_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        // The class body can read the enclosing module's docstring before the
        // type supplies a default for an undocumented class.
        class_scope
            .names
            .entry("__doc__".to_owned())
            .or_insert(NONE);
        let members = class_scope
            .class_body
            .take()
            .unwrap()
            .into_iter()
            .map(|name| (class_scope.names[&name], name))
            .collect::<Vec<_>>();
        // A hook may replace an attribute and execute code that triggers collection.
        for &(value, _) in &members {
            self.heap.keep(value);
        }
        for (value, name) in members {
            if let Some(set_name) = self.slot(value, "__set_name__") {
                let name = self.text(name);
                self.call(set_name, vec![value, class, name], Vec::new())?;
            }
        }
        self.classes.insert(name, class);
        Ok(class)
    }
}
