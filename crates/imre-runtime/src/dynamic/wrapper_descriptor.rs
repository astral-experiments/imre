use super::*;

/// A builtin descriptor and the receiver type accepted by its binding operation.
#[derive(Clone, Copy)]
pub(super) struct BuiltinDescriptor {
    owner: &'static str,
    name: &'static str,
    pub(super) method: bool,
}

impl BuiltinDescriptor {
    pub(super) fn lookup(symbol: &str) -> Option<Self> {
        if let Some((owner, attr)) = symbol.rsplit_once('.')
            && let Some(owner) = stdlib::itertools::cursor_type(owner)
            && let Some(name) = stdlib::itertools::slots(owner).find(|&slot| slot == attr)
        {
            return Some(Self {
                owner,
                name,
                method: name == "__length_hint__",
            });
        }
        if let Some((class, attr)) = symbol.rsplit_once('.')
            && let Some(owner) = crate::objects::HashState::provider_class(class)
            && let Some(&name) = ["copy", "update", "digest", "hexdigest"]
                .iter()
                .find(|&&name| name == attr)
        {
            return Some(Self {
                owner,
                name,
                method: true,
            });
        }
        if let Some((deque::CLASS, name)) = symbol.rsplit_once('.')
            && let Some(member) = deque::member(name)
        {
            return Some(Self {
                owner: deque::CLASS,
                name: member.name,
                method: member.is_method(),
            });
        }
        if let Some((owner, attr)) = symbol.rsplit_once('.')
            && let Some(class) = typing::ParameterClass::lookup(owner)
            && let Some(&name) = class.methods.iter().find(|&&name| name == attr)
        {
            return Some(Self {
                owner: class.owner,
                name,
                method: !matches!(name, "__eq__" | "__ne__" | "__iter__"),
            });
        }
        if symbol == "typing.Union.__mro_entries__" {
            return Some(Self {
                owner: "typing.Union",
                name: "__mro_entries__",
                method: true,
            });
        }
        let slots = match symbol.rsplit_once('.') {
            Some((weakref::CALLABLE_PROXY_TYPE, attr)) => Some((
                weakref::CALLABLE_PROXY_TYPE,
                attr,
                weakref::CALLABLE_PROXY_METHODS,
            )),
            Some((weakref::PROXY_TYPE, attr)) => {
                Some((weakref::PROXY_TYPE, attr, weakref::PROXY_METHODS))
            }
            Some((stdlib::os::scandir::ENTRY, attr)) => Some((
                stdlib::os::scandir::ENTRY,
                attr,
                stdlib::os::scandir::ENTRY_METHODS,
            )),
            Some((stdlib::os::scandir::ITER, attr)) => Some((
                stdlib::os::scandir::ITER,
                attr,
                stdlib::os::scandir::ITER_METHODS,
            )),
            Some(("set", attr)) if builtins::set_operators("set").contains(&attr) => {
                Some(("set", attr, builtins::set_operators("set")))
            }
            Some(("frozenset", attr)) if builtins::set_operators("frozenset").contains(&attr) => {
                Some(("frozenset", attr, builtins::set_operators("frozenset")))
            }
            Some((stdlib::functools::LRU_CACHE_CLASS, attr)) => Some((
                stdlib::functools::LRU_CACHE_CLASS,
                attr,
                stdlib::functools::COPY_METHODS,
            )),
            Some(("functools.KeyWrapper", attr)) => Some((
                "functools.KeyWrapper",
                attr,
                &[
                    "__lt__", "__le__", "__eq__", "__ne__", "__gt__", "__ge__", "__call__",
                ][..],
            )),
            Some((owner, attr)) if let Some(members) = stdlib::datetime::members(owner) => {
                Some((members.owner, attr, members.methods))
            }
            Some((owner, attr)) if let Some(class) = deque::iterator_class(owner) => {
                Some((class, attr, deque::ITERATOR_METHODS))
            }
            Some((owner, attr)) if let Some(class) = DictIterator::class(owner) => {
                Some((class, attr, DictIterator::methods(class)))
            }
            Some((value::REGEX_PATTERN_TYPE, attr)) => {
                Some((value::REGEX_PATTERN_TYPE, attr, value::REGEX_COPY_METHODS))
            }
            Some((value::REGEX_MATCH_TYPE, attr)) => {
                Some((value::REGEX_MATCH_TYPE, attr, value::REGEX_COPY_METHODS))
            }
            Some((defaultdict::CLASS, attr)) => {
                Some((defaultdict::CLASS, attr, defaultdict::METHODS))
            }
            Some(("generator", attr)) => Some(("generator", attr, &["__iter__", "__next__"][..])),
            Some(("async_generator", attr)) => Some((
                "async_generator",
                attr,
                &["__aiter__", "__anext__", "asend", "athrow", "aclose"][..],
            )),
            Some(("async_generator_asend", attr)) => Some((
                "async_generator_asend",
                attr,
                generator_protocol::ANEXT_METHODS,
            )),
            Some(("async_generator_athrow", attr)) => Some((
                "async_generator_athrow",
                attr,
                generator_protocol::ANEXT_METHODS,
            )),
            Some(("anext_awaitable", attr)) => {
                Some(("anext_awaitable", attr, generator_protocol::ANEXT_METHODS))
            }
            Some(("mappingproxy", attr)) if attr != "__class_getitem__" => {
                let members = if builtins::MAPPING_PROXY_METHODS.contains(&attr) {
                    builtins::MAPPING_PROXY_METHODS
                } else {
                    builtins::MAPPING_PROXY_SLOTS
                };
                Some(("mappingproxy", attr, members))
            }
            Some(("type", attr)) => Some(("type", attr, &["mro"][..])),
            Some(("int", attr)) => Some(("int", attr, builtins::real_methods("int"))),
            Some(("float", "as_integer_ratio")) => {
                Some(("float", "as_integer_ratio", &["as_integer_ratio"][..]))
            }
            Some(("float", attr)) => Some(("float", attr, builtins::real_methods("float"))),
            Some(("bytes", attr)) => Some(("bytes", attr, builtins::BYTES_METHODS)),
            Some(("complex", attr)) => Some(("complex", attr, super::complex::METHODS)),
            Some(("bytearray", attr)) => Some(("bytearray", attr, builtins::BYTEARRAY_METHODS)),
            Some(("memoryview", attr)) => Some(("memoryview", attr, builtins::MEMORYVIEW_METHODS)),
            Some(("BaseException", attr)) => {
                Some(("BaseException", attr, &["add_note", "with_traceback"][..]))
            }
            Some(("member_descriptor", attr)) => {
                Some(("member_descriptor", attr, super::namedtuple::MEMBER_SLOTS))
            }
            Some(("frame", attr)) => Some(("frame", attr, &["clear"][..])),
            Some(("set", attr)) if attr != "__class_getitem__" => {
                Some(("set", attr, builtins::set_methods("set")))
            }
            Some(("frozenset", attr)) if attr != "__class_getitem__" => {
                Some(("frozenset", attr, builtins::set_methods("frozenset")))
            }
            Some(("list", attr)) => Some(("list", attr, builtins::LIST_METHODS)),
            Some(("range", attr)) => Some(("range", attr, range::METHODS)),
            Some((stdlib::fixed_structseq::SCHED_PARAM, attr)) => Some((
                stdlib::fixed_structseq::SCHED_PARAM,
                attr,
                &["__reduce__", "__replace__"][..],
            )),
            Some((class, attr)) if let Some(layout) = stdlib::os::stat::layout(class) => {
                Some((layout.class, attr, layout.methods()))
            }
            Some((namespace::CLASS, attr)) => {
                Some((namespace::CLASS, attr, &["__reduce__", "__replace__"][..]))
            }
            Some(("types.GenericAlias", attr)) => {
                Some(("types.GenericAlias", attr, &["__reduce__"][..]))
            }
            Some(("generic_alias_iterator", attr)) => {
                Some(("generic_alias_iterator", attr, &["__reduce__"][..]))
            }
            Some(("typing.TypeAliasType", attr)) => {
                Some(("typing.TypeAliasType", attr, &["__reduce__"][..]))
            }
            Some(("slice", attr)) => Some(("slice", attr, slice::METHODS)),
            _ => None,
        };
        if let Some((owner, attr, methods)) = slots
            && let Some(&name) = methods.iter().find(|&&method| method == attr)
        {
            return Some(Self {
                owner,
                name,
                method: !name.starts_with("__")
                    || (deque::iterator_class(owner).is_some() && name == "__length_hint__")
                    || (owner == "list" && name == "__getitem__")
                    || (matches!(owner, "set" | "frozenset") && name == "__contains__")
                    || (owner == defaultdict::CLASS && name == "__missing__")
                    || matches!(
                        name,
                        "__copy__"
                            | "__deepcopy__"
                            | "__reversed__"
                            | "__round__"
                            | "__trunc__"
                            | "__getinitargs__"
                            | "__reduce__"
                            | "__reduce_ex__"
                            | "__replace__"
                            | "__bytes__"
                            | "__complex__"
                            | "__fspath__"
                            | "__format__"
                            | "__enter__"
                            | "__exit__"
                    ),
            });
        }
        let (owner, name) = match symbol {
            "type.__init__" => ("type", "__init__"),
            "super.__getattribute__" => ("super", "__getattribute__"),
            "super.__init__" => ("super", "__init__"),
            "super.__get__" => ("super", "__get__"),
            "super.__repr__" => ("super", "__repr__"),
            "object.__format__" => ("object", "__format__"),
            "object.__sizeof__" => ("object", "__sizeof__"),
            "dict.__sizeof__" => ("dict", "__sizeof__"),
            "dict.fromkeys" => ("dict", "fromkeys"),
            "int.__sizeof__" => ("int", "__sizeof__"),
            "str.__sizeof__" => ("str", "__sizeof__"),
            "bytearray.__sizeof__" => ("bytearray", "__sizeof__"),
            "list.__sizeof__" => ("list", "__sizeof__"),
            "set.__sizeof__" => ("set", "__sizeof__"),
            "frozenset.__sizeof__" => ("frozenset", "__sizeof__"),
            "object.__getstate__" => ("object", "__getstate__"),
            "object.__reduce__" => ("object", "__reduce__"),
            "method.__reduce__" => ("method", "__reduce__"),
            "builtin_function_or_method.__reduce__" => ("builtin_function_or_method", "__reduce__"),
            "object.__reduce_ex__" => ("object", "__reduce_ex__"),
            "int.__add__" => ("int", "__add__"),
            "int.__index__" => ("int", "__index__"),
            "int.from_bytes" => ("int", "from_bytes"),
            "bool.__repr__" => ("bool", "__repr__"),
            "int.__repr__" => ("int", "__repr__"),
            "float.__repr__" => ("float", "__repr__"),
            "int.__format__" => ("int", "__format__"),
            "str.__eq__" => ("str", "__eq__"),
            "str.__format__" => ("str", "__format__"),
            "str.__mod__" => ("str", "__mod__"),
            "str.__rmod__" => ("str", "__rmod__"),
            "str.join" => ("str", "join"),
            "str.isascii" => ("str", "isascii"),
            "str.isalpha" => ("str", "isalpha"),
            "str.isdigit" => ("str", "isdigit"),
            "str.isidentifier" => ("str", "isidentifier"),
            "str.lower" => ("str", "lower"),
            "str.title" => ("str", "title"),
            "str.swapcase" => ("str", "swapcase"),
            "str.islower" => ("str", "islower"),
            "str.center" => ("str", "center"),
            "str.ljust" => ("str", "ljust"),
            "str.rjust" => ("str", "rjust"),
            "str.zfill" => ("str", "zfill"),
            "str.istitle" => ("str", "istitle"),
            "float.__format__" => ("float", "__format__"),
            "collections._tuplegetter.__repr__" => (super::namedtuple::GETTER, "__repr__"),
            "object.__init__" => ("object", "__init__"),
            "object.__lt__" => ("object", "__lt__"),
            "object.__le__" => ("object", "__le__"),
            "object.__eq__" => ("object", "__eq__"),
            "object.__gt__" => ("object", "__gt__"),
            "object.__ge__" => ("object", "__ge__"),
            "object.__getattribute__" => ("object", "__getattribute__"),
            "object.__setattr__" => ("object", "__setattr__"),
            "object.__delattr__" => ("object", "__delattr__"),
            "object.__dir__" => ("object", "__dir__"),
            "object.__subclasshook__" => ("object", "__subclasshook__"),
            "object.__init_subclass__" => ("object", "__init_subclass__"),
            "typing.Generic.__init_subclass__" => ("typing.Generic", "__init_subclass__"),
            "classmethod_descriptor.__get__" => ("classmethod_descriptor", "__get__"),
            "object.__hash__" => ("object", "__hash__"),
            "object.__ne__" => ("object", "__ne__"),
            "object.__repr__" => ("object", "__repr__"),
            "object.__str__" => ("object", "__str__"),
            "function.__call__" => ("function", "__call__"),
            "classmethod.__get__" => ("classmethod", "__get__"),
            "functools.partial.__call__" => ("functools.partial", "__call__"),
            "functools.partial.__get__" => ("functools.partial", "__get__"),
            "method_descriptor.__get__" => ("method_descriptor", "__get__"),
            "wrapper_descriptor.__get__" => ("wrapper_descriptor", "__get__"),
            "str.__str__" => ("str", "__str__"),
            "str.__repr__" => ("str", "__repr__"),
            "bytes.__repr__" => ("bytes", "__repr__"),
            "dict.__or__" => ("dict", "__or__"),
            "dict.__ror__" => ("dict", "__ror__"),
            "dict.__ior__" => ("dict", "__ior__"),
            "dict.__eq__" => ("dict", "__eq__"),
            "dict.__init__" => ("dict", "__init__"),
            "dict.__repr__" => ("dict", "__repr__"),
            "dict.__reversed__" => ("dict", "__reversed__"),
            "tuple.__repr__" => ("tuple", "__repr__"),
            "types.SimpleNamespace.__init__" => (super::namespace::CLASS, "__init__"),
            "types.SimpleNamespace.__repr__" => (super::namespace::CLASS, "__repr__"),
            "types.SimpleNamespace.__eq__" => (super::namespace::CLASS, "__eq__"),
            "types.SimpleNamespace.__ne__" => (super::namespace::CLASS, "__ne__"),
            "tuple.__getitem__" => ("tuple", "__getitem__"),
            "tuple.__len__" => ("tuple", "__len__"),
            "array.array.__getitem__" => ("array.array", "__getitem__"),
            "array.array.__setitem__" => ("array.array", "__setitem__"),
            "tuple.count" => ("tuple", "count"),
            "tuple.index" => ("tuple", "index"),
            "_thread._local.__getattribute__" => (thread_local::CLASS, "__getattribute__"),
            "_thread._local.__setattr__" => (thread_local::CLASS, "__setattr__"),
            "_thread._local.__delattr__" => (thread_local::CLASS, "__delattr__"),
            _ => return None,
        };
        Some(Self {
            owner,
            name,
            method: matches!(
                name,
                "__format__"
                    | "__dir__"
                    | "__subclasshook__"
                    | "__init_subclass__"
                    | "__sizeof__"
                    | "__getstate__"
                    | "__reduce__"
                    | "__reduce_ex__"
                    | "__reversed__"
                    | "count"
                    | "index"
                    | "join"
                    | "from_bytes"
                    | "fromkeys"
                    | "isascii"
                    | "isalpha"
                    | "isdigit"
                    | "isidentifier"
                    | "islower"
                    | "istitle"
                    | "center"
                    | "ljust"
                    | "rjust"
                    | "zfill"
                    | "lower"
                    | "title"
                    | "swapcase"
            ),
        })
    }

