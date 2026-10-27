use super::*;

pub(super) enum MappingOperation {
    Update,
    Unpack,
}

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
            Value::Native(kind) if builtins::is_exception(kind) => match name {
                "__new__" => self
                    .builtins
                    .get(&format!("{}.__new__", builtins::exception_new_owner(kind)))
                    .copied(),
                "__init__" if kind == "StopIteration" => {
                    self.builtins.get("StopIteration.__init__").copied()
                }
                "__init__" => self.builtins.get("BaseException.__init__").copied(),
                _ => None,
            },
            Value::Native(kind) if matches!(kind.as_str(), "dict" | "int" | "str") => {
                self.builtins.get(&format!("{kind}.{name}")).copied()
            }
            _ => None,
        }
    }
    /// Find the exception storage only when the object inherits StopIteration.
    fn stop_iteration_error(&mut self, object: Id) -> Option<&mut Error> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        let stop = error.class.map_or_else(
            || error.kind == "StopIteration",
            |class| self.has_base(class, "StopIteration"),
        );
        if !stop {
            return None;
        }
        let Value::Raised(error) = self.heap.get_mut(object) else {
            unreachable!()
        };
        Some(error)
    }

    /// Find a Python-declared member on an exception that retains its original class.
    fn exception_member(&self, object: Id, name: &str) -> Option<Id> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        self.class_attr(error.class?, name)
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
        target: Id,
        operation: MappingOperation,
        mut visit: impl FnMut(&mut Self, Id, Id, Option<i64>) -> Result<()>,
    ) -> Result<bool> {
        // dict.update probes for keys before choosing a merge or a sequence of pairs.
        // A later mapping lookup may return a different method or raise.
        if matches!(operation, MappingOperation::Update)
            && !matches!(self.heap.get(input), Value::Dict(_))
            && let Err(error) = self.lookup_attr(input, "keys")
        {
            return if error.kind == "AttributeError" {
                Ok(false)
            } else {
                Err(error)
            };
        }
        if self.slot(input, "__iter__").is_none_or(
            |slot| matches!(self.heap.get(slot), Value::Native(name) if name == "dict.__iter__"),
        ) && let Some(storage) = self.dict_storage(input)
        {
            if storage == target {
                return Ok(true);
            }
            let used = self
                .dict_entries(storage)
                .expect("dictionary storage")
                .len();
            let mut position = 0;
            while let Some((current, key, value, hash)) = self.dictionary_pair(storage, position) {
                self.heap.keep(key);
                self.heap.keep(value);
                visit(self, key, value, hash)?;
                if self
                    .dict_entries(storage)
                    .expect("dictionary storage")
                    .len()
                    != used
                {
                    return Err(Error::new("RuntimeError", "dict mutated during update"));
                }
                position = current + 1;
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
                | Value::DictIterator(_)
        ) {
            return Ok(false);
        }
        let keys = match self.lookup_attr(input, "keys") {
            Ok(keys) => keys,
            Err(error)
                if error.kind == "AttributeError"
                    && matches!(operation, MappingOperation::Unpack) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let result = self.call(keys, Vec::new(), Vec::new())?;
        // PyMapping_Keys keeps an exact list returned by keys() and materializes other results.
        let keys = if matches!(self.heap.get(result), Value::List(_)) {
            result
        } else {
            let items = self.items(result)?;
            self.alloc(Value::List(items))
        };
        let iterator = self.iterator(keys)?;
        while let Some(key) = self.next_item(iterator)? {
            self.heap.keep(key);
            let value = self.get_item(input, key)?;
            visit(self, key, value, None)?;
        }
        Ok(true)
    }

    pub(super) fn bound_slot(&mut self, object: Id, name: &str) -> Result<Option<Id>> {
        let class = match self.heap.get(object) {
            Value::Instance { class, .. } => *class,
            Value::Raised(Error {
                class: Some(class), ..
            }) if name == "__str__" => *class,
            _ => return Ok(None),
        };
        self.class_attr(class, name)
            .map(|slot| self.bind_attribute(slot, object, class))
            .transpose()
    }

    pub(super) fn callable(&self, value: Id) -> bool {
        matches!(
            self.heap.get(value),
            Value::Class { .. }
                | Value::GenericAlias(_)
                | Value::Function { .. }
                | Value::AttrGetter(_)
                | Value::StaticMethod { .. }
                | Value::Cached { .. }
                | Value::Partial { .. }
                | Value::Native(_)
                | Value::CodecDecoder(_)
                | Value::IoMethod { .. }
                | Value::Method { .. }
        ) || matches!(self.heap.get(value),Value::Weak(wr) if wr.kind != weakref::Kind::Proxy)
            || self.slot(value, "__call__").is_some()
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
        if let Value::ClassMethod(callable) = *self.heap.get(value) {
            return Ok(self.alloc(Value::Method {
                callable,
                receiver: owner,
                explicit: true,
            }));
        }
        if let Some(get) = self.slot(value, "__get__") {
            return self.call(get, vec![value, receiver, owner], Vec::new());
        }
        if let Value::StaticMethod { callable, .. } = *self.heap.get(value) {
            return Ok(callable);
        }
        if let Value::Native(symbol) = self.heap.get(value)
            && let Some(slot) = super::thread_local::wrapper_name(symbol)
        {
            return super::thread_local::bind_wrapper(self, value, slot, receiver);
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
                            | "enum.auto.__init__"
                            | "enum.auto.__repr__"
                            | "object.__setattr__"
                            | "object.__delattr__"
                            | "object.__init__"
                            | "BaseException.__init__"
                            | "StopIteration.__init__"
                            | "int.to_bytes"
                            | "list.pop"
                            | "list.clear"
                            | "list.reverse"
                            | "list.sort"
                            | "map.__iter__"
                            | "map.__next__"
                            | weakref::REFERENCE_HASH
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
                    explicit: false,
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
        self.require_checkable_protocol(parent)?;
        Ok(self.subclass(child, parent))
    }

    pub(super) fn subclass(&self, class: Id, target: Id) -> bool {
        if self.equal_class(class, target) || self.structural_context_manager(class, target) {
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
            if let Some(result) = builtins::exception_subclass(child, parent) {
                return result;
            }
            return matches!(
                (child.as_str(), parent.as_str()),
                ("bool", "int")
                    | ("enum.IntEnum" | "enum.IntFlag", "int" | "enum.Enum")
                    | ("enum.StrEnum", "str" | "enum.Enum")
            );
        }
        false
    }
    fn equal_class(&self, a: Id, b: Id) -> bool {
        a == b
            || matches!((self.heap.get(a),self.heap.get(b)),(Value::Native(x),Value::Native(y)) if x==y)
    }
    pub(super) fn has_base(&self, class: Id, target: &str) -> bool {
        match self.heap.get(class) {
            Value::Class { ancestors, .. } => ancestors
                .iter()
                .any(|&b| matches!(self.heap.get(b), Value::Native(name) if name == target)),
            Value::Native(name) => name == target,
            _ => false,
        }
    }
    pub(super) fn namedtuple_fields(&self, class: Id) -> Option<Vec<String>> {
        match self.heap.get(class) {
            Value::Class {
                namedtuple_fields, ..
            } => namedtuple_fields.as_deref().map(<[String]>::to_vec),
            _ => None,
        }
    }
    pub(super) fn builtin_type_name(&self, obj: Id) -> Option<&str> {
        Some(match self.heap.get(obj) {
            Value::None
            | Value::Ellipsis
            | Value::NotImplemented
            | Value::Bool(_)
            | Value::Float(_)
            | Value::Complex(_)
            | Value::Int(_)
            | Value::Str(_)
            | Value::Bytes(_)
            | Value::List(_)
            | Value::Dict(_)
            | Value::Set { .. }
            | Value::Range { .. }
            | Value::Slice(_)
            | Value::Enumerate { .. }
            | Value::Iterator { .. }
            | Value::Tuple(_)
            | Value::DictIterator(_)
            | Value::Class { .. }
            | Value::Function { .. }
            | Value::Traceback { .. }
            | Value::PythonFrame { .. }
            | Value::Code(_)
            | Value::Cell { .. }
            | Value::Generator { .. }
            | Value::Module { .. }
            | Value::ThreadLocal(_)
            | Value::PrimitiveLock(_) => self.type_name(obj),
            Value::TextFile(_) => "_io.TextIOWrapper",
            Value::ByteArray(_) => "bytearray",
            Value::Map { .. } => "map",
            Value::Weak(wr) => wr.kind.class(),
            Value::Zip { .. } => "zip",
            Value::ClassMethod(_) => "classmethod",
            Value::PartialPlaceholder => "functools._PlaceholderType",
            Value::Count { .. } => "itertools.count",
            Value::Repeat(_) => "itertools.repeat",
            Value::StaticMethod { .. } => "staticmethod",
            Value::TypeVar(_) => "typing.TypeVar",
            Value::GenericAlias(alias) => alias.kind.class(),
            Value::Socket(_) => "socket.socket",
            Value::Property(_) => "property",
            Value::Super { .. } => "super",
            Value::ReadOnlyDescriptor { .. } => "getset_descriptor",
            Value::Method { explicit: true, .. } => "method",
            Value::Method { callable, .. } => match self.heap.get(*callable) {
                Value::Function { .. } => "method",
                Value::Native(symbol) if super::thread_local::wrapper_name(symbol).is_some() => {
                    "method-wrapper"
                }
                Value::Native(_) => "builtin_function_or_method",
                _ => return None,
            },
            Value::Native(name) if super::thread_local::wrapper_name(name).is_some() => {
                "wrapper_descriptor"
            }
            Value::Native(name) if self.builtins.get(name) == Some(&obj) => {
                if name == "typing.Any" {
                    return Some("typing._AnyMeta");
                }
                if name == "typing.Callable" {
                    return Some("typing._CallableType");
                }
                if builtins::is_builtin_type(name)
                    || matches!(
                        name.as_str(),
                        "NoneType"
                            | "NotImplementedType"
                            | "ellipsis"
                            | "float"
                            | "code"
                            | "cell"
                            | "function"
                            | "module"
                            | "method"
                            | "builtin_function_or_method"
                            | "method-wrapper"
                            | "wrapper_descriptor"
                            | "generator"
                            | "list_iterator"
                            | "tuple_iterator"
                            | "bytes_iterator"
                            | "bytearray_iterator"
                            | "range_iterator"
                            | "getset_descriptor"
                            | "functools._PlaceholderType"
                            | "operator.attrgetter"
                            | "itertools.count"
                            | "itertools.repeat"
                            | "weakref.ReferenceType"
                            | "weakref.ProxyType"
                            | "weakref.CallableProxyType"
                            | super::thread_local::CLASS
                            | super::thread_lock::CLASS
                            | "traceback"
                            | "frame"
                            | "typing.TypeVar"
                            | "typing._GenericAlias"
                            | "typing._CallableGenericAlias"
                            | "typing._CallableType"
                            | "typing._AnyMeta"
                            | "types.GenericAlias"
                            | "collections.abc._CallableGenericAlias"
                    )
                {
                    "type"
                } else if builtins::is_builtin(name)
                    || builtins::constructor_owner(name).is_some()
                    || matches!(
                        name.as_str(),
                        "binascii.a2b_base64" | "binascii.b2a_base64" | "posix.fspath"
                    )
                    || name.starts_with("_weakref.")
                {
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
                if let Some(class) = error.class {
                    return Ok(class);
                }
                let name = error.kind.clone();
                Ok(self.builtin(&name))
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
        if self.classes.get("collections.abc.Callable") == Some(&class) {
            return self.callable(value);
        }
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
        if let Value::Weak(wr) = self.heap.get(value) {
            if wr.kind.class() == name {
                return true;
            }
            if wr.kind != weakref::Kind::Reference
                && let Some(target) = wr.target
            {
                return self.instance_of(target, class);
            }
        }
        match (name, self.heap.get(value)) {
            (_, Value::Raised(error)) => {
                matches!(self.heap.get(class), Value::Native(name) if name == "object")
                    || self.raised_error_matches(error, class)
            }
            ("bytearray", Value::ByteArray(_))
            | ("object", _)
            | ("bool", Value::Bool(_))
            | ("int", Value::Int(_) | Value::Bool(_))
            | ("float", Value::Float(_))
            | ("complex", Value::Complex(_))
            | ("str", Value::Str(_))
            | ("bytes", Value::Bytes(_))
            | ("struct.Struct", Value::Struct { .. })
            | ("hmac.HMAC", Value::Hmac(_))
            | ("socket.socket", Value::Socket(_))
            | ("_io.TextIOWrapper", Value::TextFile(_))
            | ("list", Value::List(_))
            | ("dict", Value::Dict(_) | Value::DefaultDict { .. })
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | ("slice", Value::Slice(_))
            | ("range", Value::Range { .. })
            | ("map", Value::Map { .. })
            | ("selectors.PollSelector", Value::Selector(_))
            | ("contextvars.ContextVar", Value::ContextVar { .. })
            | ("contextvars.Context", Value::Context(_))
            | ("functools.partial", Value::Partial { .. })
            | ("contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | ("itertools.count", Value::Count { .. })
            | ("itertools.repeat", Value::Repeat(_))
            | (super::thread_local::CLASS, Value::ThreadLocal(_))
            | (super::thread_lock::CLASS, Value::PrimitiveLock(_))
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
            _ => {
                matches!(self.heap.get(class), Value::Native(_))
                    && self.builtin_type_name(value) == Some(name)
            }
        }
    }
    pub(super) fn error_matches(&self, kind: &str, class: Id) -> bool {
        self.error_kind_matches(kind, class, self.classes.get(kind).copied())
    }

    pub(super) fn raised_error_matches(&self, error: &Error, class: Id) -> bool {
        match self.heap.get(class) {
            Value::Tuple(classes) => classes.iter().any(|&c| self.raised_error_matches(error, c)),
            _ if let Some(actual) = error.class => self.subclass(actual, class),
            Value::Native(_) => {
                let kind = error.kind.as_str();
                let builtin = self.builtins.get(kind).copied();
                if self.classes.get(kind).copied() == builtin {
                    self.error_matches(kind, class)
                } else {
                    // A user class with the same name cannot change a native error's ancestry.
                    self.error_kind_matches(kind, class, builtin)
                }
            }
            _ => false,
        }
    }

    fn error_kind_matches(&self, kind: &str, class: Id, registered: Option<Id>) -> bool {
        if let Value::Native(parent) = self.heap.get(class)
            && let Some(result) = builtins::exception_subclass(kind, parent)
        {
            return result;
        }
        match self.heap.get(class) {
            Value::Tuple(classes) => classes
                .iter()
                .any(|&c| self.error_kind_matches(kind, c, registered)),
            Value::Class { name, .. } | Value::Native(name) => {
                kind == name
                    || name == "BaseException"
                    || (name == "Exception" && kind != "SystemExit")
                    || (name == "ValueError"
                        && matches!(kind, "JSONDecodeError" | "binascii.Error"))
                    || (name == "OSError" && kind == "socket.gaierror")
                    || registered.is_some_and(|actual| self.subclass(actual, class))
            }
            _ => false,
        }
    }
    pub(super) fn missing_attribute(&self, obj: Id, name: &str) -> Error {
        Error::new(
            "AttributeError",
            format!("'{}' object has no attribute '{name}'", self.type_name(obj)),
        )
    }

    /// Match an attribute lookup failure using the ordinary exception hierarchy.
    pub(super) fn is_attribute_error(&self, error: &Error) -> bool {
        self.is_builtin_error(error, "AttributeError")
    }

    /// Includes runtime-loaded subclasses of the named builtin exception.
    pub(super) fn is_builtin_error(&self, error: &Error, kind: &str) -> bool {
        (error.class.is_none() && error.kind == kind)
            || self
                .builtins
                .get(kind)
                .is_some_and(|&base| self.raised_error_matches(error, base))
    }

    fn attribute_hook(
        &mut self,
        hook: Id,
        obj: Id,
        class: Id,
        name: &str,
        original: Option<Id>,
    ) -> Result<Id> {
        let callable = self.bind_attribute(hook, obj, class)?;
        let key = original.unwrap_or_else(|| self.text(name));
        self.call(callable, vec![key], Vec::new())
    }

    pub(super) fn lookup_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        self.lookup_named_attr(obj, name, None)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn lookup_named_attr(
        &mut self,
        obj: Id,
        name: &str,
        original: Option<Id>,
    ) -> Result<Id> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self
                .local_attribute(obj, super::thread_local::Name::from_lookup(name, original));
        }
        let Value::Instance { class, .. } = *self.heap.get(obj) else {
            return self.default_get_attr(obj, name);
        };
        let attribute = if let Some(hook) = self.class_attr(class, "__getattribute__") {
            self.attribute_hook(hook, obj, class, name, original)
        } else {
            self.default_get_attr(obj, name)
        };
        match attribute {
            Err(error) if self.is_attribute_error(&error) => {
                let Some(hook) = self.class_attr(class, "__getattr__") else {
                    return Err(error);
                };
                self.attribute_hook(hook, obj, class, name, original)
            }
            result => result,
        }
    }

    /// Base lookup bypasses Python's __getattribute__ and __getattr__ hooks.
    pub(super) fn default_get_attr(&mut self, obj: Id, name: &str) -> Result<Id> {
        if let Value::Instance { class, fields, .. } = self.heap.get(obj) {
            if matches!(name, "name" | "value")
                && let Some((_, member_name, value)) = self.enum_member(obj)
            {
                return Ok(if name == "name" { member_name } else { value });
            }
            return self.instance_attribute(obj, *class, fields.get(name).copied(), name);
        }
        if let Value::Raised(error) = self.heap.get(obj)
            && let Some(class) = error.class
        {
            let field = error
                .trace
                .as_ref()
                .and_then(|trace| trace.fields.get(name).copied());
            if field.is_some() || self.class_attr(class, name).is_some() {
                return self.instance_attribute(obj, class, field, name);
            }
        }
        if name == "value"
            && let Some(error) = self.stop_iteration_error(obj)
        {
            return Ok(error.stop_value());
        }
        if name == "__class__" {
            return self.exact_type(obj);
        }
        if let Some(value) = super::typing::attribute(self, obj, name) {
            return value;
        }
        match self.heap.get(obj).clone() {
            Value::Complex(z) if matches!(name, "real" | "imag") => {
                return Ok(self.alloc(Value::Float(if name == "real" { z.real } else { z.imag })));
            }
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
            Value::Native(class)
                if matches!(
                    class.as_str(),
                    "traceback" | "frame" | "complex" | "float" | "enumerate"
                ) && matches!(name, "__name__" | "__qualname__" | "__module__") =>
            {
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(class)),
                    "__module__" => return Ok(self.text("builtins")),
                    _ => {}
                }
            }
            Value::ClassMethod(callable) if matches!(name, "__func__" | "__wrapped__") => {
                return Ok(callable);
            }
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
            Value::Slice(parts) if matches!(name, "start" | "stop" | "step") => {
                return Ok(parts[match name {
                    "start" => 0,
                    "stop" => 1,
                    _ => 2,
                }]);
            }
            Value::Property(property) => {
                if let Some(attribute) = property.attribute(self, name)? {
                    return Ok(attribute);
                }
            }
            Value::Native(symbol) if symbol == "posix.fspath" => match name {
                "__name__" | "__qualname__" => return Ok(self.text("fspath")),
                "__module__" => return Ok(self.text("posix")),
                "__self__" => return self.module("posix"),
                _ => {}
            },
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
                "co_freevars" => {
                    if !code.closure_supported && !code.module_definition {
                        return Err(Error::new(
                            "NotImplementedError",
                            "code lacks closure metadata",
                        ));
                    }
                    let names = code.freevars.iter().map(|name| self.text(name)).collect();
                    return Ok(self.alloc(Value::Tuple(names)));
                }
                _ => {}
            },
            Value::Cell {
                scope,
                name: cell_name,
            } if name == "cell_contents" => {
                return self
                    .scope_name(scope, &cell_name)
                    .ok_or_else(|| Error::new("ValueError", "Cell is empty"));
            }
            Value::Generator { code, .. } if name == "gi_code" => return Ok(code),
            Value::Function { code, .. } if name == "__code__" => return Ok(code),
            Value::Function {
                scope,
                fields,
                closure,
                code,
                ..
            } => {
                if name == "__closure__" {
                    let code = self.code_object(code);
                    if !code.closure_supported && !code.module_definition {
                        return Err(Error::new(
                            "NotImplementedError",
                            "code lacks closure metadata",
                        ));
                    }
                    return Ok(closure);
                }
                if name == "__globals__" {
                    return Ok(self.module_globals(scope));
                }
                if let Some(value) = self.function_attr(obj, fields, name)? {
                    return Ok(value);
                }
            }
            Value::Method {
                callable, receiver, ..
            } if let Value::Native(ref symbol) = *self.heap.get(callable)
                && let Some(slot) = super::thread_local::wrapper_name(symbol) =>
            {
                if name == "__self__" {
                    return Ok(receiver);
                }
                if let Some(value) = super::thread_local::wrapper_attribute(self, slot, name) {
                    return Ok(value);
                }
            }
            Value::Native(ref symbol)
                if let Some(slot) = super::thread_local::wrapper_name(symbol) =>
            {
                if let Some(value) = super::thread_local::wrapper_attribute(self, slot, name) {
                    return Ok(value);
                }
                if name == "__get__" {
                    let callable = self.builtin("_thread._local.wrapper_get");
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: false,
                    }));
                }
            }
            Value::Method {
                callable,
                receiver,
                explicit,
            } => match name {
                "__self__" => return Ok(receiver),
                "__func__"
                    if explicit || matches!(self.heap.get(callable), Value::Function { .. }) =>
                {
                    return Ok(callable);
                }
                _ if explicit || matches!(self.heap.get(callable), Value::Function { .. }) => {
                    return self.lookup_attr(callable, name);
                }
                _ => {
                    if let Value::Native(method) = self.heap.get(callable) {
                        let value = match name {
                            "__name__" => method.rsplit('.').next(),
                            "__qualname__" => Some(method.as_str()),
                            "__module__" => return Ok(NONE),
                            _ => None,
                        };
                        if let Some(value) = value {
                            return Ok(self.text(value.to_owned()));
                        }
                    }
                }
            },
            Value::RegexPattern(pattern) if name == "flags" => return Ok(self.int(pattern.flags)),
            Value::RegexPattern(pattern) if name == "groups" => {
                return Ok(self.int(pattern.group_count() as i64));
            }
            Value::RegexPattern(pattern) if name == "pattern" => {
                return Ok(pattern.source);
            }
            Value::Calendar { value, tzinfo } => {
                return stdlib::datetime::attribute(self, obj, &value, tzinfo, name);
            }
            Value::Native(class)
                if class == "datetime.datetime" && matches!(name, "fromtimestamp" | "combine") =>
            {
                return Ok(self.builtin(&format!("datetime.datetime.{name}")));
            }
            Value::Native(class) if class == "map" => match name {
                "__iter__" | "__next__" => return Ok(self.builtin(&format!("map.{name}"))),
                "__name__" | "__qualname__" => return Ok(self.text("map")),
                "__module__" => return Ok(self.text("builtins")),
                _ => {}
            },
            Value::Native(class) if class == "datetime.time" => match name {
                "__name__" | "__qualname__" => return Ok(self.text("time")),
                "__module__" => return Ok(self.text("datetime")),
                _ => {}
            },
            Value::Native(class) if class == super::thread_lock::CLASS => match name {
                "__module__" => return Ok(self.text("_thread")),
                "__name__" | "__qualname__" => return Ok(self.text("lock")),
                attr if super::thread_lock::is_method(attr) => {
                    return Ok(self.builtin(&format!("{class}.{attr}")));
                }
                _ => {}
            },
            Value::Native(class) if class == super::thread_local::CLASS => match name {
                "__module__" => return Ok(self.text("_thread")),
                "__name__" | "__qualname__" => return Ok(self.text("_local")),
                attr if super::thread_local::is_method(attr) => {
                    return Ok(self.builtin(&format!("{class}.{attr}")));
                }
                _ => {}
            },
            Value::Native(class)
                if matches!(class.as_str(), "itertools.count" | "itertools.repeat") =>
            {
                match name {
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(class.trim_start_matches("itertools.").to_owned()));
                    }
                    "__module__" => return Ok(self.text("itertools")),
                    _ => {}
                }
            }
            Value::Native(class) if class == "datetime.timezone" && name == "utc" => {
                return Ok(stdlib::datetime::utc(self));
            }
            Value::MappingProxy(dict)
                if matches!(name, "keys" | "items" | "values" | "get" | "copy") =>
            {
                return self.lookup_attr(dict, name);
            }
            Value::Native(ref symbol)
                if name == "__self__"
                    && let Some(owner) = builtins::constructor_owner(symbol) =>
            {
                return Ok(self.builtin(owner));
            }
            Value::Native(class)
                if matches!(class.as_str(), "int" | "float") && name == "__new__" =>
            {
                return Ok(self.builtin(&format!("{class}.__new__")));
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
                return Ok(if handle.closed() { TRUE } else { FALSE });
            }
            Value::Native(class) if class == "socket.socket" && name == "__name__" => {
                return Ok(self.text("socket"));
            }
            Value::Socket(_) if name == "family" => return Ok(self.int(i64::from(libc::AF_INET))),
            Value::Socket(_) if name == "type" => return Ok(self.int(i64::from(libc::SOCK_STREAM))),
            Value::BufferedRandom(file) if name == "closed" => {
                return Ok(if file.borrow().closed() { TRUE } else { FALSE });
            }
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
            Value::Raised(error)
                if matches!(error.kind.as_str(), "ImportError" | "ModuleNotFoundError")
                    && matches!(name, "msg" | "name" | "path" | "name_from") =>
            {
                if name == "msg" {
                    return Ok(match error.args.as_deref() {
                        Some([one]) => *one,
                        Some(_) => NONE,
                        None => self.text(error.message),
                    });
                }
                let Some(fields) = error.import else {
                    return Ok(NONE);
                };
                let value = match name {
                    "name" => fields.name.as_deref(),
                    "path" => fields.path.as_deref(),
                    _ => fields.name_from.as_deref(),
                };
                return Ok(value.map_or(NONE, |value| self.text(value)));
            }
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
            Value::Struct { format, layout } => match name {
                "format" => return Ok(self.text(format)),
                "size" => return Ok(self.int(layout.size as i64)),
                _ => {}
            },
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
            Value::Weak(wr) if name == "__callback__" => return Ok(wr.callback),
            Value::Weak(_) if name == "__hash__" => {
                let callable = self.builtin(weakref::REFERENCE_HASH);
                let class = self.builtin(weakref::Kind::Reference.class());
                return self.bind_attribute(callable, obj, class);
            }
            Value::Native(ref class)
                if class == weakref::Kind::Reference.class() && name == "__hash__" =>
            {
                return Ok(self.builtin(weakref::REFERENCE_HASH));
            }
            Value::Native(ref method)
                if method == weakref::REFERENCE_HASH && name == "__qualname__" =>
            {
                return Ok(self.text("ReferenceType.__hash__"));
            }
            Value::Native(ref builtin)
                if name == "__module__"
                    && (builtin.starts_with("weakref.") || builtin.starts_with("_weakref.")) =>
            {
                return Ok(self.text(builtin.split('.').next().unwrap()));
            }
            Value::ContextVar { name: var_name, .. } if name == "name" => return Ok(var_name),
            Value::ContextToken { variable, .. } if name == "var" => return Ok(variable),
            Value::ContextToken { old, .. } if name == "old_value" => {
                return Ok(old.unwrap_or(self.token_missing));
            }
            Value::Native(builtin)
                if matches!(builtin.as_str(), "binascii.Error" | "binascii.Incomplete") =>
            {
                let short = builtin.rsplit('.').next().unwrap();
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(short)),
                    "__module__" => return Ok(self.text("binascii")),
                    "__base__" | "__bases__" => {
                        let base = self.builtin(if short == "Error" {
                            "ValueError"
                        } else {
                            "Exception"
                        });
                        return Ok(if name == "__base__" {
                            base
                        } else {
                            self.alloc(Value::Tuple(vec![base]))
                        });
                    }
                    _ => {}
                }
            }
            Value::Native(builtin)
                if name == "__name__" && self.builtins.get(&builtin) == Some(&obj) =>
            {
                return Ok(self.text(builtin.rsplit('.').next().unwrap_or(&builtin)));
            }
            Value::Native(class) if class == "contextvars.Token" && name == "MISSING" => {
                return Ok(self.token_missing);
            }
            Value::Native(class)
                if matches!(class.as_str(), "bytes" | "bytearray")
                    && matches!(name, "maketrans" | "translate") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(class) if class == "bytes" && name == "__mod__" => {
                return Ok(self.builtin("bytes.__mod__"));
            }
            Value::Bytes(_) if name == "maketrans" => return Ok(self.builtin("bytes.maketrans")),
            Value::ByteArray(_) if name == "maketrans" => {
                return Ok(self.builtin("bytearray.maketrans"));
            }
            Value::Native(class) if class == "bool" && name == "to_bytes" => {
                return Ok(self.builtin("int.to_bytes"));
            }
            Value::Native(class)
                if class == "list" && matches!(name, "clear" | "pop" | "reverse" | "sort") =>
            {
                return Ok(self.builtin(&format!("list.{name}")));
            }
            Value::Native(class)
                if class == "dict"
                    || (builtins::is_exception(&class)
                        && matches!(name, "__new__" | "__init__"))
                    || (class == "int" && name == "to_bytes")
                    || (class == "str"
                        && (name == "maketrans" || builtins::STR_METHODS.contains(&name))) =>
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
            Value::Native(class)
                if class == "slice" && matches!(name, "indices" | "__reduce__") =>
            {
                return Ok(self.builtin(&format!("slice.{name}")));
            }
            Value::Native(class)
                if class == "enumerate" && matches!(name, "__iter__" | "__next__") =>
            {
                return Ok(self.builtin(&format!("enumerate.{name}")));
            }
            Value::Native(class) if class == "str" && name == "__new__" => {
                return Ok(self.builtin("str.__new__"));
            }
            Value::Module {
                name: module,
                scope,
                ..
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
                name: class_name,
                scope,
                ..
            } => {
                if name == "__annotations__"
                    && let Some(thunks) = self
                        .scope_data(scope)
                        .class_namespace
                        .as_ref()
                        .and_then(|c| c.annotation_thunks)
                {
                    let entries = self.dict_entries(thunks).unwrap().to_vec();
                    let mapping = self
                        .own_class_attr(obj, name)
                        .ok_or_else(|| self.missing_attribute(obj, name))?;
                    let mut evaluated = Vec::with_capacity(entries.len());
                    for (key, thunk) in entries {
                        let value = self.call(thunk, Vec::new(), Vec::new())?;
                        evaluated.push((key, value));
                    }
                    for (key, value) in evaluated {
                        self.put_item(mapping, key, value)?;
                    }
                    let Value::Scope(frame) = self.heap.get_mut(scope) else {
                        unreachable!()
                    };
                    frame.class_namespace.as_mut().unwrap().annotation_thunks = None;
                    return Ok(mapping);
                }
                if name == "__name__" {
                    return Ok(self.text(class_name));
                }
                if let Some(value) = self.class_attr(obj, name) {
                    return self.bind_attribute(value, NONE, obj);
                }
                if name == "__new__" && self.has_base(obj, "str") {
                    return Ok(self.builtin("str.__new__"));
                }
                if name == "__new__" && self.has_base(obj, "float") {
                    return Ok(self.builtin("float.__new__"));
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
        let base = self.scalar_value(obj);
        if name == "maketrans" && matches!(self.heap.get(base), Value::Str(_)) {
            return Ok(self.builtin("str.maketrans"));
        }
        if name == "__new__" {
            match self.heap.get(base) {
                Value::Int(_) => return Ok(self.builtin("int.__new__")),
                Value::Str(_) => return Ok(self.builtin("str.__new__")),
                _ => {}
            }
        }
        if let Some(method) = builtins::method_name(self.heap.get(base), name) {
            let fun = self.alloc(Value::Native(method));
            return Ok(self.alloc(Value::Method {
                callable: fun,
                receiver: obj,
                explicit: false,
            }));
        }
        Err(self.missing_attribute(obj, name))
    }
    pub(super) fn delete_attr(&mut self, obj: Id, name: &str) -> Result<()> {
        self.delete_named_attr(obj, name, None)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn delete_named_attr(
        &mut self,
        obj: Id,
        name: &str,
        original: Option<Id>,
    ) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_local_attribute(
                obj,
                super::thread_local::Name::from_lookup(name, original),
                None,
            );
        }
        if matches!(self.heap.get(obj), Value::Slice(_))
            && matches!(name, "start" | "stop" | "step")
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(hook) = self.bound_slot(obj, "__delattr__")? {
            let name = original.unwrap_or_else(|| self.text(name));
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
        if let Value::Cell {
            scope,
            name: cell_name,
        } = self.heap.get(obj).clone()
            && name == "cell_contents"
        {
            let key = self.text(&cell_name);
            self.remove_binding(scope, &cell_name, key)?;
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_base_local_attribute(obj, name, None);
        }
        let exception_member = self.exception_member(obj, name);
        let member = self.slot(obj, name);
        if let Some(descriptor) = member.or(exception_member)
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
        if let Value::Raised(error) = self.heap.get_mut(obj) {
            if error
                .trace
                .as_mut()
                .and_then(|trace| trace.fields.remove(name))
                .is_some()
            {
                return Ok(());
            }
            if exception_member.is_some() {
                return Err(self.missing_attribute(obj, name));
            }
        }
        if name == "value"
            && let Some(error) = self.stop_iteration_error(obj)
        {
            error.set_stop_value(NONE);
            return Ok(());
        }
        if name == "__class__" && member.is_none() {
            return Err(Error::new("TypeError", "can't delete __class__ attribute"));
        }
        match *self.heap.get(obj) {
            Value::Function { fields, .. } => self.delete_function_attr(obj, fields, name),
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
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
                    "__annotations__"
                        if self
                            .scope_data(scope)
                            .class_namespace
                            .as_ref()
                            .and_then(|c| c.annotation_thunks)
                            .is_some() =>
                    {
                        Error::new("AttributeError", "__annotations__")
                    }
                    _ => {
                        let key = self.text(name);
                        if self.remove_binding(scope, name, key)? {
                            if name == "__abstractmethods__"
                                && let Value::Scope(data) = self.heap.get_mut(scope)
                            {
                                data.is_abstract = false;
                            }
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
                if name == "__dict__" && member.is_none() {
                    let previous = std::mem::take(fields);
                    for value in previous.into_values() {
                        self.heap.weak_edge_removed(value);
                    }
                    return Ok(());
                }
                let value = fields
                    .remove(name)
                    .ok_or_else(|| self.missing_attribute(obj, name))?;
                self.heap.weak_edge_removed(value);
                Ok(())
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
        self.set_named_attr(obj, name, None, value)
    }

    /// Preserve a Python name object when an attribute builtin invokes a hook.
    pub(super) fn set_named_attr(
        &mut self,
        obj: Id,
        name: &str,
        original: Option<Id>,
        value: Id,
    ) -> Result<()> {
        let obj = self.weak_proxy_target(obj)?;
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_local_attribute(
                obj,
                super::thread_local::Name::from_lookup(name, original),
                Some(value),
            );
        }
        if matches!(self.heap.get(obj), Value::Slice(_))
            && matches!(name, "start" | "stop" | "step")
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(hook) = self.bound_slot(obj, "__setattr__")? {
            let name = original.unwrap_or_else(|| self.text(name));
            self.call(hook, vec![name, value], Vec::new())?;
            return Ok(());
        }
        self.default_set_attr(obj, name, value)
    }

    pub(super) fn default_set_attr(&mut self, obj: Id, name: &str, value: Id) -> Result<()> {
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_base_local_attribute(obj, name, Some(value));
        }
        if name == "__abstractmethods__"
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            return self.set_abstract_methods(scope, value);
        }
        if name == "__context__" && matches!(self.heap.get(obj), Value::Raised(_)) {
            if value != NONE && !matches!(self.heap.get(value), Value::Raised(_)) {
                return Err(Error::new(
                    "TypeError",
                    "exception context must be None or derive from BaseException",
                ));
            }
            let Value::Raised(error) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            error.tracing().context = (value != NONE).then_some(value);
            return Ok(());
        }
        if let Value::StaticMethod { dictionary, .. } = *self.heap.get(obj) {
            return super::static_method::update_attribute(
                self,
                obj,
                dictionary,
                name,
                Some(value),
            );
        }
        if let Value::Cell {
            scope,
            name: cell_name,
        } = self.heap.get(obj).clone()
            && name == "cell_contents"
        {
            self.bind(scope, &cell_name, value);
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::Function { .. }) {
            return self.set_function_attr(obj, name, value);
        }
        let exception_member = self.exception_member(obj, name);
        if let Some(descriptor) = self.slot(obj, name).or(exception_member)
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
        if exception_member.is_some()
            && let Value::Raised(error) = self.heap.get_mut(obj)
        {
            error.tracing().fields.insert(name.to_owned(), value);
            return Ok(());
        }
        if name == "value"
            && let Some(error) = self.stop_iteration_error(obj)
        {
            error.set_stop_value(value);
            return Ok(());
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
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Module {
                name: module,
                scope,
                ..
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
                if let Some(previous) = fields.insert(name.to_owned(), value)
                    && previous != value
                {
                    self.heap.weak_edge_removed(previous);
                }
            }
            Value::Class { scope, .. } => {
                let scope = *scope;
                if name == "__annotations__"
                    && let Value::Scope(data) = self.heap.get_mut(scope)
                    && let Some(namespace) = &mut data.class_namespace
                {
                    namespace.annotation_thunks = None;
                }
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
            memory_stream: None,
            fields: HashMap::new(),
            scalar: None,
        })
    }

    /// Allocate a binary64 payload while retaining the requested float subclass.
    ///
    /// ```python
    /// class Weight(float):
    ///     pass
    /// assert type(Weight("1.5")) is Weight
    /// ```
    pub(super) fn new_float(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&class, values)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "float.__new__(): not enough arguments",
            ));
        };
        let base = self.builtin("float");
        if !self.subclass(class, base) {
            return Err(Error::new(
                "TypeError",
                "float.__new__ requires a float subtype",
            ));
        }
        let value = builtins::call(self, "float", values, kwargs)?;
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

    /// Allocate the immutable string or bytes payload while retaining the Python class.
    pub(super) fn new_string(
        &mut self,
        kind: &str,
        class: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let base = self.builtin(kind);
        if !self.subclass(class, base) {
            let message = match self.heap.get(class) {
                Value::Class { name, .. } => {
                    format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
                }
                Value::Native(name) if builtins::is_builtin_type(name) => {
                    format!("{kind}.__new__({name}): {name} is not a subtype of {kind}")
                }
                _ => format!(
                    "{kind}.__new__(X): X is not a type object ({})",
                    self.type_name(class)
                ),
            };
            return Err(Error::new("TypeError", message));
        }
        let text = builtins::call(self, kind, args, kwargs)?;
        if class == base {
            return Ok(text);
        }
        let text = self.scalar_value(text);
        Ok(self.alloc(Value::Instance {
            class,
            fields: HashMap::new(),
            dictionary: None,
            scalar: Some(text),
            memory_stream: None,
        }))
    }

    /// Count a generated NamedTuple repr frame like a runtime-loaded Python call.
    pub(super) fn check_call_depth(&self, additional: usize) -> Result<()> {
        if self.call_depth + additional >= 1000 {
            return Err(Error::new(
                "RecursionError",
                "maximum recursion depth exceeded",
            ));
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
            Value::Weak(wr) => match wr.kind {
                weakref::Kind::Reference if args.is_empty() && kwargs.is_empty() => {
                    Ok(wr.target.unwrap_or(NONE))
                }
                weakref::Kind::Reference if !kwargs.is_empty() => Err(Error::new(
                    "TypeError",
                    "weakref() takes no keyword arguments",
                )),
                weakref::Kind::Reference => Err(Error::new(
                    "TypeError",
                    format!("weakref expected 0 arguments, got {}", args.len()),
                )),
                weakref::Kind::CallableProxy => self.call(wr.upgrade()?, args, kwargs),
                weakref::Kind::Proxy => Err(Error::new(
                    "TypeError",
                    "'weakref.ProxyType' object is not callable",
                )),
            },
            Value::GenericAlias(alias) => {
                let result = self.call(alias.origin, args, kwargs)?;
                if matches!(self.heap.get(result), Value::Instance { .. })
                    && let Err(error) = self.set_attr(result, "__orig_class__", callable)
                    && !alias.kind.is_typing_alias()
                    && !matches!(error.kind.as_str(), "AttributeError" | "TypeError")
                {
                    return Err(error);
                }
                Ok(result)
            }
            Value::Native(name) if name == "int.__new__" => self.new_int(&args, &kwargs),
            Value::Native(name) if name == "float.__new__" => self.new_float(&args, &kwargs),
            Value::Native(name)
                if builtins::is_builtin(&name)
                    || builtins::constructor_owner(&name).is_some()
                    || matches!(
                        name.as_str(),
                        "BaseException.__init__" | "StopIteration.__init__"
                    )
                    || !name.contains('.')
                    || matches!(
                        name.split('.').next(),
                        Some(
                            "str"
                                | "enumerate"
                                | "dict"
                                | "list"
                                | "map"
                                | "set"
                                | "int"
                                | "frozenset"
                                | "tuple"
                                | "bytes"
                                | "bytearray"
                                | "range"
                                | "slice"
                                | "function"
                                | "method"
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
                let bound = self.items(bound)?;
                let required = bound
                    .iter()
                    .filter(|&&id| matches!(self.heap.get(id), Value::PartialPlaceholder))
                    .count();
                if args.len() < required {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "missing positional arguments in 'partial' call; expected at least {required}, got {}",
                            args.len()
                        ),
                    ));
                }
                let mut supplied = args.into_iter();
                let mut positional = Vec::with_capacity(bound.len() + supplied.len());
                for argument in bound {
                    if matches!(self.heap.get(argument), Value::PartialPlaceholder) {
                        positional.push(supplied.next().unwrap());
                    } else {
                        positional.push(argument);
                    }
                }
                positional.extend(supplied);
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
            Value::Method {
                callable, receiver, ..
            } => {
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
            Value::Class { name, scope, .. } => {
                if let Some(values) = self.enum_values(callable) {
                    return self.enum_lookup(callable, values, &args, &kwargs);
                }
                if self.scope_data(scope).is_abstract {
                    return Err(self.abstract_class_error(scope, &name)?);
                }
                if self.is_protocol_declaration(callable)
                    && self.class_attr(callable, "__init__").is_none()
                {
                    return Err(Error::new("TypeError", "Protocols cannot be instantiated"));
                }
                if self.classes.get("collections.abc.Callable") == Some(&callable) {
                    return Err(Error::new(
                        "TypeError",
                        "Can't instantiate abstract class Callable without an implementation for abstract method '__call__'",
                    ));
                }
                if self.classes.get("typing.Generic") == Some(&callable) {
                    return Err(Error::new(
                        "TypeError",
                        "Can't instantiate typing.Generic directly",
                    ));
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
                        scalar: None,
                        memory_stream: None,
                    }));
                }
                let new = self.class_attr(callable, "__new__");
                let string_base = ["str", "bytes"]
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
                let float_subclass = self.has_base(callable, "float");
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
                } else if let Some(kind) = string_base {
                    self.new_string(kind, callable, &args, &kwargs)?
                } else if float_subclass {
                    let positional = std::iter::once(callable)
                        .chain(args.iter().copied())
                        .collect::<Vec<_>>();
                    self.new_float(&positional, &kwargs)?
                } else {
                    let dictionary = self
                        .has_base(callable, "dict")
                        .then(|| self.dict(Vec::new()));
                    self.instance(callable, dictionary)
                };
                let actual = self.exact_type(instance)?;
                if let Some(init) = self.class_attr(actual, "__init__") {
                    let init = self.bind_attribute(init, instance, actual)?;
                    let result = self.call(init, args, kwargs)?;
                    if result != NONE {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "__init__() should return None, not '{}'",
                                self.type_name(result)
                            ),
                        ));
                    }
                } else if new.is_none()
                    && string_base.is_none()
                    && !float_subclass
                    && (!args.is_empty() || !kwargs.is_empty())
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
                kwdefaults,
                ..
            } => {
                let parameters = self.code_object(code).parameters;
                let name = self.str_value(fields.qualname)?.to_owned();
                if parameters == NONE {
                    if let Some((key, _)) = kwargs.first() {
                        return Err(Error::new(
                            "TypeError",
                            format!("{name}() got an unexpected keyword argument '{key}'"),
                        ));
                    }
                    if !args.is_empty() {
                        let verb = if args.len() == 1 { "was" } else { "were" };
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "{name}() takes 0 positional arguments but {} {verb} given",
                                args.len()
                            ),
                        ));
                    }
                    let globals = self.module_globals(scope);
                    return self.exec_source(code, globals, None);
                }
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
                let locals = names("scope_locals");
                let nonlocals = names("scope_nonlocals");
                let Value::Scope(frame) = self.heap.get_mut(local) else {
                    unreachable!()
                };
                frame.closure.declared.extend(locals);
                frame.closure.declared_complete =
                    sig["scope_locals_complete"].as_bool().unwrap_or(false);
                frame.closure.nonlocals.extend(nonlocals);
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
                let default_values = if defaults == NONE {
                    Vec::new()
                } else {
                    self.items(defaults)?
                };
                for (index, n) in pos.iter().enumerate() {
                    if !self.scope_data(local).names.contains_key(n) {
                        let offset = index + default_values.len();
                        let val = offset
                            .checked_sub(pos.len())
                            .and_then(|i| default_values.get(i))
                            .copied()
                            .ok_or_else(|| {
                                Error::new("TypeError", format!("{name} requires argument {n:?}"))
                            })?;
                        self.bind(local, n, val);
                    }
                }
                for n in &kwonly {
                    if !self.scope_data(local).names.contains_key(n) {
                        let val = self
                            .dict_entries(kwdefaults)
                            .and_then(|entries| {
                                entries.iter().find_map(|&(key, value)| {
                                    matches!(self.heap.get(key), Value::Str(text) if text == n)
                                        .then_some(value)
                                })
                            })
                            .ok_or_else(|| {
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
                self.check_call_depth(0)?;
                self.call_depth += 1;
                let result =
                    stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || self.execute(code, local));
                self.call_depth -= 1;
                let value = result?;
                if let Some(expected) = sig["return_type"].as_str() {
                    self.require_scalar(value, expected, &format!("{name} return value"))?;
                }
                Ok(value)
            }
            Value::Instance { .. } => {
                // Binding a descriptor can call Python before it returns its target.
                let exhausted = stacker::remaining_stack()
                    .map_or(self.slot_depth >= 128, |remaining| remaining < 64 * 1024);
                if exhausted {
                    return Err(Error::new(
                        "RecursionError",
                        "maximum recursion depth exceeded while calling a Python object",
                    ));
                }
                self.slot_depth += 1;
                let result = self.call_instance(callable, args, kwargs);
                self.slot_depth -= 1;
                result
            }
            _ => Err(Error::new(
                "TypeError",
                format!("'{}' object is not callable", self.type_name(callable)),
            )),
        }
    }

    fn call_instance(
        &mut self,
        mut callable: Id,
        args: Vec<Id>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<Id> {
        // A finite delegation chain only needs one Rust frame per batch. The
        // next call checks the available stack again, including on cyclic chains.
        for _ in 0..32 {
            let Some(next) = self.bound_slot(callable, "__call__")? else {
                return Err(Error::new(
                    "TypeError",
                    format!("'{}' object is not callable", self.type_name(callable)),
                ));
            };
            callable = next;
            if !matches!(self.heap.get(callable), Value::Instance { .. }) {
                break;
            }
        }
        self.call(callable, args, kwargs)
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
        let (original, bases) = super::typing::resolve_bases(self, bases)?;
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
        let enum_namespace = self.enum_namespace(&bases);
        let enum_kind = enum_namespace.as_ref().map(|state| state.kind);
        let cell = self.alloc(Value::ClassCell(None));
        let Value::Scope(body_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        body_scope.class_body = Some(Vec::new());
        body_scope.enum_namespace = enum_namespace;
        body_scope.class_cell = Some(cell);
        let mut globals = parent;
        while let Some(outer) = self.scope_data(globals).parent {
            globals = outer;
        }
        let module_name = if let Some(exec_globals) = self.scope_data(globals).exec_globals {
            self.source_global(exec_globals, "__name__")?
        } else {
            self.scope_name(globals, "__name__")
                .unwrap_or_else(|| self.text("builtins"))
        };
        self.bind(scope, "__module__", module_name);
        if let Ok(qualname) = self.lookup_attr(body, "__qualname__") {
            self.bind(scope, "__qualname__", qualname);
        }
        let annotations = self.dict(Vec::new());
        self.bind(scope, "__annotations__", annotations);
        self.execute(code, scope)?;
        if self.scope_name(scope, "__module__").is_none() {
            let namespace = self.module_globals(parent);
            if let Some(name) = self.dict_named_item(namespace, "__name__") {
                self.bind(scope, "__module__", name);
            }
        }
        // Inheriting a protocol creates a concrete class unless Protocol is a direct base again.
        if let Some(&protocol) = self.classes.get("typing.Protocol")
            && bases.iter().any(|&base| self.subclass(base, protocol))
        {
            self.bind(
                scope,
                "_is_protocol",
                if bases.contains(&protocol) {
                    TRUE
                } else {
                    FALSE
                },
            );
        }
        super::typing::class_parameters(self, scope, original, &bases)?;
        let class = self.allocate_class(name.clone(), scope, bases)?;
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
        self.init_abstract_class(class, &name, scope)?;
        self.classes.insert(name, class);
        if let Some(kind) = enum_kind {
            let scalar = kind.scalar_base().map(|base| self.builtin(base));
            self.populate_enum(class, scalar)?;
        }
        Ok(class)
    }
}
