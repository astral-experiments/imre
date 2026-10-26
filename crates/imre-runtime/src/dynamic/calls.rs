use super::*;

impl Context {
    pub(super) fn class_attr(&self, class: Id, name: &str) -> Option<Id> {
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return self.own_class_attr(class, name);
        };
        std::iter::once(&class)
            .chain(ancestors)
            .find_map(|&base| self.own_class_attr(base, name))
    }

    pub(super) fn own_class_attr(&self, class: Id, name: &str) -> Option<Id> {
        match self.heap.get(class) {
            Value::Class { scope, .. } => self.scope_data(*scope).names.get(name).copied(),
            Value::Native(kind) if matches!(kind.as_str(), "dict" | "int" | "str") => {
                self.builtins.get(&format!("{kind}.{name}")).copied()
            }
            _ => None,
        }
    }
    /// Special methods are looked up on the type, even if the instance shadows them.
    pub(super) fn slot(&self, object: Id, name: &str) -> Option<Id> {
        let Value::Instance { class, .. } = self.heap.get(object) else {
            return None;
        };
        self.class_attr(*class, name)
    }

    /// Dict copying uses raw entries for a dict with its original iterator.
    /// An overridden iterator opts into the mapping protocol, including key lookup.
    pub(super) fn for_each_mapping_item(
        &mut self,
        input: Id,
        mut visit: impl FnMut(&mut Self, Id, Id) -> Result<()>,
    ) -> Result<bool> {
        if self.slot(input, "__iter__").is_none_or(
            |slot| matches!(self.heap.get(slot), Value::Native(name) if name == "dict.__iter__"),
        ) && let Some(entries) = self.dict_entries(input).map(<[_]>::to_vec)
        {
            for (key, value) in entries {
                visit(self, key, value)?;
            }
            return Ok(true);
        }
        if matches!(
            self.heap.get(input),
            Value::List(_)
                | Value::Tuple(_)
                | Value::Set { .. }
                | Value::Str(_)
                | Value::Bytes(_)
                | Value::Range { .. }
                | Value::Iterator { .. }
        ) {
            return Ok(false);
        }
        let keys = match self.lookup_attr(input, "keys") {
            Ok(keys) => keys,
            Err(error) if error.kind == "AttributeError" => return Ok(false),
            Err(error) => return Err(error),
        };
        let result = self.call(keys, Vec::new(), Vec::new())?;
        for key in self.items(result)? {
            let value = self.get_item(input, key)?;
            visit(self, key, value)?;
        }
        Ok(true)
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
                | Value::StaticMethod { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::Native(_)
                | Value::CodecDecoder(_)
                | Value::IoMethod { .. }
                | Value::Method { .. }
        ) || self.slot(value, "__call__").is_some()
    }

    fn native_data_descriptor(&self, descriptor: Id) -> bool {
        matches!(
            self.heap.get(descriptor),
            Value::Property(_) | Value::ReadOnlyDescriptor { .. }
        )
    }

    fn data_descriptor(&self, descriptor: Id) -> bool {
        self.native_data_descriptor(descriptor)
            || self.slot(descriptor, "__set__").is_some()
            || self.slot(descriptor, "__delete__").is_some()
    }

    pub(super) fn bind_attribute(&mut self, value: Id, receiver: Id, owner: Id) -> Result<Id> {
        if let Some(get) = self.slot(value, "__get__") {
            return self.call(get, vec![value, receiver, owner], Vec::new());
        }
        if let Value::StaticMethod { callable, .. } = *self.heap.get(value) {
            return Ok(callable);
        }
        if receiver != NONE {
            let method = match self.heap.get(value) {
                Value::Property(property) => return property.read(self, receiver),
                Value::ReadOnlyDescriptor { get, .. } => {
                    return self.call(*get, vec![receiver], Vec::new());
                }
                Value::Function { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::IoMethod { .. } => true,
                Value::Native(name) => {
                    matches!(
                        name.as_str(),
                        "ast.AST.__init__"
                            | "object.__getattribute__"
                            | "object.__setattr__"
                            | "object.__delattr__"
                            | "object.__init__"
                            | "int.to_bytes"
                            | "list.pop"
                    ) || name.starts_with("dict.")
                        || name
                            .strip_prefix("str.")
                            .is_some_and(|method| builtins::STR_METHODS.contains(&method))
                }
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
        match (member, field) {
            (Some(member), _)
                if self.data_descriptor(member)
                    && (self.slot(member, "__get__").is_some()
                        || self.native_data_descriptor(member)) =>
            {
                self.bind_attribute(member, object, class)
            }
            // Without a Python class member, object.__class__ is a data descriptor.
            (None, _) if name == "__class__" => Ok(class),
            (_, Some(field)) => Ok(field),
            (Some(member), None) => self.bind_attribute(member, object, class),
            (None, None) if matches!(name, "__getattribute__" | "__delattr__") => {
                let method = self.builtin(&format!("object.{name}"));
                self.bind_attribute(method, object, class)
            }
            (None, None) => self.builtin_attribute(object, name),
        }
    }

    pub(super) fn python_subclass_check(&self, child: Id, parent: Id) -> Result<bool> {
        if let Value::Tuple(classes) = self.heap.get(parent) {
            for &class in classes {
                if self.python_subclass_check(child, class)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        let is_class = |id| self.builtin_type_name(id) == Some("type");
        if !is_class(child) {
            return Err(Error::new(
                "TypeError",
                "issubclass() arg 1 must be a class",
            ));
        }
        if !is_class(parent) {
            return Err(Error::new(
                "TypeError",
                "issubclass() arg 2 must be a class, a tuple of classes, or a union",
            ));
        }
        Ok(self.subclass(child, parent))
    }

    pub(super) fn subclass(&self, class: Id, target: Id) -> bool {
        if self.equal_class(class, target) {
            return true;
        }
        if let Value::Class { ancestors, .. } = self.heap.get(class) {
            return ancestors.iter().any(|&base| {
                self.equal_class(base, target)
                    || (matches!(self.heap.get(base), Value::Native(_))
                        && self.subclass(base, target))
            });
        }
        if let (Value::Native(child), Value::Native(parent)) =
            (self.heap.get(class), self.heap.get(target))
        {
            if parent == "object" {
                return self.builtin_type_name(class) == Some("type");
            }
            if let (Some(child), Some(parent)) = (
                builtins::exception_kind(child),
                builtins::exception_kind(parent),
            ) {
                return crate::objects::exceptions::imre_builtin_exception_subclass(
                    child as u32,
                    parent as u32,
                );
            }
            return child == "bool" && parent == "int";
        }
        false
    }
    fn equal_class(&self, a: Id, b: Id) -> bool {
        a == b
            || matches!((self.heap.get(a),self.heap.get(b)),(Value::Native(x),Value::Native(y)) if x==y)
    }
    fn has_base(&self, class: Id, target: &str) -> bool {
        match self.heap.get(class) {
            Value::Class { ancestors, .. } => ancestors
                .iter()
                .any(|&b| matches!(self.heap.get(b), Value::Native(name) if name == target)),
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
            | Value::Set { .. }
            | Value::Range { .. }
            | Value::Tuple(_)
            | Value::Class { .. }
            | Value::Function { .. }
            | Value::Traceback { .. }
            | Value::PythonFrame { .. }
            | Value::Code(_)
            | Value::Module { .. } => self.type_name(obj),
            Value::ByteArray(_) => "bytearray",
            Value::Zip { .. } => "zip",
            Value::StaticMethod { .. } => "staticmethod",
            Value::Property(_) => "property",
            Value::Super { .. } => "super",
            Value::ReadOnlyDescriptor { .. } => "getset_descriptor",
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
                            | "code"
                            | "function"
                            | "module"
                            | "method"
                            | "builtin_function_or_method"
                            | "getset_descriptor"
                            | "operator.attrgetter"
                            | "traceback"
                            | "frame"
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
            Value::NamedTuple {
                class: Some("time.struct_time"),
                ..
            } => Ok(self.builtin("time.struct_time")),
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
            | ("socket.socket", Value::Socket(_))
            | ("list", Value::List(_))
            | ("dict", Value::Dict(_) | Value::DefaultDict { .. })
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | ("range", Value::Range { .. })
            | ("selectors.PollSelector", Value::Selector(_))
            | ("contextvars.ContextVar", Value::ContextVar { .. })
            | ("contextvars.Context", Value::Context(_))
            | ("functools.partial", Value::Partial { .. })
            | ("contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | ("tuple", Value::Tuple(_) | Value::NamedTuple { .. }) => true,
            (
                "urllib.parse.SplitResult",
                Value::NamedTuple {
                    class: Some("SplitResult"),
                    ..
                },
            )
            | (
                "urllib.parse.SplitResultBytes",
                Value::NamedTuple {
                    class: Some("SplitResultBytes"),
                    ..
                },
            ) => true,
            ("tuple", Value::Instance { class, .. }) => self.namedtuple_fields(*class).is_some(),
            (
                "time.struct_time",
                Value::NamedTuple {
                    class: Some("time.struct_time"),
                    ..
                },
            ) => true,
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
                    || (name == "ValueError"
                        && matches!(kind, "JSONDecodeError" | "binascii.Error"))
                    || (name == "ImportError" && kind == "ModuleNotFoundError")
                    || (name == "LookupError" && matches!(kind, "KeyError" | "IndexError"))
                    || (matches!(name.as_str(), "OSError" | "ConnectionError")
                        && matches!(
                            kind,
                            "BrokenPipeError"
                                | "ConnectionRefusedError"
                                | "ConnectionResetError"
                                | "ConnectionAbortedError"
                        ))
                    || (name == "OSError"
                        && matches!(
                            kind,
                            "FileNotFoundError"
                                | "PermissionError"
                                | "IsADirectoryError"
                                | "NotADirectoryError"
                                | "BlockingIOError"
                                | "TimeoutError"
                                | "socket.gaierror"
                        ))
                    || self
                        .classes
                        .get(kind)
                        .is_some_and(|&actual| self.subclass(actual, class))
            }
            _ => false,
        }
    }
    fn missing_attribute(&self, obj: Id, name: &str) -> Error {
        Error::new(
            "AttributeError",
            format!("'{}' object has no attribute '{name}'", self.type_name(obj)),
        )
    }

    /// Match an attribute lookup failure using the ordinary exception hierarchy.
    pub(super) fn is_attribute_error(&self, error: &Error) -> bool {
        error.kind == "AttributeError"
            || self
                .builtins
                .get("AttributeError")
                .is_some_and(|&base| self.error_matches(&error.kind, base))
    }

    fn attribute_hook(&mut self, hook: Id, obj: Id, class: Id, name: &str) -> Result<Id> {
        let callable = self.bind_attribute(hook, obj, class)?;
        let key = self.text(name);
        self.call(callable, vec![key], Vec::new())
    }

    pub(super) fn lookup_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        let Value::Instance { class, .. } = *self.heap.get(obj) else {
            return self.default_get_attr(obj, name);
        };
        let attribute = if let Some(hook) = self.class_attr(class, "__getattribute__") {
            self.attribute_hook(hook, obj, class, name)
        } else {
            self.default_get_attr(obj, name)
        };
        match attribute {
            Err(error) if self.is_attribute_error(&error) => {
                let Some(hook) = self.class_attr(class, "__getattr__") else {
                    return Err(error);
                };
                self.attribute_hook(hook, obj, class, name)
            }
            result => result,
        }
    }

    /// Base lookup bypasses Python's __getattribute__ and __getattr__ hooks.
    pub(super) fn default_get_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        if let Value::Instance { class, fields, .. } = self.heap.get(obj) {
            return self.instance_attribute(obj, *class, fields.get(name).copied(), name);
        }
        if name == "__class__" {
            return self.exact_type(obj);
        }
        match self.heap.get(obj).clone() {
            Value::Raised(error)
                if matches!(
                    name,
                    "__traceback__" | "__context__" | "__cause__" | "__suppress_context__"
                ) =>
            {
                match name {
                    "__traceback__" => return self.exception_traceback(obj),
                    "__context__" => {
                        return Ok(error.trace.as_ref().and_then(|t| t.context).unwrap_or(NONE));
                    }
                    "__cause__" => {
                        return Ok(error.trace.as_ref().and_then(|t| t.cause).unwrap_or(NONE));
                    }
                    "__suppress_context__" => {
                        return Ok(if error.trace.as_ref().is_some_and(|t| t.cause.is_some()) {
                            TRUE
                        } else {
                            FALSE
                        });
                    }
                    _ => unreachable!(),
                }
            }
            Value::Traceback {
                frame,
                next,
                lineno,
                ..
            } => match name {
                "tb_frame" => return Ok(frame),
                "tb_next" => return Ok(next),
                "tb_lineno" => return Ok(self.int(lineno)),
                _ => {}
            },
            Value::PythonFrame { code, .. } if name == "f_code" => return Ok(code),
            Value::PythonFrame { lineno, .. } if name == "f_lineno" => {
                return Ok(self.int(i64::from(lineno)));
            }
            Value::Native(class) if matches!(class.as_str(), "traceback" | "frame") => match name {
                "__name__" | "__qualname__" => return Ok(self.text(class)),
                "__module__" => return Ok(self.text("builtins")),
                _ => {}
            },
            Value::StaticMethod {
                callable,
                dictionary,
            } => {
                if let Some(value) =
                    super::static_method::attribute(self, callable, dictionary, name)?
                {
                    return Ok(value);
                }
            }
            Value::None if name == "__doc__" => {
                return Ok(self.text("The type of the None singleton."));
            }
            Value::Property(property) => {
                if let Some(attribute) = property.attribute(self, name)? {
                    return Ok(attribute);
                }
            }
            Value::Native(class) if class == "staticmethod" && name == "__get__" => {
                return Ok(self.builtin("staticmethod.__get__"));
            }
            Value::Native(class) if class == "property" && name == "setter" => {
                return Ok(self.builtin("property.setter"));
            }
            Value::Code(code) => match name {
                "co_name" => return Ok(code.name),
                "co_filename" => return Ok(code.filename),
                "co_firstlineno" => return Ok(self.int(i64::from(code.first_line))),
                _ => {}
            },
            Value::Function { code, .. } if name == "__code__" => return Ok(code),
            Value::Function { scope, fields, .. } => {
                if name == "__globals__" {
                    return Ok(self.module_globals(scope));
                }
                if let Some(value) = self.function_attr(obj, fields, name)? {
                    return Ok(value);
                }
            }
            Value::Method { callable, receiver } => match name {
                "__func__" => return Ok(callable),
                "__self__" => return Ok(receiver),
                _ if matches!(self.heap.get(callable), Value::Function { .. }) => {
                    return self.lookup_attr(callable, name);
                }
                _ => {}
            },
            Value::RegexPattern(pattern) if name == "flags" => return Ok(self.int(pattern.flags)),
            Value::RegexPattern(pattern) if name == "pattern" => {
                return Ok(self.text(&pattern.source));
            }
            Value::Calendar { value, tzinfo } => {
                return stdlib::datetime::attribute(self, obj, &value, tzinfo, name);
            }
            Value::Native(class)
                if class == "datetime.datetime" && matches!(name, "fromtimestamp" | "combine") =>
            {
                return Ok(self.builtin(&format!("datetime.datetime.{name}")));
            }
            Value::Native(class) if class == "datetime.time" => match name {
                "__name__" | "__qualname__" => return Ok(self.text("time")),
                "__module__" => return Ok(self.text("datetime")),
                _ => {}
            },
            Value::Native(class) if class == "datetime.timezone" && name == "utc" => {
                return Ok(stdlib::datetime::utc(self));
            }
            Value::Super {
                start,
                receiver,
                owner,
            } => return self.super_attribute(start, receiver, owner, name),
            Value::Partial {
                coroutine_marker: Some(mark),
                ..
            } if name == "_is_coroutine_marker" => {
                return Ok(mark);
            }
            Value::Partial {
                function,
                args,
                keywords,
                ..
            } => match name {
                "func" => return Ok(function),
                "args" => return Ok(args),
                "keywords" => return Ok(keywords),
                _ => {}
            },
            Value::File(handle) if name == "closed" => {
                return Ok(if handle.borrow().is_none() {
                    TRUE
                } else {
                    FALSE
                });
            }
            Value::Native(class) if class == "socket.socket" && name == "__name__" => {
                return Ok(self.text("socket"));
            }
            Value::Socket(_) if name == "family" => return Ok(self.int(i64::from(libc::AF_INET))),
            Value::Socket(_) if name == "type" => return Ok(self.int(i64::from(libc::SOCK_STREAM))),
            Value::TextFile(file) if name == "closed" => {
                return Ok(if file.closed() { TRUE } else { FALSE });
            }
            Value::Raised(Error {
                os_args: Some(args),
                ..
            }) => match name {
                "errno" => return Ok(self.int(args.errno)),
                "strerror" => return Ok(self.text(args.description)),
                "args" => {
                    let fields = vec![self.int(args.errno), self.text(args.description)];
                    return Ok(self.alloc(Value::Tuple(fields)));
                }
                "filename" | "filename2" => {
                    if let Some([first, second]) = args.text_paths {
                        let path = if name == "filename" { first } else { second };
                        return Ok(path.map_or(NONE, |path| self.text(path)));
                    }
                }
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
            Value::CodecInfo { entry, decode } => match name {
                "name" => return Ok(self.text(entry.codec().name())),
                "decode" => return Ok(decode),
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
            Value::Native(class) if class == "bool" && name == "to_bytes" => {
                return Ok(self.builtin("int.to_bytes"));
            }
            Value::Native(class) if class == "list" && name == "pop" => {
                return Ok(self.builtin("list.pop"));
            }
            Value::Native(class)
                if class == "dict"
                    || (class == "int" && name == "to_bytes")
                    || (class == "str" && builtins::STR_METHODS.contains(&name)) =>
            {
                if let Some(method) = self.own_class_attr(obj, name) {
                    return Ok(method);
                }
            }
            Value::Native(class)
                if class == "object"
                    && matches!(
                        name,
                        "__getattribute__" | "__setattr__" | "__delattr__" | "__init__"
                    ) =>
            {
                return Ok(self.builtin(&format!("object.{name}")));
            }
            Value::Native(class) if class == "str" && name == "__new__" => {
                return Ok(self.builtin("str.__new__"));
            }
            Value::Module {
                name: module,
                scope,
            } => {
                if name == "__dict__" {
                    return Ok(self.module_globals(scope));
                }
                if let Some(value) = self.scope_name(scope, name) {
                    return Ok(value);
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
                if name == "__new__" && self.has_base(obj, "str") {
                    return Ok(self.builtin("str.__new__"));
                }
            }
            Value::NamedTuple {
                class,
                names,
                values,
                extras,
            } => {
                if let Some(i) = names.iter().position(|n| n == name) {
                    return Ok(values[i]);
                }
                if let Some((_, value)) = extras.iter().find(|(key, _)| *key == name) {
                    return Ok(*value);
                }
                if class == Some("time.struct_time") && name == "__class__" {
                    return Ok(self.builtin("time.struct_time"));
                }
                if matches!(class, Some("SplitResult" | "SplitResultBytes"))
                    && let Some(property) = stdlib::urllib::property(self, values[1], name)?
                {
                    return Ok(property);
                }
            }
            _ => {}
        }
        self.builtin_attribute(obj, name)
    }

    fn builtin_attribute(&mut self, obj: Id, name: &str) -> Result<Id> {
        let base = self.string_storage(obj).unwrap_or(obj);
        if let Some(method) = builtins::method_name(self.heap.get(base), name) {
            let fun = self.alloc(Value::Native(method));
            return Ok(self.alloc(Value::Method {
                callable: fun,
                receiver: obj,
            }));
        }
        Err(self.missing_attribute(obj, name))
    }
    pub(super) fn delete_attr(&mut self, obj: Id, name: &str) -> Result<()> {
        if let Some(hook) = self.bound_slot(obj, "__delattr__")? {
            let name = self.text(name);
            self.call(hook, vec![name], Vec::new())?;
            return Ok(());
        }
        self.default_delete_attr(obj, name)
    }

    pub(super) fn delete_dict_attribute(
        &mut self,
        obj: Id,
        dictionary: Id,
        name: &str,
    ) -> Result<()> {
        let key = self.text(name);
        match self.delete_item(dictionary, key) {
            Err(error) if error.kind == "KeyError" => Err(self.missing_attribute(obj, name)),
            result => result,
        }
    }

    /// Bypass the object's deletion hook, while honoring data descriptors on its class.
    pub(super) fn default_delete_attr(&mut self, obj: Id, name: &str) -> Result<()> {
        if let Some(descriptor) = self.slot(obj, name)
            && self.data_descriptor(descriptor)
        {
            match *self.heap.get(descriptor) {
                Value::Property(property) => return property.remove(self, obj),
                Value::ReadOnlyDescriptor { owner, name, .. } => {
                    return Err(Error::new(
                        "AttributeError",
                        format!("attribute '{name}' of '{owner}' objects is not writable"),
                    ));
                }
                _ => {}
            }
            let delete = self
                .bound_slot(descriptor, "__delete__")?
                .ok_or_else(|| Error::new("AttributeError", "__delete__"))?;
            self.call(delete, vec![obj], Vec::new())?;
            return Ok(());
        }
        if name == "__class__" {
            return Err(Error::new("TypeError", "can't delete __class__ attribute"));
        }
        match *self.heap.get(obj) {
            Value::Function { fields, .. } => self.delete_function_attr(obj, fields, name),
            Value::Property(_) if matches!(name, "fget" | "fset" | "fdel") => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
            Value::Property(_) if matches!(name, "__doc__" | "__name__") => {
                let Value::Property(property) = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                if name == "__doc__" {
                    property.doc = NONE;
                } else {
                    property.name = None;
                }
                Ok(())
            }
            Value::StaticMethod { dictionary, .. } => {
                super::static_method::update_attribute(self, obj, dictionary, name, None)
            }
            Value::Module { scope, .. } => {
                if name == "__dict__" {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                let key = self.text(name);
                if self.remove_binding(scope, name, key)? {
                    Ok(())
                } else {
                    Err(self.missing_attribute(obj, name))
                }
            }
            Value::Class {
                scope,
                name: ref class,
                ..
            } => {
                let error = match name {
                    "__name__" | "__qualname__" | "__bases__" | "__type_params__"
                    | "__module__" | "__doc__" => Error::new(
                        "TypeError",
                        format!("cannot delete '{name}' attribute of immutable type '{class}'"),
                    ),
                    "__dict__" | "__mro__" => Error::new(
                        "AttributeError",
                        format!("attribute '{name}' of 'type' objects is not writable"),
                    ),
                    "__base__" => Error::new("AttributeError", "readonly attribute"),
                    "__annotate__" => {
                        Error::new("TypeError", "cannot delete __annotate__ attribute")
                    }
                    _ => {
                        let key = self.text(name);
                        if self.remove_binding(scope, name, key)? {
                            return Ok(());
                        }
                        let Value::Class { name: class, .. } = self.heap.get(obj) else {
                            unreachable!()
                        };
                        Error::new(
                            "AttributeError",
                            format!("type object '{class}' has no attribute '{name}'"),
                        )
                    }
                };
                Err(error)
            }
            Value::Instance { class, .. } if self.builtins.get("object") != Some(&class) => {
                if self
                    .namedtuple_fields(class)
                    .is_some_and(|fields| fields.iter().any(|field| field == name))
                {
                    return Err(Error::new("AttributeError", "can't delete attribute"));
                }
                let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                if name == "__dict__" {
                    fields.clear();
                    return Ok(());
                }
                fields
                    .remove(name)
                    .map(|_| ())
                    .ok_or_else(|| self.missing_attribute(obj, name))
            }
            Value::Native(ref class) if self.builtin_type_name(obj) == Some("type") => {
                Err(Error::new(
                    "TypeError",
                    format!("cannot set '{name}' attribute of immutable type '{class}'"),
                ))
            }
            _ => Err(Error::new(
                "AttributeError",
                format!(
                    "'{}' object has no attribute '{name}' and no __dict__ for setting new attributes",
                    self.type_name(obj)
                ),
            )),
        }
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
        if let Value::StaticMethod { dictionary, .. } = *self.heap.get(obj) {
            return super::static_method::update_attribute(
                self,
                obj,
                dictionary,
                name,
                Some(value),
            );
        }
        if matches!(self.heap.get(obj), Value::Function { .. }) {
            return self.set_function_attr(obj, name, value);
        }
        if let Some(descriptor) = self.slot(obj, name)
            && self.data_descriptor(descriptor)
        {
            if let Value::Property(property) = *self.heap.get(descriptor) {
                return property.write(self, obj, value);
            }
            if let Value::ReadOnlyDescriptor { owner, name, .. } = *self.heap.get(descriptor) {
                return Err(Error::new(
                    "AttributeError",
                    format!("attribute '{name}' of '{owner}' objects is not writable"),
                ));
            }
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
        let object_class = self.builtins.get("object").copied();
        match self.heap.get_mut(obj) {
            Value::Code(_) if matches!(name, "co_name" | "co_filename" | "co_firstlineno") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Partial {
                coroutine_marker, ..
            } if name == "_is_coroutine_marker" => {
                *coroutine_marker = Some(value);
            }
            Value::Property(property) if name == "__name__" => property.name = Some(value),
            Value::Property(property) if name == "__doc__" => property.doc = value,
            Value::Property(_) if matches!(name, "fget" | "fset" | "fdel") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Module {
                name: module,
                scope,
            } => {
                if name == "__dict__" {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                if module == "sys" && name == "path" {
                    self.sys_path = value;
                }
                let scope = *scope;
                self.bind(scope, name, value);
            }
            Value::Instance { class, fields, .. } if Some(*class) != object_class => {
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

    pub(super) fn instance(&mut self, class: Id, dictionary: Option<Id>) -> Id {
        self.alloc(Value::Instance {
            class,
            dictionary,
            text_memory: None,
            fields: HashMap::new(),
            text: None,
        })
    }

    /// Allocate the immutable str payload while retaining the actual Python class.
    pub(super) fn new_str(
        &mut self,
        class: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let base = self.builtin("str");
        if !self.subclass(class, base) {
            let message = match self.heap.get(class) {
                Value::Class { name, .. } => {
                    format!("str.__new__({name}): {name} is not a subtype of str")
                }
                Value::Native(name) if builtins::is_builtin_type(name) => {
                    format!("str.__new__({name}): {name} is not a subtype of str")
                }
                _ => format!(
                    "str.__new__(X): X is not a type object ({})",
                    self.type_name(class)
                ),
            };
            return Err(Error::new("TypeError", message));
        }
        let text = builtins::call(self, "str", args, kwargs)?;
        if class == base {
            return Ok(text);
        }
        let text = self.string_storage(text).expect("str() returns a string");
        Ok(self.alloc(Value::Instance {
            class,
            fields: HashMap::new(),
            dictionary: None,
            text: Some(text),
            text_memory: None,
        }))
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
                                | "int"
                                | "frozenset"
                                | "tuple"
                                | "bytes"
                                | "bytearray"
                                | "range"
                                | "function"
                                | "object"
                                | "property"
                                | "staticmethod"
                        )
                    ) =>
            {
                builtins::call(self, &name, &args, &kwargs)
            }
            Value::Native(name) => stdlib::call(self, &name, &args, &kwargs),
            Value::StaticMethod { callable, .. } => self.call(callable, args, kwargs),
            Value::CodecDecoder(codec) => stdlib::codecs::decode(self, codec, &args, &kwargs),
            Value::IoMethod { owner, name } => stdlib::io::call(self, owner, name, &args, &kwargs),
            Value::Partial {
                function,
                args: bound,
                keywords,
                ..
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
            Value::Class {
                name, ancestors, ..
            } => {
                if ancestors.iter().any(|&base| {
                    matches!(self.heap.get(base), Value::Native(kind) if builtins::is_exception(kind))
                }) {
                    let error = self.exception(&name, &args);
                    return Ok(self.alloc(Value::Raised(error)));
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
                        dictionary: None,
                        text: None,
                        text_memory: None,
                    }));
                }
                let new = self.class_attr(callable, "__new__");
                let str_subclass = self.has_base(callable, "str");
                let instance = if let Some(new) = new {
                    let allocate = self.bind_attribute(new, NONE, callable)?;
                    let positional = std::iter::once(callable)
                        .chain(args.iter().copied())
                        .collect();
                    let instance = self.call(allocate, positional, kwargs.clone())?;
                    if !self.instance_of(instance, callable) {
                        return Ok(instance);
                    }
                    instance
                } else if str_subclass {
                    self.new_str(callable, &args, &kwargs)?
                } else {
                    let dictionary = self
                        .has_base(callable, "dict")
                        .then(|| self.dict(Vec::new()));
                    self.instance(callable, dictionary)
                };
                if let Some(init) = self.bound_slot(instance, "__init__")? {
                    if self.call(init, args, kwargs)? != NONE {
                        return Err(Error::new("TypeError", "__init__ must return None"));
                    }
                } else if new.is_none() && !str_subclass && (!args.is_empty() || !kwargs.is_empty())
                {
                    return Err(Error::new(
                        "TypeError",
                        format!("{name} takes no arguments"),
                    ));
                }
                Ok(instance)
            }
            Value::Function {
                fields,
                scope,
                class_cell,
                code,
                defaults,
                ..
            } => {
                let parameters = self.code_object(code).parameters;
                let name = self.str_value(fields.qualname)?.to_owned();
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
                let Value::Scope(frame) = self.heap.get_mut(local) else {
                    unreachable!()
                };
                frame.first_argument = pos.into_iter().next();
                if sig["class_cell"] == true {
                    frame.class_cell = class_cell;
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
                | ("float", Value::Float(_))
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
        let cell = self.alloc(Value::ClassCell(None));
        let Value::Scope(body_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        body_scope.class_body = Some(Vec::new());
        body_scope.class_cell = Some(cell);
        if let Ok(qualname) = self.lookup_attr(body, "__qualname__") {
            self.bind(scope, "__qualname__", qualname);
        }
        let annotations = self.dict(Vec::new());
        self.bind(scope, "__annotations__", annotations);
        self.execute(code, scope)?;
        let ancestors = self.linearized_bases(&bases)?;
        let class = self.alloc(Value::Class {
            name: name.clone(),
            scope,
            ancestors,
        });
        *self.heap.get_mut(cell) = Value::ClassCell(Some(class));
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
            if matches!(self.heap.get(value), Value::Property(_)) {
                let name = self.text(name);
                let Value::Property(property) = self.heap.get_mut(value) else {
                    unreachable!()
                };
                property.name = Some(name);
            } else if let Some(set_name) = self.slot(value, "__set_name__") {
                let name = self.text(name);
                self.call(set_name, vec![value, class, name], Vec::new())?;
            }
        }
        self.classes.insert(name, class);
        Ok(class)
    }
}