    /// Report the descriptor type exposed by type() and explicit __get__.
    pub(super) fn class(self) -> &'static str {
        if matches!(
            (self.owner, self.name),
            ("object", "__subclasshook__" | "__init_subclass__")
                | ("int", "from_bytes")
                | ("dict", "fromkeys")
                | ("typing.Generic", "__init_subclass__")
        ) {
            "classmethod_descriptor"
        } else if self.method {
            "method_descriptor"
        } else {
            "wrapper_descriptor"
        }
    }

    pub(super) fn repr(self) -> String {
        let kind = if self.method {
            "method"
        } else {
            "slot wrapper"
        };
        format!("<{kind} '{}' of '{}' objects>", self.name, self.owner)
    }

    pub(super) fn bound_repr(self, ctx: &Context, receiver: Id) -> String {
        let kind = ctx
            .builtin_type_name(receiver)
            .unwrap_or_else(|| ctx.type_name(receiver));
        format!(
            "<method-wrapper '{}' of {kind} object at {receiver:#x}>",
            self.name
        )
    }

    pub(super) fn bind(self, ctx: &mut Context, descriptor: Id, receiver: Id) -> Result<Id> {
        if receiver == NONE {
            return Ok(descriptor);
        }
        let accepts = match self.owner {
            class if crate::objects::HashState::is_provider_class(class) => {
                ctx.type_name(receiver) == class
            }
            owner if stdlib::itertools::cursor_type(owner).is_some() => {
                ctx.instance_of(receiver, ctx.builtins[owner])
            }
            owner if stdlib::datetime::members(owner).is_some() => {
                ctx.instance_of(receiver, ctx.builtins[owner])
            }
            owner if DictIterator::class(owner).is_some() => {
                matches!(ctx.heap.get(receiver), Value::DictIterator(iter) if iter.class_name() == owner)
            }
            "generator" => matches!(ctx.heap.get(receiver), Value::Generator { .. }),
            "async_generator" | "async_generator_asend" | "async_generator_athrow" => {
                ctx.heap.get(receiver).intrinsic_type_name() == Some(self.owner)
            }
            "anext_awaitable" => matches!(ctx.heap.get(receiver), Value::ANextAwaitable { .. }),
            owner if typing::ParameterClass::lookup(owner).is_some() => {
                ctx.builtin_type_name(receiver) == Some(owner)
            }
            "typing.TypeAliasType" => matches!(ctx.heap.get(receiver), Value::TypeAlias { .. }),
            "typing.Union" => matches!(ctx.heap.get(receiver), Value::Union(_)),
            super::namedtuple::GETTER => {
                matches!(ctx.heap.get(receiver), Value::TupleGetter { .. })
            }
            "object" => true,
            "type" => ctx.is_type_object(receiver),
            "function" => matches!(ctx.heap.get(receiver), Value::Function { .. }),
            "classmethod" => matches!(ctx.heap.get(receiver), Value::ClassMethod { .. }),
            stdlib::functools::PARTIAL_CLASS => {
                matches!(
                    ctx.heap.get(ctx.scalar_value(receiver)),
                    Value::Partial { .. }
                )
            }
            "functools.KeyWrapper" => matches!(ctx.heap.get(receiver), Value::KeyWrapper { .. }),
            super::namespace::CLASS => namespace::instance_class(ctx, receiver).is_some(),
            "frame" => matches!(ctx.heap.get(receiver), Value::PythonFrame { .. }),
            "BaseException" => matches!(ctx.heap.get(receiver), Value::Raised(_)),
            class @ (stdlib::fixed_structseq::SCHED_PARAM
            | stdlib::os::stat::CLASS
            | stdlib::os::stat::VFS_CLASS) => matches!(
                ctx.heap.get(receiver),
                Value::NamedTuple {
                    class: Some(owner),
                    ..
                } if *owner == class
            ),
            stdlib::os::scandir::ENTRY => matches!(ctx.heap.get(receiver), Value::DirEntry(_)),
            stdlib::os::scandir::ITER => matches!(ctx.heap.get(receiver), Value::Scandir(_)),
            "range" => matches!(ctx.heap.get(receiver), Value::Range(_)),
            "memoryview" => matches!(ctx.heap.get(receiver), Value::MemoryView { .. }),
            "array.array" => matches!(ctx.heap.get(receiver), Value::Array(_)),
            "slice" => matches!(ctx.heap.get(receiver), Value::Slice(_)),
            "member_descriptor" => ctx.exact_type(receiver)? == ctx.builtin("member_descriptor"),
            "method_descriptor" | "wrapper_descriptor" | "classmethod_descriptor" => {
                ctx.builtin_type_name(receiver) == Some(self.owner)
            }
            "dict" => ctx.dict_storage(receiver).is_some(),
            "mappingproxy" => matches!(ctx.heap.get(receiver), Value::MappingProxy(_)),
            deque::ITERATOR | deque::REVERSE_ITERATOR => {
                ctx.heap.get(receiver).intrinsic_type_name() == Some(self.owner)
            }
            deque::CLASS => matches!(ctx.heap.get(ctx.scalar_value(receiver)), Value::Deque(_)),
            "collections.defaultdict" => ctx.defaultdict_factory(receiver).is_some(),
            _ => ctx.builtin_slot_accepts(receiver, self.owner),
        };
        if !accepts {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                    self.name,
                    self.owner,
                    ctx.builtin_type_name(receiver)
                        .unwrap_or_else(|| ctx.type_name(receiver))
                ),
            ));
        }
        Ok(ctx.alloc(Value::Method {
            callable: descriptor,
            receiver,
            explicit: false,
        }))
    }

    pub(super) fn attribute(
        self,
        ctx: &mut Context,
        name: &str,
        receiver: Option<Id>,
    ) -> Option<Id> {
        Some(match name {
            "__text_signature__" if self.owner == stdlib::functools::LRU_CACHE_CLASS && self.name == "__reduce__" => ctx.text("($self, /)"),
            "__doc__" | "__text_signature__" if self.owner == stdlib::functools::LRU_CACHE_CLASS => NONE,
            "__doc__" if self.owner == weakref::PROXY_TYPE && self.name != "__repr__" => NONE,
            "__text_signature__" if self.owner == weakref::PROXY_TYPE => ctx.text("($self, /)"),
            "__doc__" | "__text_signature__" if self.owner == deque::CLASS => {
                let member = deque::member(self.name)?;
                ctx.text(if name == "__doc__" { member.doc } else { member.signature })
            }
            "__doc__" | "__text_signature__"
                if deque::iterator_class(self.owner).is_some()
                    && matches!(self.name, "__length_hint__" | "__reduce__") =>
            {
                ctx.text(match (self.name, name) {
                    (_, "__text_signature__") => "($self, /)",
                    ("__length_hint__", _) => "Private method returning an estimate of len(list(it)).",
                    _ => "Return state information for pickling.",
                })
            }
            "__doc__" if self.owner == namespace::CLASS && self.name == "__reduce__" => {
                ctx.text("Return state information for pickling")
            }
            "__text_signature__" if self.owner == namespace::CLASS && self.name == "__reduce__" => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.owner == namespace::CLASS && self.name == "__replace__" => ctx.text(
                "Return a copy of the namespace object with new values for the specified attributes.",
            ),
            "__text_signature__" if self.owner == namespace::CLASS && self.name == "__replace__" => {
                ctx.text("($self, /, **changes)")
            }
            "__doc__" | "__text_signature__" if self.owner == defaultdict::CLASS => {
                let (doc, signature) = defaultdict::metadata(self.name)?;
                ctx.text(if name == "__doc__" { doc } else { signature })
            }
            "__doc__" | "__text_signature__"
                if matches!(self.owner, "set" | "frozenset")
                    && let Some(op) = builtins::set_operator(self.name) =>
            {
                if name == "__text_signature__" {
                    ctx.text("($self, value, /)")
                } else if self.name.starts_with("__r") {
                    ctx.text(format!("Return value{op}self."))
                } else if self.name.starts_with("__i") {
                    ctx.text(format!("Return self{op}=value."))
                } else {
                    ctx.text(format!("Return self{op}value."))
                }
            }
            "__doc__" if self.owner == "typing.Generic" => ctx.text("Function to initialize subclasses."),
            "__text_signature__" if self.owner == "typing.Generic" => NONE,
            "__name__" => ctx.text(self.name),
            "__qualname__" => {
                let owner = if self.method
                    && let Some(receiver) = receiver
                {
                    match ctx.heap.get(receiver) {
                        _ if self.owner == "type" || self.class() == "classmethod_descriptor" => ctx.class_qualname(receiver),
                        _ if matches!(self.owner, value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE) => self.owner.rsplit('.').next()?,
                        _ if matches!(self.owner, "builtin_function_or_method" | "method") => self.owner,
                        Value::Deque(_) if self.owner == deque::CLASS => "deque",
                        Value::DefaultDict { .. } if self.owner == defaultdict::CLASS => "defaultdict",
                        Value::NamedTuple { class: Some(owner), .. } if let Some(layout) = stdlib::os::stat::layout(owner) => layout.name(),
                        value if let Some(class) = value.instance_class() => ctx
                            .class_attr(class, "__qualname__")
                            .and_then(|name| ctx.str_value(name).ok())
                            .unwrap_or_else(|| ctx.type_name(receiver)),
                        _ => ctx.type_name(receiver),
                    }
                } else {
                    self.owner.rsplit('.').next()?
                };
                ctx.text(format!("{owner}.{}", self.name))
            }
            "__objclass__" if receiver.is_none() && self.owner == "typing.Generic" => {
                typing::named_class(ctx, "typing", "Generic")
            }
            "__objclass__" if receiver.is_none() || !self.method => ctx.builtin(self.owner),
            "__module__" if receiver.is_some() && self.method => NONE,
            "__text_signature__" if matches!(self.owner, value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE) => {
                ctx.text(if self.name == "__copy__" { "($self, /)" } else { "($self, memo, /)" })
            }
            "__doc__" | "__text_signature__"
                if self.name == "__len__" && matches!(self.owner, "list" | "tuple" | "mappingproxy") =>
            {
                ctx.text(if name == "__doc__" {
                    "Return len(self)."
                } else {
                    "($self, /)"
                })
            }
            "__doc__" | "__text_signature__" if matches!(self.owner, "mappingproxy" | "str")
                && let Some(op) = super::operators::comparison_op(self.name) =>
            {
                if name == "__doc__" {
                    ctx.text(format!("Return self{op}value."))
                } else {
                    ctx.text("($self, value, /)")
                }
            }
            "__doc__" if self.name == "__get__" => {
                ctx.text("Return an attribute of instance, which is of type owner.")
            }
            "__text_signature__" if self.name == "__get__" => {
                ctx.text("($self, instance, owner=None, /)")
            }
            "__doc__" if self.name == "__lt__" => ctx.text("Return self<value."),
            "__text_signature__" if self.name == "__lt__" => ctx.text("($self, value, /)"),
            "__doc__" if matches!(self.owner, "object" | "dict") && self.name == "__init__" => {
                ctx.text("Initialize self.  See help(type(self)) for accurate signature.")
            }
            "__text_signature__" if matches!(self.owner, "object" | "dict") && self.name == "__init__" => {
                ctx.text("($self, /, *args, **kwargs)")
            }
            "__doc__" if self.name == "__getattribute__" => ctx.text("Return getattr(self, name)."),
            "__text_signature__" if self.name == "__getattribute__" => ctx.text("($self, name, /)"),
            "__doc__" if self.owner == "itertools.repeat" && self.name == "__length_hint__" => {
                ctx.text("Private method returning an estimate of len(list(it)).")
            }
            "__text_signature__" if self.owner == "itertools.repeat" && self.name == "__length_hint__" => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.owner == "dict" && self.name == "__reversed__" => {
                ctx.text("Return a reverse iterator over the dict keys.")
            }
            "__text_signature__" if self.owner == "dict" && self.name == "__reversed__" => ctx.text("($self, /)"),
            "__doc__" if self.owner == "mappingproxy" && self.method => ctx.text(match self.name {
                "__reversed__" => "D.__reversed__() -> reverse iterator",
                "keys" => "D.keys() -> a set-like object providing a view on D's keys",
                "values" => "D.values() -> an object providing a view on D's values",
                "items" => "D.items() -> a set-like object providing a view on D's items",
                "copy" => "D.copy() -> a shallow copy of D",
                "get" => "Return the value for key if key is in the mapping, else default.",
                _ => return None,
            }),
            "__text_signature__" if self.owner == "mappingproxy" && self.method => {
                ctx.text(if self.name == "get" { "($self, key, default=None, /)" } else { "($self, /)" })
            }
            "__doc__" if self.name == "add_note" => ctx.text("Add a note to the exception"),
            "__text_signature__" if self.name == "add_note" => ctx.text("($self, note, /)"),
            "__doc__" if self.name == "__call__" => ctx.text("Call self as a function."),
            "__text_signature__" if self.name == "__call__" => {
                ctx.text("($self, /, *args, **kwargs)")
            }
            "__doc__" if self.name == "__getitem__" && self.owner == "mappingproxy" => {
                ctx.text("Return self[key].")
            }
            "__text_signature__" if self.name == "__getitem__" && self.owner == "mappingproxy" => {
                ctx.text("($self, key, /)")
            }
            "__doc__" if self.name == "__hash__" => ctx.text("Return hash(self)."),
            "__doc__" if self.name == "__repr__" => ctx.text("Return repr(self)."),
            "__doc__" if self.name == "__iter__" => ctx.text("Implement iter(self)."),
            "__doc__" if self.name == "__next__" => ctx.text("Implement next(self)."),
            "__text_signature__" if matches!(self.name, "__hash__" | "__iter__" | "__next__" | "__repr__") => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.name == "__contains__" => ctx.text("Return bool(key in self)."),
            "__text_signature__" if self.name == "__contains__" => ctx.text("($self, key, /)"),
            "__doc__" if self.name == "__neg__" => ctx.text("-self"),
            "__doc__" if self.name == "__pos__" => ctx.text("+self"),
            "__doc__" if self.name == "__bool__" => ctx.text("True if self else False"),
            "__doc__" if self.name == "__int__" => ctx.text("int(self)"),
            "__doc__" if self.name == "__float__" => ctx.text("float(self)"),
            "__doc__" if self.name == "__abs__" => ctx.text("abs(self)"),
            "__text_signature__" if matches!(self.name, "__neg__" | "__pos__" | "__abs__" | "__bool__" | "__int__" | "__float__") => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.name == "__index__" => ctx.text(
                "Return self converted to an integer, if self is suitable for use as an index into a list.",
            ),
            "__text_signature__" if self.name == "__index__" => ctx.text("($self, /)"),
            "__doc__" if self.name == "__pow__" => ctx.text("Return pow(self, value, mod)."),
            "__text_signature__" if self.name == "__pow__" => ctx.text("($self, value, mod=None, /)"),
            "__doc__" if self.name == "__add__" => ctx.text("Return self+value."),
            "__doc__" if self.name == "__radd__" => ctx.text("Return value+self."),
            "__doc__" if self.name == "__sub__" => ctx.text("Return self-value."),
            "__doc__" if self.name == "__rsub__" => ctx.text("Return value-self."),
            "__doc__" if self.name == "__mul__" => ctx.text("Return self*value."),
            "__doc__" if self.name == "__rmul__" => ctx.text("Return value*self."),
            "__doc__" if self.name == "__truediv__" => ctx.text("Return self/value."),
            "__doc__" if self.name == "__rtruediv__" => ctx.text("Return value/self."),
            "__doc__" if self.name == "__mod__" => ctx.text("Return self%value."),
            "__doc__" if self.name == "__rmod__" => ctx.text("Return value%self."),
            "__doc__" if self.name == "__floordiv__" => ctx.text("Return self//value."),
            "__doc__" if self.name == "__rfloordiv__" => ctx.text("Return value//self."),
            "__doc__" if self.name == "__ior__" => ctx.text("Return self|=value."),
            "__doc__" if matches!(self.owner, "dict" | "mappingproxy") && self.name == "__or__" => ctx.text("Return self|value."),
            "__doc__" if matches!(self.owner, "dict" | "mappingproxy") && self.name == "__ror__" => ctx.text("Return value|self."),
            "__text_signature__" if self.name == "__ior__" || (matches!(self.owner, "dict" | "mappingproxy") && matches!(self.name, "__or__" | "__ror__")) => ctx.text("($self, value, /)"),
            "__doc__" if self.name == "__iadd__" => ctx.text("Implement self+=value."),
            "__text_signature__" if matches!(self.name, "__add__" | "__radd__" | "__sub__" | "__rsub__" | "__iadd__" | "__mul__" | "__rmul__" | "__truediv__" | "__rtruediv__" | "__mod__" | "__rmod__" | "__floordiv__" | "__rfloordiv__") => {
                ctx.text("($self, value, /)")
            }
            "__text_signature__" if self.name == "endswith" => {
                ctx.text("($self, suffix[, start[, end]], /)")
            }
            "__doc__" if self.name == "endswith" => ctx.text(format!(
                "Return True if the {owner} ends with the specified suffix, False otherwise.\n\n  suffix\n    A bytes or a tuple of bytes to try.\n  start\n    Optional start position. Default: start of the {owner}.\n  end\n    Optional stop position. Default: end of the {owner}.",
                owner = self.owner
            )),
            "__doc__" | "__text_signature__" if self.method => {
                let value = match (self.name, name) {
                    ("intersection_update", "__doc__") if self.owner == "set" => {
                        "Update the set, keeping only elements found in it and all others."
                    }
                    ("intersection_update", _) if self.owner == "set" => "($self, /, *others)",
                    ("utctimetuple", "__doc__") if self.owner == "datetime.datetime" => "Return UTC time tuple, compatible with time.localtime().",
                    ("utctimetuple", _) if self.owner == "datetime.datetime" => "($self, /)",
                    ("__reduce__", "__doc__") if matches!(self.owner, "builtin_function_or_method" | "method" | "types.GenericAlias" | "generic_alias_iterator") => return Some(NONE),
                    ("__reduce__", _) if matches!(self.owner, "builtin_function_or_method" | "method" | "types.GenericAlias" | "generic_alias_iterator") => "($self, /)",
                    ("__reduce__", "__doc__") if self.owner == stdlib::datetime::TZINFO => "-> (cls, state)",
                    ("__reduce__", _) if self.owner == stdlib::datetime::TZINFO => "($self, /)",
                    ("tzname", "__doc__") if self.owner == "datetime.timezone" => "If name is specified when timezone is created, returns the name.  Otherwise returns offset as 'UTC(+|-)HH:MM'.",
                    ("utcoffset", "__doc__") if self.owner == "datetime.timezone" => "Return fixed offset.",
                    ("dst", "__doc__") if self.owner == "datetime.timezone" => "Return None.",
                    ("tzname" | "utcoffset" | "dst", _) if self.owner == "datetime.timezone" => "($self, object, /)",
                    ("__getinitargs__", "__doc__") if self.owner == "datetime.timezone" => "pickle support",
                    ("__getinitargs__", _) if self.owner == "datetime.timezone" => "($self, /)",
                    ("__getstate__" | "__reduce__" | "__reduce_ex__", "__doc__") if self.owner == "object" => "Helper for pickle.",
                    ("__getstate__" | "__reduce__", _) if self.owner == "object" => "($self, /)",
                    ("__reduce_ex__", _) if self.owner == "object" => "($self, protocol, /)",
                    ("__replace__", "__doc__") if self.owner == stdlib::os::stat::CLASS => "Return a copy of the structure with new values for the specified fields.",
                    ("__replace__", _) if self.owner == stdlib::os::stat::CLASS => "($self, /, **changes)",
                    ("__reduce__", "__doc__") if stdlib::os::stat::layout(self.owner).is_some() => return Some(NONE),
                    ("__reduce__", _) if stdlib::os::stat::layout(self.owner).is_some() => "($self, /)",
                    ("count", "__doc__") if self.owner == "tuple" => "Return number of occurrences of value.",
                    ("count", _) if self.owner == "tuple" => "($self, value, /)",
                    ("replace", "__doc__") if matches!(self.owner, "bytes" | "bytearray") => {
                        "Return a copy with all occurrences of substring old replaced by new.\n\n  count\n    Maximum number of occurrences to replace.\n    -1 (the default value) means replace all occurrences.\n\nIf the optional argument count is given, only the first count occurrences are\nreplaced."
                    }
                    ("replace", _) if matches!(self.owner, "bytes" | "bytearray") => "($self, old, new, count=-1, /)",
                    ("count", "__doc__") if self.owner == "bytes" => {
                        "Return the number of non-overlapping occurrences of subsection 'sub' in bytes B[start:end].\n\n  start\n    Optional start position. Default: start of the bytes.\n  end\n    Optional stop position. Default: end of the bytes."
                    }
                    ("find", "__doc__") if self.owner == "bytes" => {
                        "Return the lowest index in B where subsection 'sub' is found, such that 'sub' is contained within B[start,end].\n\n  start\n    Optional start position. Default: start of the bytes.\n  end\n    Optional stop position. Default: end of the bytes.\n\nReturn -1 on failure."
                    }
                    ("__bytes__", "__doc__") => "Convert this value to exact type bytes.",
                    ("__complex__", "__doc__") => "Convert this value to exact type complex.",
                    ("__bytes__" | "__complex__", _) => "($self, /)",
                    ("index", "__doc__") if self.owner == "bytes" => {
                        "Return the lowest index in B where subsection 'sub' is found, such that 'sub' is contained within B[start,end].\n\n  start\n    Optional start position. Default: start of the bytes.\n  end\n    Optional stop position. Default: end of the bytes.\n\nRaise ValueError if the subsection is not found."
                    }
                    ("count" | "find" | "index", _) if self.owner == "bytes" => "($self, sub[, start[, end]], /)",
                    ("isascii", "__doc__") => {
                        "Return True if all characters in the string are ASCII, False otherwise.\n\nASCII characters have code points in the range U+0000-U+007F.\nEmpty string is ASCII too."
                    }
                    ("isalpha", "__doc__") => {
                        "Return True if the string is an alphabetic string, False otherwise.\n\nA string is alphabetic if all characters in the string are alphabetic and there\nis at least one character in the string."
                    }
                    ("isdigit", "__doc__") => {
                        "Return True if the string is a digit string, False otherwise.\n\nA string is a digit string if all characters in the string are digits and there\nis at least one character in the string."
                    }
                    ("isidentifier", "__doc__") => {
                        "Return True if the string is a valid Python identifier, False otherwise.\n\nCall keyword.iskeyword(s) to test whether string s is a reserved identifier,\nsuch as \"def\" or \"class\"."
                    }
                    ("islower", "__doc__") => {
                        "Return True if the string is a lowercase string, False otherwise.\n\nA string is lowercase if all cased characters in the string are lowercase and\nthere is at least one cased character in the string."
                    }
                    ("istitle", "__doc__") => {
                        "Return True if the string is a title-cased string, False otherwise.\n\nIn a title-cased string, upper- and title-case characters may only\nfollow uncased characters and lowercase characters only cased ones."
                    }
                    ("center", "__doc__") => {
                        "Return a centered string of length width.\n\nPadding is done using the specified fill character (default is a space)."
                    }
                    ("removeprefix", "__doc__") if self.owner == "bytes" => {
                        "Return a bytes object with the given prefix string removed if present.\n\nIf the bytes starts with the prefix string, return bytes[len(prefix):].\nOtherwise, return a copy of the original bytes."
                    }
                    ("removeprefix", _) if self.owner == "bytes" => "($self, prefix, /)",
                    ("ljust", "__doc__") if matches!(self.owner, "bytes" | "bytearray") => {
                        "Return a left-justified string of length width.\n\nPadding is done using the specified fill character."
                    }
                    ("ljust", _) if matches!(self.owner, "bytes" | "bytearray") => "($self, width, fillchar=b' ', /)",
                    ("ljust", "__doc__") => {
                        "Return a left-justified string of length width.\n\nPadding is done using the specified fill character (default is a space)."
                    }
                    ("rjust", "__doc__") => {
                        "Return a right-justified string of length width.\n\nPadding is done using the specified fill character (default is a space)."
                    }
                    ("center" | "ljust" | "rjust", _) => "($self, width, fillchar=' ', /)",
                    ("zfill", "__doc__") => {
                        "Pad a numeric string with zeros on the left, to fill a field of the given width.\n\nThe string is never truncated."
                    }
                    ("zfill", _) => "($self, width, /)",
                    ("upper", "__doc__") if matches!(self.owner, "bytes" | "bytearray") => {
                        "B.upper() -> copy of B\n\nReturn a copy of B with all ASCII characters converted to uppercase."
                    }
                    ("upper", _) if matches!(self.owner, "bytes" | "bytearray") => return Some(NONE),
                    ("lower", "__doc__") if self.owner == "bytes" => {
                        "B.lower() -> copy of B\n\nReturn a copy of B with all ASCII characters converted to lowercase."
                    }
                    ("lower", "__doc__") => "Return a copy of the string converted to lowercase.",
                    ("title", "__doc__") => {
                        "Return a version of the string where each word is titlecased.\n\nMore specifically, words start with uppercased characters and all remaining\ncased characters have lower case."
                    }
                    ("swapcase", "__doc__") => {
                        "Convert uppercase characters to lowercase and lowercase characters to uppercase."
                    }
                    (
                        "isascii" | "isalpha" | "isdigit" | "isidentifier" | "islower" | "istitle" | "lower" | "title"
                        | "swapcase",
                        _,
                    ) => "($self, /)",
                    ("mro", "__doc__") => "Return a type's method resolution order.",
                    ("mro", _) => "($self, /)",
                    ("__getitem__", "__doc__") if self.owner == "list" => "Return self[index].",
                    ("__getitem__", _) if self.owner == "list" => "($self, index, /)",
                    ("join", "__doc__") => "Concatenate any number of strings.\n\nThe string whose method is called is inserted in between each given string.\nThe result is returned as a new string.\n\nExample: '.'.join(['ab', 'pq', 'rs']) -> 'ab.pq.rs'",
                    ("join", _) => "($self, iterable, /)",
                    ("from_bytes", "__doc__") => "Return the integer represented by the given array of bytes.\n\n  bytes\n    Holds the array of bytes to convert.  The argument must either\n    support the buffer protocol or be an iterable object producing bytes.\n    Bytes and bytearray are examples of built-in objects that support the\n    buffer protocol.\n  byteorder\n    The byte order used to represent the integer.  If byteorder is 'big',\n    the most significant byte is at the beginning of the byte array.  If\n    byteorder is 'little', the most significant byte is at the end of the\n    byte array.  To request the native byte order of the host system, use\n    sys.byteorder as the byte order value.  Default is to use 'big'.\n  signed\n    Indicates whether two's complement is used to represent the integer.",
                    ("from_bytes", _) => "($type, /, bytes, byteorder='big', *, signed=False)",
                    ("fromkeys", "__doc__") => "Create a new dictionary with keys from iterable and values set to value.",
                    ("fromkeys", _) => "($type, iterable, value=None, /)",
                    ("append", "__doc__") => "Append object to the end of the list.",
                    ("pop", "__doc__") => {
                        "Remove and return item at index (default last).\n\nRaises IndexError if list is empty or index is out of range."
                    }
                    ("__round__", "__doc__") if self.owner == "int" => "Rounding an Integral returns itself.\n\nRounding with an ndigits argument also returns an integer.",
                    ("__round__", "__doc__") => "Return the Integral closest to x, rounding half toward even.\n\nWhen an argument is passed, work like built-in round(x, ndigits).",
                    ("__round__", _) => "($self, ndigits=None, /)",
                    ("__trunc__", "__doc__") if self.owner == "int" => "Truncating an Integral returns itself.",
                    ("__trunc__", "__doc__") => "Return the Integral closest to x between 0 and x.",
                    ("__trunc__", _) => "($self, /)",
                    ("as_integer_ratio", "__doc__") => {
                        "Return a pair of integers, whose ratio is exactly equal to the original float.\n\nThe ratio is in lowest terms and has a positive denominator.  Raise\nOverflowError on infinities and a ValueError on NaNs.\n\n>>> (10.0).as_integer_ratio()\n(10, 1)\n>>> (0.0).as_integer_ratio()\n(0, 1)\n>>> (-.25).as_integer_ratio()\n(-1, 4)"
                    }
                    ("as_integer_ratio", _) => "($self, /)",
                    ("append", _) => "($self, object, /)",
                    _ => "($self, index=-1, /)",
                };
                ctx.text(value)
            }
            _ => return None,
        })
    }
}

