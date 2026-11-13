use super::*;

/// A builtin descriptor and the receiver type accepted by its binding operation.
#[derive(Clone, Copy)]
pub(super) struct BuiltinDescriptor {
    owner: &'static str,
    pub(super) name: &'static str,
    method: bool,
}

impl BuiltinDescriptor {
    pub(super) fn lookup(symbol: &str) -> Option<Self> {
        if let Some(attr) = symbol.strip_prefix("dict.")
            && builtins::constructor_owner(symbol).is_none()
            && let Some(&name) = builtins::DICT_METHODS.iter().find(|&&name| name == attr)
        {
            return Some(Self {
                owner: "dict",
                name,
                method: !builtins::dict_slot(symbol),
            });
        }
        if let Some(attr) = symbol.strip_prefix("str.")
            && let Some(&name) = builtins::STR_METHODS.iter().find(|&&name| name == attr)
        {
            return Some(Self {
                owner: "str",
                name,
                method: !name.starts_with("__") || name == "__format__",
            });
        }
        if let Some(attr) = symbol.strip_prefix("_thread._ThreadHandle.")
            && let Some(&name) = thread_handle::METHODS.iter().find(|&&name| name == attr)
        {
            return Some(Self {
                owner: thread_handle::CLASS,
                name,
                method: name != "__repr__",
            });
        }
        if let Some(owner) = super::builtins::exception_init_descriptor(symbol) {
            return Some(Self {
                owner,
                name: "__init__",
                method: false,
            });
        }
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
        if let Some(owner) = typing::generic_alias_method_owner(symbol) {
            return Some(Self {
                owner,
                name: "__class_getitem__",
                method: true,
            });
        }
        if let Some((stdlib::functools::PARTIAL_CLASS, attr)) = symbol.rsplit_once('.')
            && let Some(&name) = ["__reduce__", "__setstate__"]
                .iter()
                .find(|&&name| name == attr)
        {
            return Some(Self {
                owner: stdlib::functools::PARTIAL_CLASS,
                name,
                method: true,
            });
        }
        if symbol == "typing.Union.__mro_entries__" {
            return Some(Self {
                owner: "typing.Union",
                name: "__mro_entries__",
                method: true,
            });
        }
        if let Some((owner, _)) = symbol
            .strip_suffix(".__call__")
            .and_then(stdlib::operator::callable_type)
        {
            return Some(Self {
                owner,
                name: "__call__",
                method: false,
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
                stdlib::functools::LRU_METHODS,
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
            Some((value::ZSTD_COMPRESS_TYPE, attr)) => Some((
                value::ZSTD_COMPRESS_TYPE,
                attr,
                value::ZSTD_COMPRESS_METHODS,
            )),
            Some((value::ZSTD_DECOMPRESS_TYPE, attr)) => Some((
                value::ZSTD_DECOMPRESS_TYPE,
                attr,
                value::CODEC_DECOMPRESS_METHODS,
            )),
            Some((value::BZ2_COMPRESS_TYPE, attr)) => Some((
                value::BZ2_COMPRESS_TYPE,
                attr,
                value::CODEC_COMPRESS_METHODS,
            )),
            Some((value::BZ2_DECOMPRESS_TYPE, attr)) => Some((
                value::BZ2_DECOMPRESS_TYPE,
                attr,
                value::CODEC_DECOMPRESS_METHODS,
            )),

            Some((value::LZMA_COMPRESS_TYPE, attr)) => Some((
                value::LZMA_COMPRESS_TYPE,
                attr,
                value::CODEC_COMPRESS_METHODS,
            )),
            Some((value::LZMA_DECOMPRESS_TYPE, attr)) => Some((
                value::LZMA_DECOMPRESS_TYPE,
                attr,
                value::CODEC_DECOMPRESS_METHODS,
            )),
            Some((value::ZLIB_DECOMPRESS_TYPE, attr)) => Some((
                value::ZLIB_DECOMPRESS_TYPE,
                attr,
                value::ZLIB_DECOMPRESS_COPY_METHODS,
            )),
            Some((value::REGEX_PATTERN_TYPE, attr)) => {
                Some((value::REGEX_PATTERN_TYPE, attr, value::REGEX_COPY_METHODS))
            }
            Some((value::REGEX_MATCH_TYPE, attr)) => {
                Some((value::REGEX_MATCH_TYPE, attr, value::REGEX_COPY_METHODS))
            }
            Some((defaultdict::CLASS, attr)) => {
                Some((defaultdict::CLASS, attr, defaultdict::METHODS))
            }
            Some(("filter", attr)) => Some(("filter", attr, builtins::FILTER_METHODS)),
            Some(("map", attr)) => Some(("map", attr, builtins::ITERATOR_SLOTS)),
            Some(("zip", attr)) => Some(("zip", attr, builtins::ITERATOR_SLOTS)),
            Some(("set_iterator", attr)) => {
                Some(("set_iterator", attr, builtins::SET_ITERATOR_METHODS))
            }
            Some(("list_iterator", attr)) => {
                Some(("list_iterator", attr, &["__iter__", "__next__"][..]))
            }
            Some(("tuple_iterator", attr)) => {
                Some(("tuple_iterator", attr, &["__iter__", "__next__"][..]))
            }
            Some(("generator", attr)) => Some(("generator", attr, &["__iter__", "__next__"][..])),
            Some(("struct.Struct", attr)) => {
                Some(("struct.Struct", attr, builtins::STRUCT_METHODS))
            }
            Some((value::STRUCT_UNPACK_ITERATOR, attr)) => Some((
                value::STRUCT_UNPACK_ITERATOR,
                attr,
                builtins::UNPACK_ITERATOR_METHODS,
            )),
            Some(("memory_iterator", attr)) => {
                Some(("memory_iterator", attr, &["__iter__", "__next__"][..]))
            }
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
            Some(("mappingproxy", attr)) => {
                let members = if builtins::MAPPING_PROXY_METHODS.contains(&attr) {
                    builtins::MAPPING_PROXY_METHODS
                } else {
                    builtins::MAPPING_PROXY_SLOTS
                };
                Some(("mappingproxy", attr, members))
            }
            Some(("type", attr)) => {
                Some(("type", attr, &["mro", "__subclasses__", "__setattr__"][..]))
            }
            Some(("int", attr)) => Some(("int", attr, builtins::real_methods("int"))),
            Some(("float", "as_integer_ratio")) => {
                Some(("float", "as_integer_ratio", &["as_integer_ratio"][..]))
            }
            Some(("float", attr)) => Some(("float", attr, builtins::real_methods("float"))),
            Some(("bytes", attr)) => Some(("bytes", attr, builtins::BYTES_METHODS)),
            Some(("complex", attr)) => Some(("complex", attr, super::complex::METHODS)),
            Some(("bytearray", attr)) => Some(("bytearray", attr, builtins::BYTEARRAY_METHODS)),
            Some(("memoryview", attr)) => Some(("memoryview", attr, builtins::MEMORYVIEW_METHODS)),
            Some(("BaseExceptionGroup", attr)) => {
                Some(("BaseExceptionGroup", attr, super::exception_group::METHODS))
            }
            Some((owner, attr)) if let Some(kind) = builtins::exception_kind(owner) => {
                Some((kind.name(), attr, builtins::exception_methods(kind)))
            }
            Some(("getset_descriptor", attr)) => {
                Some(("getset_descriptor", attr, super::namedtuple::MEMBER_SLOTS))
            }
            Some(("member_descriptor", attr)) => {
                Some(("member_descriptor", attr, super::namedtuple::MEMBER_SLOTS))
            }
            Some(("frame", attr)) => Some(("frame", attr, &["clear"][..])),
            Some(("set", attr)) => Some(("set", attr, builtins::set_methods("set"))),
            Some(("frozenset", attr)) => {
                Some(("frozenset", attr, builtins::set_methods("frozenset")))
            }
            Some(("list", attr)) if attr != "__new__" => {
                Some(("list", attr, builtins::LIST_METHODS))
            }
            Some(("range", attr)) => Some(("range", attr, range::METHODS)),
            Some((stdlib::fixed_structseq::SCHED_PARAM, attr)) => Some((
                stdlib::fixed_structseq::SCHED_PARAM,
                attr,
                &["__reduce__", "__replace__"][..],
            )),
            Some((class, attr)) if let Some(layout) = stdlib::structseq::layout(class) => {
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
                    || (name == "__length_hint__"
                        && (matches!(owner, "set_iterator" | value::STRUCT_UNPACK_ITERATOR)
                            || deque::iterator_class(owner).is_some()))
                    || (owner == "type" && name == "__subclasses__")
                    || (owner == "list" && name == "__getitem__")
                    || (matches!(owner, "set" | "frozenset") && name == "__contains__")
                    || (owner == defaultdict::CLASS && name == "__missing__")
                    || matches!(
                        name,
                        "__copy__"
                            | "__deepcopy__"
                            | "__reversed__"
                            | "__ceil__"
                            | "__floor__"
                            | "__round__"
                            | "__trunc__"
                            | "__getinitargs__"
                            | "__getnewargs__"
                            | "__setstate__"
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
            "type.__call__" => ("type", "__call__"),
            "type.__init__" => ("type", "__init__"),
            "super.__getattribute__" => ("super", "__getattribute__"),
            "super.__init__" => ("super", "__init__"),
            "super.__get__" => ("super", "__get__"),
            "super.__repr__" => ("super", "__repr__"),
            "object.__format__" => ("object", "__format__"),
            "object.__sizeof__" => ("object", "__sizeof__"),
            "dict.__sizeof__" => ("dict", "__sizeof__"),
            "int.__sizeof__" => ("int", "__sizeof__"),
            "str.__sizeof__" => ("str", "__sizeof__"),
            "bytearray.__sizeof__" => ("bytearray", "__sizeof__"),
            "list.__sizeof__" => ("list", "__sizeof__"),
            "set.__sizeof__" => ("set", "__sizeof__"),
            "frozenset.__sizeof__" => ("frozenset", "__sizeof__"),
            "object.__getstate__" => ("object", "__getstate__"),
            "object.__reduce__" => ("object", "__reduce__"),
            "method-wrapper.__reduce__" => ("method-wrapper", "__reduce__"),
            "method.__reduce__" => ("method", "__reduce__"),
            "builtin_function_or_method.__reduce__" => ("builtin_function_or_method", "__reduce__"),
            "object.__reduce_ex__" => ("object", "__reduce_ex__"),
            "int.__add__" => ("int", "__add__"),
            "int.__index__" => ("int", "__index__"),
            "float.fromhex" => ("float", "fromhex"),
            "bytes.fromhex" => ("bytes", "fromhex"),
            "bytearray.fromhex" => ("bytearray", "fromhex"),
            "int.from_bytes" => ("int", "from_bytes"),
            "bool.__repr__" => ("bool", "__repr__"),
            "int.__repr__" => ("int", "__repr__"),
            "float.__repr__" => ("float", "__repr__"),
            "int.__format__" => ("int", "__format__"),
            "float.__format__" => ("float", "__format__"),
            "collections._tuplegetter.__repr__" => (super::namedtuple::GETTER, "__repr__"),
            "module.__init__" => ("module", "__init__"),
            "module.__dir__" => ("module", "__dir__"),
            "module.__repr__" => ("module", "__repr__"),
            "module.__getattribute__" => ("module", "__getattribute__"),
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
            "function.__get__" => ("function", "__get__"),
            "method.__get__" => ("method", "__get__"),
            "classmethod.__get__" => ("classmethod", "__get__"),
            "functools.partial.__call__" => ("functools.partial", "__call__"),
            "functools.partial.__get__" => ("functools.partial", "__get__"),
            "functools.partial.__repr__" => (stdlib::functools::PARTIAL_CLASS, "__repr__"),
            "method_descriptor.__get__" => ("method_descriptor", "__get__"),
            "wrapper_descriptor.__get__" => ("wrapper_descriptor", "__get__"),
            "bytes.__repr__" => ("bytes", "__repr__"),
            "tuple.__repr__" => ("tuple", "__repr__"),
            "types.SimpleNamespace.__init__" => (super::namespace::CLASS, "__init__"),
            "types.SimpleNamespace.__repr__" => (super::namespace::CLASS, "__repr__"),
            "types.SimpleNamespace.__eq__" => (super::namespace::CLASS, "__eq__"),
            "types.SimpleNamespace.__ne__" => (super::namespace::CLASS, "__ne__"),
            "tuple.__getitem__" => ("tuple", "__getitem__"),
            "tuple.__getnewargs__" => ("tuple", "__getnewargs__"),
            "tuple.__iter__" => ("tuple", "__iter__"),
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
                    | "__getnewargs__"
                    | "__getstate__"
                    | "__reduce__"
                    | "__reduce_ex__"
                    | "__reversed__"
                    | "count"
                    | "index"
                    | "fromhex"
                    | "from_bytes"
            ),
        })
    }

    /// Report the descriptor type exposed by type() and explicit __get__.
    pub(super) fn class(self) -> &'static str {
        if matches!(
            (self.owner, self.name),
            ("object", "__subclasshook__" | "__init_subclass__")
                | ("int", "from_bytes")
                | ("float" | "bytes" | "bytearray", "fromhex")
                | ("dict", "fromkeys")
                | (_, "__class_getitem__")
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
        let kind = if self.class() == "wrapper_descriptor" {
            "slot wrapper"
        } else {
            "method"
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
            owner if self.name == "__init__" && builtins::is_exception(owner) => {
                ctx.instance_of(receiver, ctx.builtins[owner])
            }
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
            "list_iterator"
            | "tuple_iterator"
            | "memory_iterator"
            | thread_handle::CLASS
            | value::ZSTD_COMPRESS_TYPE
            | value::ZSTD_DECOMPRESS_TYPE
            | value::BZ2_COMPRESS_TYPE
            | value::BZ2_DECOMPRESS_TYPE
            | value::LZMA_COMPRESS_TYPE
            | value::LZMA_DECOMPRESS_TYPE
            | value::STRUCT_UNPACK_ITERATOR
            | "generator"
            | "async_generator"
            | "async_generator_asend"
            | "async_generator_athrow" => {
                ctx.heap.get(receiver).intrinsic_type_name() == Some(self.owner)
            }
            "anext_awaitable" => matches!(ctx.heap.get(receiver), Value::ANextAwaitable { .. }),
            owner if typing::ParameterClass::lookup(owner).is_some() => {
                ctx.builtin_type_name(receiver) == Some(owner)
            }
            value::ZLIB_DECOMPRESS_TYPE => {
                matches!(ctx.heap.get(receiver), Value::ZlibDecompressor { .. })
            }
            "typing.TypeAliasType" => matches!(ctx.heap.get(receiver), Value::TypeAlias { .. }),
            "typing.Union" => matches!(ctx.heap.get(receiver), Value::Union(_)),
            super::namedtuple::GETTER => {
                matches!(ctx.heap.get(receiver), Value::TupleGetter { .. })
            }
            "BaseExceptionGroup" => {
                matches!(ctx.heap.get(receiver), Value::Raised(error) if error.group().is_some())
            }
            "object" => true,
            "type" => ctx.is_type_object(receiver),
            "function" => matches!(ctx.heap.get(receiver), Value::Function { .. }),
            "classmethod" => matches!(ctx.heap.get(receiver), Value::ClassMethod { .. }),
            stdlib::functools::PARTIAL_CLASS => ctx.builtin_slot_accepts(receiver, self.owner),
            "functools.KeyWrapper" => matches!(ctx.heap.get(receiver), Value::KeyWrapper { .. }),
            super::namespace::CLASS => namespace::instance_class(ctx, receiver).is_some(),
            "frame" => matches!(ctx.heap.get(receiver), Value::PythonFrame { .. }),
            owner if builtins::is_exception(owner) => ctx.builtin_slot_accepts(receiver, owner),
            class
                if class == stdlib::fixed_structseq::SCHED_PARAM
                    || stdlib::structseq::layout(class).is_some() =>
            {
                ctx.builtin_slot_accepts(receiver, class)
            }
            stdlib::os::scandir::ENTRY => matches!(ctx.heap.get(receiver), Value::DirEntry(_)),
            stdlib::os::scandir::ITER => matches!(ctx.heap.get(receiver), Value::Scandir(_)),
            "range" => matches!(ctx.heap.get(receiver), Value::Range(_)),
            "memoryview" => matches!(ctx.heap.get(receiver), Value::MemoryView { .. }),
            "array.array" => matches!(ctx.heap.get(receiver), Value::Array(_)),
            "slice" => matches!(ctx.heap.get(receiver), Value::Slice(_)),
            "struct.Struct" => matches!(
                ctx.heap.get(ctx.scalar_value(receiver)),
                Value::Struct { .. }
            ),
            "member_descriptor" | "getset_descriptor" => {
                ctx.exact_type(receiver)? == ctx.builtin(self.owner)
            }
            "method_descriptor" | "wrapper_descriptor" | "classmethod_descriptor" => {
                ctx.builtin_type_name(receiver) == Some(self.owner)
            }
            "dict" => ctx.dict_storage(receiver).is_some(),
            "mappingproxy" => matches!(ctx.heap.get(receiver), Value::MappingProxy(_)),
            deque::ITERATOR | deque::REVERSE_ITERATOR => {
                ctx.heap.get(receiver).intrinsic_type_name() == Some(self.owner)
            }
            deque::CLASS => matches!(ctx.heap.get(ctx.scalar_value(receiver)), Value::Deque(_)),
            "module" => matches!(ctx.heap.get(receiver), Value::Module { .. }),
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
            "__text_signature__" if self.owner == stdlib::functools::LRU_CACHE_CLASS && matches!(self.name, "__reduce__" | "cache_info" | "cache_clear") => ctx.text("($self, /)"),
            "__doc__" if self.owner == stdlib::functools::LRU_CACHE_CLASS => match self.name {
                "cache_info" => ctx.text("Report cache statistics"),
                "cache_clear" => ctx.text("Clear the cache and cache statistics"),
                _ => NONE,
            },
            "__text_signature__" if self.owner == stdlib::functools::LRU_CACHE_CLASS => NONE,
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
            "__doc__" if self.name == "__class_getitem__" => ctx.text("See PEP 585"),
            "__text_signature__" if self.name == "__class_getitem__" => ctx.text("($type, object, /)"),
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
                let owner = if self.class() != "wrapper_descriptor"
                    && let Some(receiver) = receiver
                {
                    match ctx.heap.get(receiver) {
                        _ if self.owner == "type" || self.class() == "classmethod_descriptor" => ctx.class_qualname(receiver),
                        _ if matches!(self.owner, value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE | value::ZSTD_COMPRESS_TYPE | value::ZSTD_DECOMPRESS_TYPE | value::BZ2_COMPRESS_TYPE | value::BZ2_DECOMPRESS_TYPE | value::LZMA_COMPRESS_TYPE | value::LZMA_DECOMPRESS_TYPE) => self.owner.rsplit('.').next()?,
                        _ if matches!(self.owner, "builtin_function_or_method" | "method" | "method-wrapper") => self.owner,
                        Value::Deque(_) if self.owner == deque::CLASS => "deque",
                        Value::DefaultDict { .. } if self.owner == defaultdict::CLASS => "defaultdict",
                        Value::NamedTuple { class: Some(owner), .. } if let Some(layout) = stdlib::structseq::layout(owner) => layout.name(),
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
            "__objclass__" if receiver.is_none() || self.class() == "wrapper_descriptor" => ctx.builtin(self.owner),
            "__module__" if receiver.is_some() && self.class() != "wrapper_descriptor" => NONE,
            "__text_signature__" if self.owner == value::ZLIB_DECOMPRESS_TYPE && self.name == "__deepcopy__" => {
                ctx.text("($self, memo, /)")
            }
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
            "__doc__" | "__text_signature__" if matches!(self.owner, "int" | "float" | "mappingproxy" | "str")
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
            "__doc__" if self.owner == "module" && self.name == "__dir__" => ctx.text("__dir__() -> list\nspecialized dir() implementation"),
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
            "__doc__" if self.name == "with_traceback" => ctx.text("Set self.__traceback__ to tb and return self."),
            "__text_signature__" if self.name == "with_traceback" => ctx.text("($self, tb, /)"),
            "__doc__" if self.name == "add_note" => ctx.text("Add a note to the exception"),
            "__text_signature__" if self.name == "add_note" => ctx.text("($self, note, /)"),
            "__doc__" if self.name == "__call__" => ctx.text("Call self as a function."),
            "__text_signature__" if self.name == "__call__" => {
                ctx.text("($self, /, *args, **kwargs)")
            }
            "__doc__" if self.name == "__getitem__" && matches!(self.owner, "mappingproxy" | "str") => {
                ctx.text("Return self[key].")
            }
            "__text_signature__" if self.name == "__getitem__" && matches!(self.owner, "mappingproxy" | "str") => {
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
            "__doc__" if self.owner == "int" && let Some((op, reflected)) = super::operators::numeric_binary_slot(self.owner, self.name) => {
                ctx.text(if reflected { format!("Return value{op}self.") } else { format!("Return self{op}value.") })
            }
            "__text_signature__" if self.owner == "int" && super::operators::numeric_binary_slot(self.owner, self.name).is_some() => ctx.text("($self, value, /)"),
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
            "__doc__" if self.owner == "list" && self.name == "__setitem__" => ctx.text("Set self[key] to value."),
            "__text_signature__" if self.owner == "list" && self.name == "__setitem__" => ctx.text("($self, key, value, /)"),
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
                    ("__reduce__", "__doc__") if matches!(self.owner, "builtin_function_or_method" | "method" | "method-wrapper" | "types.GenericAlias" | "generic_alias_iterator") => return Some(NONE),
                    ("__reduce__", _) if matches!(self.owner, "builtin_function_or_method" | "method" | "method-wrapper" | "types.GenericAlias" | "generic_alias_iterator") => "($self, /)",
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
                    ("__replace__", "__doc__") if stdlib::structseq::layout(self.owner).is_some() => "Return a copy of the structure with new values for the specified fields.",
                    ("__replace__", _) if stdlib::structseq::layout(self.owner).is_some() => "($self, /, **changes)",
                    ("__reduce__", "__doc__") if stdlib::structseq::layout(self.owner).is_some() => return Some(NONE),
                    ("__reduce__", _) if stdlib::structseq::layout(self.owner).is_some() => "($self, /)",
                    ("__getnewargs__", "__doc__") => return Some(NONE),
                    ("__getnewargs__", _) => "($self, /)",
                    ("count", "__doc__") if self.owner == "tuple" => "Return number of occurrences of value.",
                    ("count", _) if self.owner == "tuple" => "($self, value, /)",
                    ("partition", "__doc__") if self.owner == "bytes" => {
                        "Partition the bytes into three parts using the given separator.\n\nThis will search for the separator sep in the bytes. If the separator is found,\nreturns a 3-tuple containing the part before the separator, the separator\nitself, and the part after it.\n\nIf the separator is not found, returns a 3-tuple containing the original bytes\nobject and two empty bytes objects."
                    }
                    ("partition", _) if self.owner == "bytes" => "($self, sep, /)",
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
                    ("isprintable", "__doc__") => {
                        "Return True if all characters in the string are printable, False otherwise.\n\nA character is printable if repr() may use it in its output."
                    }
                    ("isnumeric", "__doc__") => {
                        "Return True if the string is a numeric string, False otherwise.\n\nA string is numeric if all characters in the string are numeric and there is at\nleast one character in the string."
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
                    ("rjust", "__doc__") if self.owner == "bytes" => {
                        "Return a right-justified string of length width.\n\nPadding is done using the specified fill character."
                    }
                    ("ljust" | "rjust", _) if matches!(self.owner, "bytes" | "bytearray") => "($self, width, fillchar=b' ', /)",
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
                    ("casefold", "__doc__") => "Return a version of the string suitable for caseless comparisons.",
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
                        "isascii" | "isalpha" | "isdigit" | "isidentifier" | "islower" | "isnumeric" | "isprintable" | "istitle" | "lower" | "title"
                        | "swapcase" | "casefold",
                        _,
                    ) => "($self, /)",
                    ("mro", "__doc__") => "Return a type's method resolution order.",
                    ("__subclasses__", "__doc__") => "Return a list of immediate subclasses.",
                    ("mro" | "__subclasses__", _) => "($self, /)",
                    ("__getitem__", "__doc__") if self.owner == "list" => "Return self[index].",
                    ("__getitem__", _) if self.owner == "list" => "($self, index, /)",
                    ("join", "__doc__") => "Concatenate any number of strings.\n\nThe string whose method is called is inserted in between each given string.\nThe result is returned as a new string.\n\nExample: '.'.join(['ab', 'pq', 'rs']) -> 'ab.pq.rs'",
                    ("join", _) => "($self, iterable, /)",
                    ("from_bytes", "__doc__") => "Return the integer represented by the given array of bytes.\n\n  bytes\n    Holds the array of bytes to convert.  The argument must either\n    support the buffer protocol or be an iterable object producing bytes.\n    Bytes and bytearray are examples of built-in objects that support the\n    buffer protocol.\n  byteorder\n    The byte order used to represent the integer.  If byteorder is 'big',\n    the most significant byte is at the beginning of the byte array.  If\n    byteorder is 'little', the most significant byte is at the end of the\n    byte array.  To request the native byte order of the host system, use\n    sys.byteorder as the byte order value.  Default is to use 'big'.\n  signed\n    Indicates whether two's complement is used to represent the integer.",
                    ("from_bytes", _) => "($type, /, bytes, byteorder='big', *, signed=False)",
                    ("hex", "__doc__") if self.owner == "float" => "Return a hexadecimal representation of a floating-point number.\n\n>>> (-0.1).hex()\n'-0x1.999999999999ap-4'\n>>> 3.14159.hex()\n'0x1.921f9f01b866ep+1'",
                    ("hex", _) if self.owner == "float" => "($self, /)",
                    ("fromhex", "__doc__") if self.owner == "bytes" => "Create a bytes object from a string of hexadecimal numbers.\n\nSpaces between two numbers are accepted.\nExample: bytes.fromhex('B9 01EF') -> b'\\\\xb9\\\\x01\\\\xef'.",
                    ("fromhex", "__doc__") if self.owner == "bytearray" => "Create a bytearray object from a string of hexadecimal numbers.\n\nSpaces between two numbers are accepted.\nExample: bytearray.fromhex('B9 01EF') -> bytearray(b'\\\\xb9\\\\x01\\\\xef')",
                    ("fromhex", "__doc__") => "Create a floating-point number from a hexadecimal string.\n\n>>> float.fromhex('0x1.ffffp10')\n2047.984375\n>>> float.fromhex('-0x1p-1074')\n-5e-324",
                    ("fromhex", _) => "($type, string, /)",
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
                    ("__ceil__", "__doc__") => "Return the ceiling as an Integral.",
                    ("__floor__", "__doc__") => "Return the floor as an Integral.",
                    ("__ceil__" | "__floor__" | "__trunc__", _) => "($self, /)",
                    ("as_integer_ratio", "__doc__") => {
                        "Return a pair of integers, whose ratio is exactly equal to the original float.\n\nThe ratio is in lowest terms and has a positive denominator.  Raise\nOverflowError on infinities and a ValueError on NaNs.\n\n>>> (10.0).as_integer_ratio()\n(10, 1)\n>>> (0.0).as_integer_ratio()\n(0, 1)\n>>> (-.25).as_integer_ratio()\n(-1, 4)"
                    }
                    ("is_integer", "__doc__") => "Return True if the float is an integer.",
                    ("as_integer_ratio" | "is_integer", _) => "($self, /)",
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
        Value::Function { .. } if class == "function" => None,
        Value::Method { .. }
            if class == "method" && ctx.builtin_slot_accepts(descriptor, class) =>
        {
            None
        }
        Value::Cached { .. } if class == stdlib::functools::LRU_CACHE_CLASS => None,
        Value::Super { .. } if class == "super" => None,
        Value::ClassMethod { .. } if class == "classmethod" => None,
        Value::Partial { .. } if class == "functools.partial" => None,
        member @ Value::HeapTypeMethod { .. } if member.intrinsic_type_name() == Some(class) => {
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
                    ctx.bind_python_method(descriptor, receiver)
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
