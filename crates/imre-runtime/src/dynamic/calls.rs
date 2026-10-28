use super::wrapper_descriptor::Wrapper;
use super::*;

pub(super) enum MappingOperation {
    Copy,
    Update,
    Unpack,
}

#[derive(Default)]
struct InstanceLayout {
    dictionary: bool,
    declared_slot: bool,
    active_slot: bool,
}

impl Context {
    pub(super) fn class_attr(&self, class: Id, name: &str) -> Option<Id> {
        if name == "__hash__" {
            if matches!(self.heap.get(class), Value::Native(_)) {
                return self.own_class_attr(class, name);
            }
            return self
                .find_in_mro(class, |base| {
                    let slot = self.own_class_attr(base, name);
                    (slot.is_some() || matches!(self.heap.get(base), Value::Native(_)))
                        .then_some(slot)
                })
                .flatten();
        }
        self.find_in_mro(class, |base| self.own_class_attr(base, name))
    }

    /// Visit the class and its bases in Python's method resolution order.
    pub(super) fn find_in_mro<T>(
        &self,
        class: Id,
        mut lookup: impl FnMut(Id) -> Option<T>,
    ) -> Option<T> {
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return lookup(class);
        };
        std::iter::once(class)
            .chain(ancestors.iter().copied())
            .find_map(lookup)
    }

    pub(super) fn own_class_attr(&self, class: Id, name: &str) -> Option<Id> {
        match self.heap.get(class) {
            Value::Class { scope, .. } => self.scope_data(*scope).names.get(name).copied(),
            Value::Native(kind) if name == "__hash__" && kind == "object" => {
                self.builtins.get("object.__hash__").copied()
            }
            Value::Native(kind) if builtins::is_enum_base(kind) => {
                self.builtins.get(&format!("enum.Enum.{name}")).copied()
            }
            Value::Native(kind)
                if name == "__hash__"
                    && matches!(kind.as_str(), "list" | "dict" | "set" | "bytearray") =>
            {
                Some(NONE)
            }
            Value::Native(kind) if builtins::is_exception(kind) => match name {
                "__new__" => self
                    .builtins
                    .get(&format!("{}.__new__", builtins::exception_new_owner(kind)))
                    .copied(),
                "__init__" => self
                    .builtins
                    .get(builtins::exception_initializer(kind))
                    .copied(),
                _ => None,
            },
            Value::Native(kind)
                if matches!(
                    kind.as_str(),
                    "dict"
                        | "list"
                        | "tuple"
                        | "int"
                        | "bytes"
                        | "str"
                        | "property"
                        | "module"
                        | weakref::REFERENCE_TYPE
                ) || (name == "__repr__"
                    && matches!(kind.as_str(), "set" | "frozenset" | "bytes" | "bytearray")) =>
            {
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

    /// Lazily allocate the exact dictionary exposed through BaseException.__dict__.
    fn exception_dictionary(&mut self, obj: Id) -> Id {
        let Value::Raised(error) = self.heap.get(obj) else {
            unreachable!()
        };
        if let Some(dictionary) = error.trace.as_ref().and_then(|trace| trace.dictionary) {
            return dictionary;
        }
        let dictionary = self.dict(Vec::new());
        let Value::Raised(error) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        error.tracing().dictionary = Some(dictionary);
        dictionary
    }

    /// Instance attributes use raw dict storage even when __dict__ was set to a subclass.
    fn dictionary_field(&mut self, dictionary: Option<Id>, name: &str) -> Result<Option<Id>> {
        let Some(dictionary) = dictionary else {
            return Ok(None);
        };
        let storage = self.dict_storage(dictionary).expect("instance dictionary");
        let key = self.text(name);
        let (_, index) = self.find_dictionary_key(storage, key, None)?;
        Ok(index.map(|i| self.dict_entries(storage).expect("instance dictionary")[i].1))
    }

    fn update_exception_field(&mut self, obj: Id, name: &str, value: Option<Id>) -> Result<()> {
        let dictionary = if value.is_some() {
            self.exception_dictionary(obj)
        } else {
            let Value::Raised(error) = self.heap.get(obj) else {
                unreachable!()
            };
            let Some(dictionary) = error.trace.as_ref().and_then(|trace| trace.dictionary) else {
                return Err(self.missing_attribute(obj, name));
            };
            dictionary
        };
        self.update_dictionary_field(obj, dictionary, name, value)
    }

    fn update_dictionary_field(
        &mut self,
        obj: Id,
        dictionary: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<()> {
        let storage = self.dict_storage(dictionary).expect("instance dictionary");
        if let Some(value) = value {
            let key = self.text(name);
            self.put_item(storage, key, value)
        } else {
            self.delete_dict_attribute(obj, storage, name)
        }
    }

    /// Native member descriptors take precedence over entries in the instance dictionary.
    fn native_exception_descriptor(&self, error: &Error, name: &str) -> bool {
        let inherits = |kind| {
            self.builtins
                .get(kind)
                .is_some_and(|class| self.raised_error_matches(error, *class))
        };
        match name {
            "args"
            | "__class__"
            | "__context__"
            | "__cause__"
            | "__traceback__"
            | "__suppress_context__" => true,
            "value" => inherits("StopIteration"),
            "msg" => inherits("ImportError") || inherits("SyntaxError"),
            "name" => {
                inherits("ImportError") || inherits("AttributeError") || inherits("NameError")
            }
            "obj" => inherits("AttributeError"),
            "path" | "name_from" => inherits("ImportError"),
            "code" => inherits("SystemExit"),
            "filename" => inherits("OSError") || inherits("SyntaxError"),
            "errno" | "strerror" | "filename2" | "characters_written" => inherits("OSError"),
            "lineno"
            | "offset"
            | "text"
            | "end_lineno"
            | "end_offset"
            | "print_file_and_line"
            | "_metadata" => inherits("SyntaxError"),
            "encoding" | "object" | "start" | "end" | "reason" => {
                inherits("UnicodeEncodeError")
                    || inherits("UnicodeDecodeError")
                    || inherits("UnicodeTranslateError")
            }
            "message" | "exceptions" => inherits("BaseExceptionGroup"),
            _ => false,
        }
    }

    /// Find a Python-declared member on an exception that retains its original class.
    fn exception_member(&self, object: Id, name: &str) -> Option<Id> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        self.class_attr(error.class?, name)
    }

    /// Bind an override only if it precedes the built-in numeric method in the MRO.
    pub(super) fn bound_numeric_slot(&mut self, value: Id, slot: &str) -> Result<Option<Id>> {
        let class = match *self.heap.get(value) {
            Value::Instance { class, .. } => class,
            Value::Raised(_) => self.exact_type(value)?,
            _ => return Ok(None),
        };
        let method = self.find_in_mro(class, |base| match self.heap.get(base) {
            Value::Native(name) if matches!(name.as_str(), "int" | "float") => Some(None),
            _ => self.own_class_attr(base, slot).map(Some),
        });
        method
            .flatten()
            .map(|method| self.bind_attribute(method, value, class))
            .transpose()
    }

    /// Special methods are looked up on the type, even if the instance shadows them.
    pub(super) fn slot(&self, object: Id, name: &str) -> Option<Id> {
        self.class_attr(self.heap.get(object).instance_class()?, name)
    }

    /// Construct a cache with an independently traced dictionary and keyword marker.
    pub(super) fn cached_function(&mut self, function: Id) -> Id {
        let mark = self.heap.temporary_mark();
        self.heap.keep(function);
        let table = self.dict(Vec::new());
        self.heap.keep(table);
        let marker = self.alloc(Value::Native("functools._cache_marker".into()));
        self.heap.keep(marker);
        let result = self.alloc(Value::Cached {
            function,
            entries: Vec::new(),
            table,
            marker,
        });
        self.heap.release_temporaries_since(mark);
        result
    }

    /// Return dictionary storage when the receiver still uses the builtin iterator.
    pub(super) fn raw_dict_source(&self, value: Id) -> Option<Id> {
        self.dict_storage(value).filter(|_| {
            self.slot(value, "__iter__").is_none_or(
                |slot| matches!(self.heap.get(slot), Value::Native(name) if name == "dict.__iter__"),
            )
        })
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
        if let Some(storage) = self.raw_dict_source(input) {
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
                | Value::Range(_)
                | Value::Iterator { .. }
                | Value::DictIterator(_)
                | Value::MappingViewIterator { .. }
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
            Value::Instance { class, .. }
            | Value::Module {
                constructor: Some(class),
                ..
            } => *class,
            Value::Raised(Error {
                class: Some(class), ..
            }) if matches!(name, "__dir__" | "__str__") => *class,
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
                | Value::ItemGetter(_)
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
            Value::Property(_) | Value::GetSetDescriptor { .. } | Value::WeakrefDescriptor(_)
        )
    }

    fn data_descriptor(&self, descriptor: Id) -> bool {
        self.native_data_descriptor(descriptor)
            || self.slot(descriptor, "__set__").is_some()
            || self.slot(descriptor, "__delete__").is_some()
    }

    /// Bind the inherited dictionary constructor to the class used for the result.
    fn bind_dict_fromkeys(&mut self, owner: Id) -> Id {
        let callable = self.builtin("dict.fromkeys");
        self.alloc(Value::Method {
            callable,
            receiver: owner,
            explicit: false,
        })
    }

    pub(super) fn bind_attribute(&mut self, value: Id, receiver: Id, owner: Id) -> Result<Id> {
        if matches!(self.heap.get(value), Value::Native(name) if name == "dict.fromkeys") {
            return Ok(self.bind_dict_fromkeys(owner));
        }
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
            && let Some(slot) = Wrapper::lookup(symbol)
        {
            return slot.bind(self, value, receiver);
        }
        if receiver != NONE {
            let method = match self.heap.get(value) {
                Value::Property(property) => return property.read(self, receiver),
                Value::WeakrefDescriptor(owner) => return self.weakref_list(*owner, receiver),
                Value::GetSetDescriptor { get, .. } => {
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
                            | "enum.Enum.__copy__"
                            | "enum.Enum.__deepcopy__"
                            | "object.__setattr__"
                            | "object.__delattr__"
                            | "object.__init__"
                            | "BaseException.__init__"
                            | "ImportError.__init__"
                            | "OSError.__init__"
                            | "StopIteration.__init__"
                            | "module.__init__"
                            | "bytes.decode"
                            | "bytes.rfind"
                            | "bytes.rsplit"
                            | "bytes.rstrip"
                            | "int.to_bytes"
                            | "list.append"
                            | "list.index"
                            | "list.pop"
                            | "list.clear"
                            | "list.copy"
                            | "list.reverse"
                            | "list.sort"
                            | "map.__iter__"
                            | "map.__next__"
                    ) || name
                        .strip_prefix("weakref.ReferenceType.")
                        .is_some_and(|method| {
                            method != "__new__" && weakref::REFERENCE_METHODS.contains(&method)
                        })
                        || name.starts_with("property.")
                        || name.starts_with("dict.")
                        || name.starts_with("abc.")
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

    pub(super) fn instance_attribute(
        &mut self,
        object: Id,
        class: Id,
        field: Option<Id>,
        name: &str,
    ) -> Result<Id> {
        let member = self.class_attr(class, name);
        if member.is_none()
            && matches!(name, "fget" | "fset" | "fdel" | "__name__")
            && let Some(property) = self.heap.get(object).property().copied()
            && let Some(value) = property.attribute(self, name)?
        {
            return Ok(value);
        }
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
            (None, _)
                if name == "__callback__"
                    && let Some(wr) = self.heap.get(object).weak_reference() =>
            {
                Ok(wr.callback)
            }
            (_, Some(field)) => Ok(field),
            (Some(member), None) => self.bind_attribute(member, object, class),
            (None, None) if matches!(name, "__getattribute__" | "__delattr__") => {
                let method = self.builtin(&format!("object.{name}"));
                self.bind_attribute(method, object, class)
            }
            (None, None) => self.builtin_attribute(object, name),
        }
    }

    pub(super) fn python_subclass_check(&mut self, child: Id, parent: Id) -> Result<bool> {
        if let Value::Union(union) = self.heap.get(parent) {
            return self.python_subclass_check(child, union.args);
        }
        if let Value::Tuple(classes) = self.heap.get(parent) {
            for class in classes.clone() {
                if self.python_subclass_check(child, class)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if let Value::GenericAlias(alias) = self.heap.get(parent) {
            return Err(alias.kind.class_check_error("issubclass"));
        }
        if let Some(check) = self.bound_slot(parent, "__subclasscheck__")? {
            let result = self.call(check, vec![child], Vec::new())?;
            return self.truth(result);
        }
        let is_class = |id| {
            matches!(self.builtin_type_name(id), Some("type" | "typing._AnyMeta"))
                || matches!(self.heap.get(id), Value::Native(name) if super::builtins::is_builtin_type(name))
        };
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
        if self.has_abc_subclass_hook(parent)
            && let Some(result) = self.abc_hook_result(child, parent)?
        {
            return Ok(result);
        }
        Ok(self.subclass(child, parent))
    }

    pub(super) fn subclass(&self, class: Id, target: Id) -> bool {
        self.direct_subclass(class, target)
            || super::mapping_abc::registered_subclass(self, class, target)
    }

    /// Test native or C3 inheritance without traversing the ABC registry.
    pub(super) fn direct_subclass(&self, class: Id, target: Id) -> bool {
        if self.equal_class(class, target) || self.structural_context_manager(class, target) {
            return true;
        }
        if let Value::Class { ancestors, .. } = self.heap.get(class) {
            return ancestors.iter().any(|&base| {
                self.equal_class(base, target)
                    || (matches!(self.heap.get(base), Value::Native(_))
                        && self.direct_subclass(base, target))
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
                    | ("collections.defaultdict", "dict")
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
    pub(super) fn namedtuple_fields(&self, class: Id) -> Option<std::rc::Rc<[String]>> {
        match self.heap.get(class) {
            Value::Class {
                namedtuple_fields, ..
            } => namedtuple_fields.clone(),
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
            | Value::Range(_)
            | Value::RangeIterator(_)
            | Value::Slice(_)
            | Value::Enumerate { .. }
            | Value::Iterator { .. }
            | Value::Tuple(_)
            | Value::DictIterator(_)
            | Value::MappingViewIterator { .. }
            | Value::CallableIterator(_)
            | Value::FormatIterator { .. }
            | Value::File(_)
            | Value::Class { .. }
            | Value::Function { .. }
            | Value::Traceback { .. }
            | Value::PythonFrame { .. }
            | Value::Code(_)
            | Value::Cell(_)
            | Value::Generator { .. }
            | Value::Module { .. }
            | Value::ThreadLocal(_)
            | Value::PrimitiveLock(_) => self.type_name(obj),
            Value::TextFile(_) => "_io.TextIOWrapper",
            Value::DefaultDict { .. } => "collections.defaultdict",
            Value::ByteArray(_) => "bytearray",
            Value::Map { .. } => "map",
            Value::Filter { .. } => "filter",
            Value::Weak(wr) => wr.kind.class(),
            Value::Zip { .. } => "zip",
            Value::ClassMethod(_) => "classmethod",
            Value::PartialPlaceholder => "functools._PlaceholderType",
            Value::Count { .. } => "itertools.count",
            Value::ReverseSequence { .. } => "reversed",
            Value::Deque(_) => super::deque::CLASS,
            Value::DequeIterator { reverse: true, .. } => super::deque::REVERSE_ITERATOR,
            Value::DequeIterator { .. } => super::deque::ITERATOR,
            Value::Repeat(_) => "itertools.repeat",
            Value::ItemGetter(_) => "operator.itemgetter",
            Value::StaticMethod { .. } => "staticmethod",
            Value::TypeVar(_) => "typing.TypeVar",
            Value::GenericAlias(alias) => alias.kind.class(),
            Value::Socket(_) => "socket.socket",
            Value::Union(_) => "typing.Union",
            Value::Property(_) => "property",
            Value::Super { .. } => "super",
            Value::GetSetDescriptor { .. } | Value::WeakrefDescriptor(_) => "getset_descriptor",
            Value::MappingProxy(_) => "mappingproxy",
            Value::Method { explicit: true, .. } => "method",
            Value::Method { callable, .. } => match self.heap.get(*callable) {
                Value::Function { .. } => "method",
                Value::Native(symbol) if Wrapper::lookup(symbol).is_some() => "method-wrapper",
                Value::Native(_) => "builtin_function_or_method",
                _ => return None,
            },
            Value::Native(name) if Wrapper::lookup(name).is_some() => "wrapper_descriptor",
            Value::Native(name) if self.builtins.get(name) == Some(&obj) => {
                if name == "typing.Any" {
                    return Some("typing._AnyMeta");
                }
                if name == "typing.Callable" {
                    return Some("typing._CallableType");
                }
                if builtins::is_builtin_type(name)
                    || builtins::is_iterator_type(name)
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
                            | "getset_descriptor"
                            | "mappingproxy"
                            | "functools._PlaceholderType"
                            | "operator.attrgetter"
                            | "operator.itemgetter"
                            | "collections.defaultdict"
                            | "array.array"
                            | super::deque::CLASS
                            | "weakref.ReferenceType"
                            | "weakref.ProxyType"
                            | "weakref.CallableProxyType"
                            | super::thread_local::CLASS
                            | super::thread_lock::CLASS
                            | "traceback"
                            | "frame"
                            | "typing.TypeVar"
                            | "typing._GenericAlias"
                            | "typing._LiteralGenericAlias"
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
                        "binascii.a2b_base64"
                            | "binascii.b2a_base64"
                            | "posix.fspath"
                            | "posix._path_normpath"
                            | "zlib.adler32"
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
            Value::Union(_) => Ok(super::union::class(self)),
            Value::TextFile(_) => {
                self.module("io")?;
                Ok(self.builtins["_io.TextIOWrapper"])
            }
            Value::Instance { class, .. }
            | Value::Module {
                constructor: Some(class),
                ..
            } => Ok(*class),
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
        if self.classes.get("typing.Union") == Some(&class) {
            return matches!(self.heap.get(value), Value::Union(_));
        }
        if self.classes.get("collections.abc.Callable") == Some(&class) {
            return self.callable(value);
        }
        if let Value::Tuple(classes) = self.heap.get(class) {
            return classes.iter().any(|&c| self.instance_of(value, c));
        }
        if let Some(actual) = self
            .builtin_type_name(value)
            .and_then(|name| self.builtins.get(name))
            && (super::mapping_abc::registered_subclass(self, *actual, class)
                || matches!(self.heap.get(value), Value::TextFile(_))
                    && self.direct_subclass(*actual, class))
        {
            return true;
        }
        if let Some(actual) = self.heap.get(value).instance_class()
            && self.subclass(actual, class)
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
            | ("collections.defaultdict", Value::DefaultDict { .. })
            | ("set", Value::Set { frozen: false, .. })
            | ("frozenset", Value::Set { frozen: true, .. })
            | ("slice", Value::Slice(_))
            | ("range", Value::Range(_))
            | ("map", Value::Map { .. })
            | ("filter", Value::Filter { .. })
            | ("selectors.PollSelector", Value::Selector(_))
            | ("contextvars.ContextVar", Value::ContextVar { .. })
            | ("contextvars.Context", Value::Context(_))
            | ("functools.partial", Value::Partial { .. })
            | ("contextvars.Token", Value::ContextToken { .. })
            | ("operator.attrgetter", Value::AttrGetter(_))
            | ("operator.itemgetter", Value::ItemGetter(_))
            | ("itertools.count", Value::Count { .. })
            | (super::deque::CLASS, Value::Deque(_))
            | (super::deque::ITERATOR, Value::DequeIterator { reverse: false, .. })
            | (super::deque::REVERSE_ITERATOR, Value::DequeIterator { reverse: true, .. })
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
        (error.class.is_none()
            && builtins::exception_subclass(&error.kind, kind).unwrap_or(error.kind == kind))
            || self
                .builtins
                .get(kind)
                .is_some_and(|&base| self.raised_error_matches(error, base))
    }

    /// Materialize native exception arguments before their backing fields can change.
    fn exception_argument_tuple(&mut self, obj: Id, error: Error) -> Id {
        if let Some(args) = error.args {
            return args;
        }
        let fields = if error.message.is_empty() {
            Vec::new()
        } else {
            vec![
                self.import_error_message(obj, None)
                    .unwrap_or_else(|| self.text(error.message)),
            ]
        };
        let tuple = self.alloc(Value::Tuple(fields));
        let Value::Raised(error) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        error.args = Some(tuple);
        tuple
    }

    /// Preserve ImportError's original message when its arguments are replaced.
    pub(super) fn import_error_message(&mut self, obj: Id, replacement: Option<Id>) -> Option<Id> {
        let Value::Raised(error) = self.heap.get(obj) else {
            return None;
        };
        if !self.is_builtin_error(error, "ImportError") {
            return None;
        }
        let message = if let Some(message) = replacement {
            if error.args.is_none() {
                self.exception_argument_tuple(obj, error.clone());
            }
            message
        } else {
            if let Some(message) = error.trace.as_ref().and_then(|trace| trace.import_message) {
                return Some(message);
            }
            match self.exception_arguments(error) {
                Some([one]) => *one,
                Some(_) => NONE,
                None => self.text(error.message.clone()),
            }
        };
        let Value::Raised(error) = self.heap.get_mut(obj) else {
            unreachable!()
        };
        error.tracing().import_message = Some(message);
        Some(message)
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
        let class = match *self.heap.get(obj) {
            Value::Instance { class, .. }
            | Value::Module {
                constructor: Some(class),
                ..
            } => class,
            _ => return self.default_get_attr(obj, name),
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
        if let Value::Union(union) = *self.heap.get(obj) {
            match name {
                "__args__" => return Ok(union.args),
                "__parameters__" => return Ok(union.parameters),
                "__class__" | "__origin__" => return Ok(super::union::class(self)),
                "__module__" => return Ok(self.text("typing")),
                "__name__" | "__qualname__" => return Ok(self.text("Union")),
                _ => {}
            }
        }
        if let Value::Instance {
            class,
            fields,
            namespace,
            ..
        } = self.heap.get(obj)
        {
            let (class, inline, namespace) = (*class, fields.get(name).copied(), *namespace);
            if matches!(name, "name" | "value")
                && let Some((_, member_name, value)) = self.enum_member(obj)
            {
                return Ok(if name == "name" { member_name } else { value });
            }
            let layout = self.instance_layout(class, name);
            if layout.active_slot {
                return inline.ok_or_else(|| self.missing_attribute(obj, name));
            }
            let member = self.class_attr(class, name);
            if name == "__dict__" && member.is_none() && layout.dictionary {
                return Ok(self.instance_dictionary(obj, class));
            }
            let inline = inline.filter(|_| !layout.declared_slot);
            let field = if inline.is_some()
                || namespace.is_none()
                || member.is_some_and(|member| self.data_descriptor(member))
            {
                inline
            } else {
                self.dictionary_field(namespace, name)?
            };
            return self.instance_attribute(obj, class, field, name);
        }
        if let Value::Raised(error) = self.heap.get(obj) {
            let dictionary = error.trace.as_ref().and_then(|trace| trace.dictionary);
            if let Some(class) = error.class
                && let Some(member) = self.class_attr(class, name)
            {
                let field = if self.data_descriptor(member) {
                    None
                } else {
                    self.dictionary_field(dictionary, name)?
                };
                return self.instance_attribute(obj, class, field, name);
            }
            if let Some(fields) = error
                .trace
                .as_ref()
                .and_then(|trace| trace.source_syntax.as_ref())
                && let Some(slot) = super::source_code::syntax_member(name)
            {
                return Ok(fields[slot].get());
            }
            if name == "__dict__" {
                return Ok(self.exception_dictionary(obj));
            }
        }
        if name == "value"
            && let Some(error) = self.stop_iteration_error(obj)
        {
            return Ok(error.stop_value());
        }
        if matches!(name, "__new__" | "__init__") && matches!(self.heap.get(obj), Value::Raised(_))
        {
            let class = self.exact_type(obj)?;
            return self.instance_attribute(obj, class, None, name);
        }
        if name == "__class__" {
            return self.exact_type(obj);
        }
        if let Value::Deque(ring) = self.heap.get(obj) {
            if name == "maxlen" {
                return Ok(ring.maxlen.map_or(NONE, |n| self.int(n as i64)));
            }
            return self.builtin_attribute(obj, name);
        }
        if name == "msg"
            && let Some(message) = self.import_error_message(obj, None)
        {
            return Ok(message);
        }
        if let Some(value) = super::typing::attribute(self, obj, name) {
            return value;
        }
        if let Some(value) = self.os_attribute(obj, name)? {
            return Ok(value);
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
                if ((builtins::is_builtin_type(&class) && !class.contains('.'))
                    || matches!(class.as_str(), "traceback" | "frame"))
                    && matches!(name, "__name__" | "__qualname__" | "__module__") =>
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
            Value::Native(ref class) if class == "module" => match name {
                "__doc__" => return Ok(self.text(module_type::DOC)),
                "__init__" | "__new__" => return Ok(self.builtin(&format!("module.{name}"))),
                "__name__" | "__qualname__" => return Ok(self.text("module")),
                "__module__" => return Ok(self.text("builtins")),
                _ => {}
            },
            Value::WeakrefDescriptor(owner) => match name {
                "__objclass__" => return Ok(owner),
                "__name__" => return Ok(self.text("__weakref__")),
                "__doc__" => return Ok(self.text("list of weak references to the object")),
                "__qualname__" => {
                    let name = self.lookup_attr(owner, "__qualname__")?;
                    return Ok(self.text(format!("{}.__weakref__", self.str_value(name)?)));
                }
                "__get__" | "__set__" | "__delete__" => {
                    let callable = self.builtin(&format!("weakref.descriptor.{name}"));
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: true,
                    }));
                }
                _ => {}
            },
            Value::Property(property) => {
                if let Some(attribute) = property.attribute(self, name)? {
                    return Ok(attribute);
                }
            }
            Value::Native(symbol)
                if matches!(
                    symbol.as_str(),
                    "posix.fspath" | "posix._path_normpath" | "zlib.adler32"
                ) =>
            {
                let (module, function) = symbol.split_once('.').unwrap();
                match name {
                    "__name__" | "__qualname__" => return Ok(self.text(function)),
                    "__module__" => return Ok(self.text(module)),
                    "__self__" => return self.module(module),
                    _ => {}
                }
            }
            Value::Native(class) if class == "staticmethod" && name == "__get__" => {
                return Ok(self.builtin("staticmethod.__get__"));
            }
            Value::Native(class)
                if class == "property" && super::property::METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("property.{name}")));
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
            Value::Cell(contents) if name == "cell_contents" => {
                return contents.ok_or_else(|| Error::new("ValueError", "Cell is empty"));
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
                && let Some(slot) = Wrapper::lookup(symbol) =>
            {
                if name == "__self__" {
                    return Ok(receiver);
                }
                if let Some(value) = slot.attribute(self, name) {
                    return Ok(value);
                }
            }
            Value::Native(ref symbol) if let Some(slot) = Wrapper::lookup(symbol) => {
                if let Some(value) = slot.attribute(self, name) {
                    return Ok(value);
                }
                if name == "__get__" {
                    let callable = self.builtin("wrapper_descriptor.__get__");
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
            Value::Native(class)
                if matches!(
                    class.as_str(),
                    "array.array"
                        | super::deque::CLASS
                        | super::deque::ITERATOR
                        | super::deque::REVERSE_ITERATOR
                ) =>
            {
                let (module, short) = class.rsplit_once('.').unwrap();
                match name {
                    "__name__" | "__qualname__" => {
                        return Ok(self.text(short));
                    }
                    "__module__" => return Ok(self.text(module)),
                    _ if class == super::deque::CLASS && super::deque::METHODS.contains(&name) => {
                        return Ok(self.builtin(&format!("{class}.{name}")));
                    }
                    _ => {}
                }
            }
            Value::Native(class) if class == "tuple" && name == "__getitem__" => {
                return Ok(self.builtin("tuple.__getitem__"));
            }
            Value::Native(class) if matches!(class.as_str(), "map" | "filter") => match name {
                "__iter__" | "__next__" => return Ok(self.builtin(&format!("{class}.{name}"))),
                "__name__" | "__qualname__" => return Ok(self.text(&class)),
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
            Value::Raised(error)
                if let Some(index) = builtins::IMPORT_KEYWORDS
                    .iter()
                    .position(|&key| key == name)
                    && self.is_builtin_error(&error, "ImportError") =>
            {
                let Some(fields) = error.import else {
                    return Ok(error
                        .trace
                        .as_ref()
                        .and_then(|trace| trace.import_names)
                        .map_or(NONE, |names| names[index]));
                };
                let value = match name {
                    "name" => fields.name.as_deref(),
                    "path" => fields.path.as_deref(),
                    _ => fields.name_from.as_deref(),
                };
                return Ok(value.map_or(NONE, |value| self.text(value)));
            }
            Value::Raised(error) => {
                if let Some(state) = &error.decode {
                    match name {
                        "encoding" => return Ok(state.encoding),
                        "object" => return Ok(state.object),
                        "start" => return Ok(self.int(state.start)),
                        "end" => return Ok(self.int(state.end)),
                        "reason" => return Ok(state.reason),
                        _ => {}
                    }
                }
                if name == "args" {
                    return Ok(self.exception_argument_tuple(obj, error));
                }
                if error.kind == "UnicodeDecodeError"
                    && let Some([encoding, object, start, end, reason]) =
                        self.exception_arguments(&error)
                {
                    match name {
                        "encoding" => return Ok(*encoding),
                        "object" => return Ok(*object),
                        "start" => return Ok(*start),
                        "end" => return Ok(*end),
                        "reason" => return Ok(*reason),
                        _ => {}
                    }
                }
            }
            Value::DefaultDict { factory, .. } if name == "default_factory" => return Ok(factory),
            Value::Native(class) if class == "collections.defaultdict" && name == "__missing__" => {
                return Ok(self.builtin("collections.defaultdict.__missing__"));
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
            Value::Weak(_) if weakref::REFERENCE_METHODS.contains(&name) => {
                let callable = self.builtin(&format!("{}.{name}", weakref::REFERENCE_TYPE));
                let class = self.builtin(weakref::REFERENCE_TYPE);
                return self.bind_attribute(callable, obj, class);
            }
            Value::Native(ref class)
                if class == weakref::REFERENCE_TYPE
                    && weakref::REFERENCE_METHODS.contains(&name) =>
            {
                return Ok(self.builtin(&format!("{}.{name}", weakref::REFERENCE_TYPE)));
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
            Value::Native(method)
                if method.starts_with("abc.abstract.") && name == "__isabstractmethod__" =>
            {
                return Ok(TRUE);
            }
            Value::Native(class) if class == "contextvars.Token" && name == "MISSING" => {
                return Ok(self.token_missing);
            }
            Value::Native(class)
                if matches!(class.as_str(), "bytes" | "bytearray")
                    && matches!(name, "__repr__" | "maketrans" | "translate") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Bytes(_) if name == "maketrans" => return Ok(self.builtin("bytes.maketrans")),
            Value::ByteArray(_) if name == "maketrans" => {
                return Ok(self.builtin("bytearray.maketrans"));
            }
            Value::Native(class) if class == "bool" && name == "to_bytes" => {
                return Ok(self.builtin("int.to_bytes"));
            }
            Value::Native(class) if class == "collections.defaultdict" && name == "clear" => {
                return Ok(self.builtin("dict.clear"));
            }
            Value::Native(class)
                if class == "collections.defaultdict" && matches!(name, "copy" | "__copy__") =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
            }
            Value::Native(class)
                if name == "__reduce__"
                    && matches!(
                        class.as_str(),
                        "set" | "frozenset" | "collections.defaultdict"
                    ) =>
            {
                return Ok(self.builtin(&format!("{class}.__reduce__")));
            }
            Value::Native(class)
                if name == "fromkeys"
                    && matches!(class.as_str(), "dict" | "collections.defaultdict") =>
            {
                return Ok(self.bind_dict_fromkeys(obj));
            }
            Value::Dict(_) | Value::DefaultDict { .. } if name == "fromkeys" => {
                let owner = self.exact_type(obj)?;
                return Ok(self.bind_dict_fromkeys(owner));
            }
            Value::Native(class) if class == "set" && name == "copy" => {
                return Ok(self.builtin("set.copy"));
            }
            Value::Native(class) if class == "set" && name == "discard" => {
                return Ok(self.builtin("set.discard"));
            }
            Value::Native(class)
                if class == "list"
                    && matches!(
                        name,
                        "clear" | "copy" | "index" | "pop" | "reverse" | "sort"
                    ) =>
            {
                return Ok(self.builtin(&format!("list.{name}")));
            }
            Value::Native(class)
                if matches!(class.as_str(), "dict" | "list" | "tuple")
                    || builtins::is_enum_base(&class)
                    || (name == "__repr__" && matches!(class.as_str(), "set" | "frozenset"))
                    || (builtins::is_exception(&class)
                        && matches!(name, "__new__" | "__init__"))
                    || (class == "bytes" && builtins::BYTES_METHODS.contains(&name))
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
                        "__hash__"
                            | "__getattribute__"
                            | "__setattr__"
                            | "__delattr__"
                            | "__init__"
                    ) =>
            {
                return Ok(self.builtin(&format!("object.{name}")));
            }
            Value::Native(class)
                if class == "slice" && matches!(name, "indices" | "__reduce__") =>
            {
                return Ok(self.builtin(&format!("slice.{name}")));
            }
            Value::Native(class) if class == "operator.itemgetter" && name == "__reduce__" => {
                return Ok(self.builtin("operator.itemgetter.__reduce__"));
            }
            Value::Native(class)
                if (class == "range" && matches!(name, "__iter__" | "__reversed__"))
                    || (matches!(class.as_str(), "range_iterator" | "longrange_iterator")
                        && matches!(name, "__iter__" | "__next__" | "__length_hint__")) =>
            {
                return Ok(self.builtin(&format!("{class}.{name}")));
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
                constructor,
                ..
            } => {
                if name == "__dict__" {
                    return Ok(self.module_globals(scope));
                }
                if name == "__class__" {
                    return Ok(constructor.unwrap_or_else(|| self.builtin("module")));
                }
                if let Some(class) = constructor {
                    return self.module_instance_attribute(obj, class, scope, name);
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
                if name == "__dict__" {
                    return Ok(self.class_dictionary(scope));
                }
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
                    let assigned = self
                        .scope_data(scope)
                        .class_namespace
                        .as_ref()
                        .and_then(|namespace| namespace.name_override);
                    return Ok(assigned.unwrap_or_else(|| self.heap.code_name(&class_name)));
                }
                if let Some(value) = self.class_attr(obj, name) {
                    return self.bind_attribute(value, NONE, obj);
                }
                if name == "register"
                    && (super::mapping_abc::is_abc_class(self, obj) || self.is_bundled_abc(obj))
                {
                    let callable = self.builtin("abc.register");
                    return Ok(self.alloc(Value::Method {
                        callable,
                        receiver: obj,
                        explicit: true,
                    }));
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
        if let Value::Raised(error) = self.heap.get(obj)
            && !self.native_exception_descriptor(error, name)
        {
            let dictionary = error.trace.as_ref().and_then(|trace| trace.dictionary);
            if let Some(field) = self.dictionary_field(dictionary, name)? {
                return Ok(field);
            }
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
        if name == "cell_contents" && matches!(self.heap.get(obj), Value::Cell(_)) {
            self.heap.replace_cell(obj, None);
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_base_local_attribute(obj, name, None);
        }
        let exception_member = self.exception_member(obj, name);
        let member = self.writable_attribute_member(obj, name);
        if name == "__callback__"
            && member.is_none()
            && self.heap.get(obj).weak_reference().is_some()
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Some(descriptor) = member.or(exception_member)
            && self.data_descriptor(descriptor)
        {
            match *self.heap.get(descriptor) {
                Value::Property(property) => return property.remove(self, obj),
                Value::WeakrefDescriptor(owner) => return Err(self.readonly_weakref(owner)),
                Value::GetSetDescriptor { set: Some(set), .. } => {
                    self.call(set, vec![obj], Vec::new())?;
                    return Ok(());
                }
                Value::GetSetDescriptor { owner, name, .. } => {
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
        if self.update_syntax_member(obj, name, NONE) {
            return Ok(());
        }
        if exception_member.is_some() {
            return self.update_exception_field(obj, name, None);
        }
        if self.update_os_field(obj, name, None)? {
            return Ok(());
        }
        if name == "__dict__" && matches!(self.heap.get(obj), Value::Raised(_)) {
            return Err(Error::new("TypeError", "cannot delete __dict__"));
        }
        if name == "value"
            && let Some(error) = self.stop_iteration_error(obj)
        {
            error.set_stop_value(NONE);
            return Ok(());
        }
        if name == "msg" && self.import_error_message(obj, Some(NONE)).is_some() {
            return Ok(());
        }
        if name == "args" && matches!(self.heap.get(obj), Value::Raised(_)) {
            return Err(Error::new("TypeError", "args may not be deleted"));
        }
        if name == "__class__" && member.is_none() {
            return Err(Error::new("TypeError", "can't delete __class__ attribute"));
        }
        if member.is_none()
            && matches!(name, "__name__" | "fget" | "fset" | "fdel")
            && let Some(property) = self.heap.get_mut(obj).property_mut()
        {
            if name == "__name__" {
                property.name = None;
                return Ok(());
            }
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if let Value::Raised(error) = self.heap.get(obj)
            && !self.native_exception_descriptor(error, name)
        {
            return self.update_exception_field(obj, name, None);
        }
        self.check_instance_field(obj, name)?;
        match *self.heap.get(obj) {
            Value::Function { fields, .. } => self.delete_function_attr(obj, fields, name),
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                Err(Error::new("AttributeError", "readonly attribute"))
            }
            Value::Property(_) if name == "__doc__" => {
                self.heap.get_mut(obj).property_mut().unwrap().doc = NONE;
                Ok(())
            }
            Value::StaticMethod { dictionary, .. } => {
                super::static_method::update_attribute(self, obj, dictionary, name, None)
            }
            Value::Module {
                scope, constructor, ..
            } => {
                if constructor.is_some() {
                    self.update_module_annotations(scope, name, None)?;
                }
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
            Value::Class { scope, .. } => {
                let error = match name {
                    "__name__" | "__qualname__" | "__bases__" | "__type_params__"
                    | "__module__" | "__doc__" => Error::new(
                        "TypeError",
                        format!(
                            "cannot delete '{name}' attribute of immutable type '{}'",
                            self.class_name(obj)
                        ),
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
                        Error::new(
                            "AttributeError",
                            format!(
                                "type object '{}' has no attribute '{name}'",
                                self.class_name(obj)
                            ),
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
                let layout = self.instance_layout(class, name);
                if name == "__dict__" && member.is_none() {
                    if !layout.dictionary {
                        return Err(self.missing_attribute(obj, name));
                    }
                    self.replace_instance_dictionary(obj, class, None);
                    return Ok(());
                }
                if !layout.active_slot
                    && let Value::Instance {
                        namespace: Some(storage),
                        ..
                    } = *self.heap.get(obj)
                {
                    return self.update_dictionary_field(obj, storage, name, None);
                }
                if layout.declared_slot && !layout.active_slot {
                    return Err(self.missing_attribute(obj, name));
                }
                let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
                    unreachable!()
                };
                let value = fields
                    .shift_remove(name)
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

    /// Whether instances have a dictionary and whether this particular name addresses a slot.
    fn instance_layout(&self, class: Id, name: &str) -> InstanceLayout {
        let mut layout = InstanceLayout::default();
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return layout;
        };
        let mut shadowed = false;
        for &base in std::iter::once(&class).chain(ancestors) {
            let Value::Class {
                scope, name: owner, ..
            } = self.heap.get(base)
            else {
                continue;
            };
            let frame = self.scope_data(*scope);
            shadowed |= frame.names.contains_key(name);
            let Some(slots) = frame
                .class_namespace
                .as_deref()
                .and_then(|namespace| namespace.slot_names.as_deref())
            else {
                layout.dictionary = true;
                continue;
            };
            for slot in slots {
                match slot.as_str() {
                    "__dict__" => layout.dictionary = true,
                    "__weakref__" => {}
                    slot => {
                        let owner = owner.trim_start_matches('_');
                        let matches = if slot.starts_with("__")
                            && !slot.ends_with("__")
                            && !owner.is_empty()
                        {
                            name.strip_prefix('_').and_then(|n| n.strip_prefix(owner)) == Some(slot)
                        } else {
                            name == slot
                        };
                        if matches {
                            layout.declared_slot = true;
                            layout.active_slot |= !shadowed;
                        }
                    }
                }
            }
        }
        layout
    }

    fn writable_attribute_member(&self, obj: Id, name: &str) -> Option<Id> {
        let member = self.slot(obj, name)?;
        if let Value::Instance { class, .. } = *self.heap.get(obj)
            && self.instance_layout(class, name).active_slot
        {
            return None;
        }
        Some(member)
    }

    /// Move ordinary fields out of the inline storage while keeping declared slots in place.
    fn take_instance_fields(&mut self, obj: Id, class: Id) -> Vec<(String, Id)> {
        let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        let original = std::mem::take(fields);
        let mut ordinary = Vec::new();
        let mut retained = indexmap::IndexMap::new();
        for (name, value) in original {
            if self.instance_layout(class, &name).declared_slot
                || self
                    .namedtuple_fields(class)
                    .is_some_and(|names| names.contains(&name))
            {
                retained.insert(name, value);
            } else {
                ordinary.push((name, value));
            }
        }
        let Value::Instance { fields, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        *fields = retained;
        ordinary
    }

    /// Look up native backing fields on either side of ordinary dictionary materialization.
    pub(super) fn raw_instance_field(&self, obj: Id, name: &str) -> Option<Id> {
        let Value::Instance {
            fields, namespace, ..
        } = self.heap.get(obj)
        else {
            return None;
        };
        fields
            .get(name)
            .copied()
            .or_else(|| self.dict_named_item(self.dict_storage((*namespace)?)?, name))
    }

    /// Lazily expose the dictionary that subsequent ordinary attribute operations use.
    fn instance_dictionary(&mut self, obj: Id, class: Id) -> Id {
        if let Value::Instance {
            namespace: Some(dictionary),
            ..
        } = *self.heap.get(obj)
        {
            return dictionary;
        }
        let entries = self
            .take_instance_fields(obj, class)
            .into_iter()
            .map(|(name, value)| (self.text(name), value))
            .collect();
        let dictionary = self.dict(entries);
        let Value::Instance { namespace, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        *namespace = Some(dictionary);
        dictionary
    }

    fn replace_instance_dictionary(&mut self, obj: Id, class: Id, new: Option<Id>) {
        let removed = self.take_instance_fields(obj, class);
        let Value::Instance { namespace, .. } = self.heap.get_mut(obj) else {
            unreachable!()
        };
        let previous = std::mem::replace(namespace, new);
        if previous != new
            && let Some(old) = previous
        {
            self.heap.weak_edge_removed(old);
        }
        for (_, value) in removed {
            self.heap.weak_edge_removed(value);
        }
    }

    /// A slot-only instance can retain fields declared by any of its Python bases.
    fn check_instance_field(&mut self, obj: Id, name: &str) -> Result<()> {
        let Value::Instance { class, .. } = *self.heap.get(obj) else {
            return Ok(());
        };
        if !matches!(self.heap.get(class), Value::Class { .. }) {
            return Ok(());
        }
        let layout = self.instance_layout(class, name);
        if layout.dictionary || layout.active_slot {
            return Ok(());
        }
        let attribute = self.text(name);
        let quoted = self.repr(attribute);
        let kind = self.type_name(obj);
        let message = if self.class_attr(class, name).is_some() {
            format!("'{kind}' object attribute {quoted} is read-only")
        } else {
            format!(
                "'{kind}' object has no attribute {quoted} and no __dict__ for setting new attributes"
            )
        };
        Err(Error::new("AttributeError", message))
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
        if matches!(
            self.heap.get(obj),
            Value::Raised(Error {
                decode: Some(_),
                ..
            })
        ) && matches!(name, "encoding" | "object" | "start" | "end" | "reason")
        {
            let offset = if matches!(name, "start" | "end") {
                let integer = self
                    .as_int(value)
                    .map_err(|_| Error::new("TypeError", "an integer is required"))?;
                Some(
                    integer
                        .to_i64()
                        .and_then(|n| isize::try_from(n).ok())
                        .ok_or_else(|| {
                            Error::new(
                                "OverflowError",
                                "Python int too large to convert to C ssize_t",
                            )
                        })? as i64,
                )
            } else {
                None
            };
            let Value::Raised(Error {
                decode: Some(state),
                ..
            }) = self.heap.get_mut(obj)
            else {
                unreachable!();
            };
            match name {
                "encoding" => state.encoding = value,
                "object" => state.object = value,
                "start" => state.start = offset.unwrap(),
                "end" => state.end = offset.unwrap(),
                "reason" => state.reason = value,
                _ => unreachable!(),
            }
            return Ok(());
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
        if name == "__callback__"
            && self.slot(obj, name).is_none()
            && self.heap.get(obj).weak_reference().is_some()
        {
            return Err(Error::new("AttributeError", "readonly attribute"));
        }
        if matches!(self.heap.get(obj), Value::ThreadLocal(_)) {
            return self.update_base_local_attribute(obj, name, Some(value));
        }
        if name == "__abstractmethods__"
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            return self.set_abstract_methods(scope, value);
        }
        if name == "__name__"
            && let Value::Class { scope, .. } = *self.heap.get(obj)
        {
            let text = self.str_value(value).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "can only assign string to {}.__name__, not '{}'",
                        self.class_name(obj),
                        self.type_name(value)
                    ),
                )
            })?;
            if text.contains('\0') {
                return Err(Error::new(
                    "ValueError",
                    "type name must not contain null characters",
                ));
            }
            let Value::Scope(frame) = self.heap.get_mut(scope) else {
                unreachable!()
            };
            let old = frame
                .class_namespace
                .get_or_insert_with(Default::default)
                .name_override
                .replace(value);
            if let Some(old) = old
                && old != value
            {
                self.heap.weak_edge_removed(old);
            }
            return Ok(());
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
        if name == "cell_contents" && matches!(self.heap.get(obj), Value::Cell(_)) {
            self.heap.replace_cell(obj, Some(value));
            return Ok(());
        }
        if matches!(self.heap.get(obj), Value::Function { .. }) {
            return self.set_function_attr(obj, name, value);
        }
        let exception_member = self.exception_member(obj, name);
        let member = self.writable_attribute_member(obj, name);
        if let Some(descriptor) = member.or(exception_member)
            && self.data_descriptor(descriptor)
        {
            if let Value::Property(property) = *self.heap.get(descriptor) {
                return property.write(self, obj, value);
            }
            if let Value::WeakrefDescriptor(owner) = *self.heap.get(descriptor) {
                return Err(self.readonly_weakref(owner));
            }
            if let Value::GetSetDescriptor {
                owner, name, set, ..
            } = *self.heap.get(descriptor)
            {
                if let Some(set) = set {
                    self.call(set, vec![obj, value], Vec::new())?;
                    return Ok(());
                }
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
        if self.update_syntax_member(obj, name, value) {
            return Ok(());
        }
        if exception_member.is_some() {
            return self.update_exception_field(obj, name, Some(value));
        }
        if self.update_os_field(obj, name, Some(value))? {
            return Ok(());
        }
        if name == "__dict__" && matches!(self.heap.get(obj), Value::Raised(_)) {
            if self.dict_storage(value).is_none() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__dict__ must be set to a dictionary, not a '{}'",
                        self.type_name(value)
                    ),
                ));
            }
            let Value::Raised(error) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            error.tracing().dictionary = Some(value);
            return Ok(());
        }
        if name == "value"
            && let Some(error) = self.stop_iteration_error(obj)
        {
            error.set_stop_value(value);
            return Ok(());
        }
        if member.is_none()
            && matches!(name, "__name__" | "fget" | "fset" | "fdel")
            && let Some(property) = self.heap.get_mut(obj).property_mut()
        {
            match name {
                "__name__" => {
                    property.name = Some(value);
                    return Ok(());
                }
                "fget" | "fset" | "fdel" => {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                _ => {}
            }
        }
        if name == "msg" && self.import_error_message(obj, Some(value)).is_some() {
            return Ok(());
        }
        if name == "args" && matches!(self.heap.get(obj), Value::Raised(_)) {
            let tuple = if matches!(self.heap.get(value), Value::Tuple(_)) {
                value
            } else {
                let iter = self.iterator(value)?;
                self.heap.keep(iter);
                let mut items = Vec::new();
                while let Some(item) = self.next_item(iter)? {
                    self.heap.keep(item);
                    items.push(item);
                }
                self.alloc(Value::Tuple(items))
            };
            let _ = self.import_error_message(obj, None);
            let Value::Raised(error) = self.heap.get_mut(obj) else {
                unreachable!()
            };
            error.args = Some(tuple);
            return Ok(());
        }
        if let Value::Raised(error) = self.heap.get(obj)
            && !self.native_exception_descriptor(error, name)
        {
            return self.update_exception_field(obj, name, Some(value));
        }
        self.check_instance_field(obj, name)?;
        if let Value::Instance {
            class, namespace, ..
        } = *self.heap.get(obj)
        {
            let layout = self.instance_layout(class, name);
            if layout.dictionary && name == "__dict__" && member.is_none() {
                if self.dict_storage(value).is_none() {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "__dict__ must be set to a dictionary, not a '{}'",
                            self.type_name(value)
                        ),
                    ));
                }
                self.replace_instance_dictionary(obj, class, Some(value));
                return Ok(());
            }
            if !layout.active_slot {
                let storage = namespace.or_else(|| {
                    layout
                        .declared_slot
                        .then(|| self.instance_dictionary(obj, class))
                });
                if let Some(storage) = storage {
                    return self.update_dictionary_field(obj, storage, name, Some(value));
                }
            }
        }
        let object_class = self.builtins.get("object").copied();
        match self.heap.get_mut(obj) {
            Value::DefaultDict { factory, .. } if name == "default_factory" => {
                let old = std::mem::replace(factory, value);
                self.heap.weak_edge_removed(old);
            }
            Value::Code(_) if matches!(name, "co_name" | "co_filename" | "co_firstlineno") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Partial {
                coroutine_marker, ..
            } if name == "_is_coroutine_marker" => {
                *coroutine_marker = Some(value);
            }
            Value::Property(property) if name == "__doc__" => property.doc = value,
            Value::Complex(_) if matches!(name, "real" | "imag") => {
                return Err(Error::new("AttributeError", "readonly attribute"));
            }
            Value::Module {
                name: module,
                scope,
                constructor,
                ..
            } => {
                if name == "__dict__" {
                    return Err(Error::new("AttributeError", "readonly attribute"));
                }
                if constructor.is_none() && module == "sys" && name == "path" {
                    self.sys_path = value;
                }
                let scope = *scope;
                if constructor.is_some() {
                    self.update_module_annotations(scope, name, Some(value))?;
                }
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
                if name == "__dict__" {
                    return Err(Error::new(
                        "AttributeError",
                        "attribute '__dict__' of 'type' objects is not writable",
                    ));
                }
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
        let property = self.has_base(class, "property").then(Box::default);
        self.alloc(Value::Instance {
            namespace: None,
            class,
            dictionary,
            property,
            weak_reference: None,
            memory_stream: None,
            fields: Default::default(),
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
            namespace: None,
            class,
            fields: Default::default(),
            scalar: Some(value),
            dictionary: None,
            property: None,
            weak_reference: None,
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
            namespace: None,
            class,
            fields: Default::default(),
            dictionary: None,
            scalar: Some(text),
            property: None,
            weak_reference: None,
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
                weakref::Kind::Reference => wr.call(&args, &kwargs),
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
            Value::Native(name) if name == "wrapper_descriptor.__get__" => {
                wrapper_descriptor::descriptor_get(self, &args, &kwargs)
            }
            Value::Native(name) if name == "int.__new__" => self.new_int(&args, &kwargs),
            Value::Native(name) if name == "float.__new__" => self.new_float(&args, &kwargs),
            Value::Native(name)
                if builtins::is_builtin(&name)
                    || builtins::constructor_owner(&name).is_some()
                    || matches!(
                        name.as_str(),
                        "BaseException.__init__"
                            | "ImportError.__init__"
                            | "OSError.__init__"
                            | "StopIteration.__init__"
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
                                | "filter"
                                | "set"
                                | "int"
                                | "frozenset"
                                | "tuple"
                                | "bytes"
                                | "bytearray"
                                | "range"
                                | "range_iterator"
                                | "longrange_iterator"
                                | "module"
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
            Value::ItemGetter(keys) => self.apply_itemgetter(keys, &args, &kwargs),
            Value::Cached {
                function,
                table,
                marker,
                ..
            } => {
                let mark = self.heap.temporary_mark();
                let result = (|| -> Result<Id> {
                    self.heap.keep(callable);
                    self.heap.keep(table);
                    let fast = args.len() == 1
                        && kwargs.is_empty()
                        && matches!(self.heap.get(args[0]), Value::Int(_) | Value::Str(_));
                    let mut parts = args.clone();
                    if !kwargs.is_empty() {
                        parts.push(marker);
                        for (name, value) in &kwargs {
                            let name = self.text(name.clone());
                            self.heap.keep(name);
                            parts.push(name);
                            parts.push(*value);
                        }
                    }
                    for &part in &parts {
                        self.heap.keep(part);
                    }
                    let key = if fast {
                        args[0]
                    } else {
                        self.alloc(Value::Tuple(parts))
                    };
                    self.heap.keep(key);
                    let hash = self.dictionary_hash(key)?;
                    if let Some(index) = self.find_dictionary_key(table, key, Some(hash))?.1 {
                        let (resident, value) = self.dict_entries(table).unwrap()[index];
                        let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                            unreachable!()
                        };
                        if let Some(i) = entries.iter().position(|call| call.key == resident) {
                            let hit = entries.remove(i);
                            entries.push(hit);
                        }
                        return Ok(value);
                    }
                    let result = self.call(function, args.clone(), kwargs.clone())?;
                    self.heap.keep(result);
                    if self
                        .find_dictionary_key(table, key, Some(hash))?
                        .1
                        .is_some()
                    {
                        return Ok(result);
                    }
                    let evicted = {
                        let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                            unreachable!()
                        };
                        if entries.len() >= 128 {
                            Some(entries.remove(0))
                        } else {
                            None
                        }
                    };
                    if let Some(old) = evicted {
                        self.heap.keep(old.key);
                        if let Some(i) = self.find_dictionary_key(table, old.key, Some(old.hash))?.1
                        {
                            let Value::Dict(entries) = self.heap.get_mut(table) else {
                                unreachable!()
                            };
                            let (key, value) = entries.remove(i);
                            self.heap.weak_edge_removed(key);
                            self.heap.weak_edge_removed(value);
                        }
                    }
                    let resident = self.set_cache_key(table, key, result, hash)?;
                    {
                        let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                            unreachable!()
                        };
                        if let Some(i) = entries.iter().position(|entry| entry.key == resident) {
                            entries.remove(i);
                        }
                    }
                    loop {
                        let old = {
                            let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                                unreachable!()
                            };
                            if entries.len() >= 128 {
                                Some(entries.remove(0))
                            } else {
                                None
                            }
                        };
                        let Some(old) = old else { break };
                        self.heap.keep(old.key);
                        if let Some(i) = self.find_dictionary_key(table, old.key, Some(old.hash))?.1
                        {
                            let Value::Dict(entries) = self.heap.get_mut(table) else {
                                unreachable!()
                            };
                            let (key, value) = entries.remove(i);
                            self.heap.weak_edge_removed(key);
                            self.heap.weak_edge_removed(value);
                        }
                    }
                    let Value::Cached { entries, .. } = self.heap.get_mut(callable) else {
                        unreachable!()
                    };
                    entries.push(value::CachedCall {
                        key: resident,
                        hash,
                    });
                    Ok(result)
                })();
                self.heap.release_temporaries_since(mark);
                result
            }
            Value::Class {
                name,
                scope,
                ancestors,
                ..
            } => {
                let (mut args, mut kwargs) = (args, kwargs);
                if let Some(&decimal) = self.classes.get("decimal.Decimal")
                    && (callable == decimal
                        || (ancestors.contains(&decimal)
                            && self.class_attr(callable, "__new__").is_none()
                            && self.class_attr(callable, "__init__")
                                == self.class_attr(decimal, "__init__")))
                {
                    let value = match (&mut args[..], &mut kwargs[..]) {
                        ([value], []) => Some(value),
                        ([], [(key, value)]) if key == "value" => Some(value),
                        _ => None,
                    };
                    if let Some(value) = value {
                        if let Value::Instance { class, .. } = self.heap.get(*value)
                            && self.direct_subclass(*class, decimal)
                        {
                            if *class == decimal && callable == decimal {
                                return Ok(*value);
                            }
                            if let Some(text) = self.raw_instance_field(*value, "_text") {
                                *value = text;
                            }
                        }
                        if let Some(text) = self.string_storage(*value) {
                            *value = text;
                        } else if matches!(
                            self.heap.get(self.scalar_value(*value)),
                            Value::Int(_) | Value::Bool(_)
                        ) {
                            let text =
                                crate::integer_text::try_decimal_coefficient(&self.as_int(*value)?)
                                    .ok_or_else(crate::Error::memory_error)?;
                            *value = self.text(text.into_string());
                        }
                    }
                }
                if let Some(values) = self.enum_values(callable) {
                    return self.enum_lookup(callable, values, &args, &kwargs);
                }
                if self.classes.get("typing.Union") == Some(&callable) {
                    return Err(Error::new(
                        "TypeError",
                        "cannot create 'typing.Union' instances",
                    ));
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
                    let mut fields: indexmap::IndexMap<_, _> =
                        names.iter().cloned().zip(args).collect();
                    for (k, v) in kwargs {
                        if !names.contains(&k) || fields.insert(k.clone(), v).is_some() {
                            return Err(Error::new(
                                "TypeError",
                                format!("invalid argument {k:?} to {name}"),
                            ));
                        }
                    }
                    if fields.len() != names.len()
                        && let Some(defaults) = self.find_in_mro(callable, |base| {
                            let Value::Class { scope, .. } = *self.heap.get(base) else {
                                return None;
                            };
                            self.scope_data(scope)
                                .class_namespace
                                .as_ref()?
                                .namedtuple_defaults
                        })
                        && let Value::Tuple(defaults) = self.heap.get(defaults)
                    {
                        for (name, value) in names.iter().rev().zip(defaults.iter().rev()) {
                            fields.entry(name.clone()).or_insert(*value);
                        }
                    }
                    if fields.len() != names.len() {
                        return Err(Error::new("TypeError", format!("missing field for {name}")));
                    }
                    return Ok(self.alloc(Value::Instance {
                        namespace: None,
                        class: callable,
                        fields,
                        dictionary: None,
                        scalar: None,
                        property: None,
                        weak_reference: None,
                        memory_stream: None,
                    }));
                }
                let new = self.class_attr(callable, "__new__").or_else(|| {
                    self.has_base(callable, "int")
                        .then(|| self.builtin("int.__new__"))
                });
                let string_base = ["str", "bytes"]
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
                let float_subclass = self.has_base(callable, "float");
                let tuple_subclass = self.has_base(callable, "tuple");
                let collection_kind = ["set", "frozenset", "bytearray"]
                    .into_iter()
                    .find(|kind| self.has_base(callable, kind));
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
                } else if self.has_base(callable, "module") {
                    self.allocate_module(callable)
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
                    let storage = if self.has_base(callable, "list") {
                        Some(self.alloc(Value::List(Vec::new())))
                    } else if tuple_subclass {
                        Some(builtins::call(self, "tuple", &args, &kwargs)?)
                    } else if let Some(kind) = collection_kind {
                        let custom_init = self.class_attr(callable, "__init__").is_some();
                        let items = if kind != "frozenset" && custom_init {
                            &[][..]
                        } else {
                            &args
                        };
                        let keywords = if custom_init { &[][..] } else { &kwargs };
                        Some(builtins::call(self, kind, items, keywords)?)
                    } else {
                        None
                    };
                    let object = self.instance(callable, dictionary);
                    if let Value::Instance { scalar, .. } = self.heap.get_mut(object) {
                        *scalar = storage;
                    }
                    object
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
                    && !tuple_subclass
                    && collection_kind.is_none()
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
                    return self.exec_source(code, globals, None, Some(callable));
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
                    let extra = if pos.is_empty() {
                        args
                    } else {
                        copy_ids(args.get(pos.len()..).unwrap_or_default())?
                    };
                    let extra = self.alloc(Value::Tuple(extra));
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
                let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                    self.execute(code, local, FrameOwner::Function(callable))
                });
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
        #[derive(serde::Deserialize)]
        struct ClassBindings {
            #[serde(default)]
            scope_nonlocals: std::collections::HashSet<String>,
        }
        let parameters = self.code_object(code).parameters;
        let bindings: ClassBindings = serde_json::from_str(self.str_value(parameters)?)
            .map_err(|e| Error::new("SystemError", e.to_string()))?;
        let module = self.scope_data(parent).module.clone();
        let scope = self.scope(&module, Some(parent));
        let enum_namespace = self.enum_namespace(&bases);
        let enum_kind = enum_namespace.as_ref().map(|state| state.kind);
        let cell = self.alloc(Value::ClassCell(None));
        let Value::Scope(body_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        body_scope.class_body = Some(Vec::new());
        body_scope.closure.nonlocals = bindings.scope_nonlocals;
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
        self.bind_local(scope, "__annotations__", annotations);
        self.execute(code, scope, FrameOwner::Function(body))?;
        if let Value::Scope(frame) = self.heap.get_mut(scope) {
            frame.globals.clear();
            frame.closure.nonlocals.clear();
        }
        if self.scope_name(scope, "__qualname__").is_none() {
            let qualname = self.text(&name);
            self.bind_local(scope, "__qualname__", qualname);
        }
        if self.scope_name(scope, "__module__").is_none() {
            let namespace = self.module_globals(parent);
            if let Some(name) = self.dict_named_item(namespace, "__name__") {
                self.bind_local(scope, "__module__", name);
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
        self.install_weakref_descriptor(class, scope);
        *self.heap.get_mut(cell) = Value::ClassCell(Some(class));
        let Value::Scope(class_scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        // The class body can read the enclosing module's docstring before the
        // type supplies a default for an undocumented class.
        let has_doc_slot = class_scope
            .class_namespace
            .as_deref()
            .and_then(|namespace| namespace.slot_names.as_deref())
            .is_some_and(|slots| slots.iter().any(|slot| slot == "__doc__"));
        if !has_doc_slot {
            class_scope
                .names
                .entry("__doc__".to_owned())
                .or_insert(NONE);
        }
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