/// Implement the descriptor's explicit __get__ with Python's receiver and owner rules.
pub(super) fn descriptor_get(
    ctx: &mut Context,
    class: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&descriptor, arguments)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '__get__' of '{class}' object needs an argument"),
        ));
    };
    let payload = if matches!(class, "super" | stdlib::functools::PARTIAL_CLASS) {
        ctx.scalar_value(descriptor)
    } else {
        descriptor
    };
    let slot = match ctx.heap.get(payload) {
        Value::Super { .. } if class == "super" => None,
        Value::ClassMethod { .. } if class == "classmethod" => None,
        Value::Partial { .. } if class == "functools.partial" => None,
        Value::HeapTypeMethod { name, .. }
            if class
                == if matches!(*name, "__repr__" | "__str__") {
                    "wrapper_descriptor"
                } else {
                    "method_descriptor"
                } =>
        {
            None
        }
        Value::Native(symbol)
            if let Some(slot) = BuiltinDescriptor::lookup(symbol)
                && slot.class() == class =>
        {
            Some(slot)
        }
        _ => {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '__get__' requires a '{class}' object but received a '{}'",
                    ctx.builtin_type_name(descriptor)
                        .unwrap_or_else(|| ctx.type_name(descriptor))
                ),
            ));
        }
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "wrapper __get__() takes no keyword arguments",
        ));
    }
    match *arguments {
        [] => Err(Error::new(
            "TypeError",
            "__get__ expected at least 1 argument, got 0",
        )),
        [NONE] | [NONE, NONE] => Err(Error::new("TypeError", "__get__(None, None) is invalid")),
        [receiver] | [receiver, _] => {
            if class == stdlib::functools::PARTIAL_CLASS {
                return Ok(if receiver == NONE {
                    descriptor
                } else {
                    ctx.alloc(Value::Method {
                        callable: descriptor,
                        receiver,
                        explicit: true,
                    })
                });
            }
            if let Value::Super {
                start,
                receiver: bound,
                ..
            } = *ctx.heap.get(payload)
            {
                return ctx.bind_super(descriptor, receiver, start, bound);
            }
            if let Some(slot) = slot
                && slot.class() != "classmethod_descriptor"
                && !matches!(ctx.heap.get(descriptor), Value::Native(symbol) if stdlib::datetime::clock_classmethod(symbol))
            {
                return slot.bind(ctx, descriptor, receiver);
            }
            if let Value::HeapTypeMethod { owner, .. } = *ctx.heap.get(descriptor) {
                if receiver == NONE {
                    return Ok(descriptor);
                }
                return ctx.bind_attribute(descriptor, receiver, owner);
            }
            let owner = match arguments.get(1) {
                None | Some(&NONE) => ctx.exact_type(receiver)?,
                Some(&owner) => owner,
            };
            if let Some(slot) = slot {
                stdlib::datetime::validate_classmethod_receiver(ctx, slot.owner, slot.name, owner)?;
            }
            ctx.bind_attribute(descriptor, receiver, owner)
        }
        _ => Err(Error::new(
            "TypeError",
            format!(
                "__get__ expected at most 2 arguments, got {}",
                arguments.len()
            ),
        )),
    }
}
