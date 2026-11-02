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
            Some(("mappingproxy", attr)) => {
                Some(("mappingproxy", attr, builtins::MAPPING_PROXY_METHODS))
            }
            Some(("type", attr)) => Some(("type", attr, &["mro"][..])),
            Some(("int", attr)) => Some(("int", attr, builtins::real_methods("int"))),
            Some(("float", "as_integer_ratio")) => {
                Some(("float", "as_integer_ratio", &["as_integer_ratio"][..]))
            }
            Some(("float", attr)) => Some(("float", attr, builtins::real_methods("float"))),
            Some(("bytes", attr)) => Some(("bytes", attr, builtins::BYTES_METHODS)),
            Some(("complex", attr)) => Some(("complex", attr, &["__complex__"][..])),
            Some(("bytearray", attr)) => Some(("bytearray", attr, builtins::BYTEARRAY_METHODS)),
            Some(("memoryview", attr)) => Some(("memoryview", attr, builtins::MEMORYVIEW_METHODS)),
            Some(("BaseException", attr)) => {
                Some(("BaseException", attr, &["add_note", "with_traceback"][..]))
            }
            Some(("member_descriptor", attr)) => {
                Some(("member_descriptor", attr, super::namedtuple::MEMBER_SLOTS))
            }
            Some(("frame", attr)) => Some(("frame", attr, &["clear"][..])),
            Some(("set", attr)) => Some((
                "set",
                attr,
                &[
                    "__reduce__",
                    "difference",
                    "intersection",
                    "issubset",
                    "union",
                    "update",
                ][..],
            )),
            Some(("frozenset", attr)) => Some((
                "frozenset",
                attr,
                &[
                    "__reduce__",
                    "difference",
                    "intersection",
                    "issubset",
                    "union",
                ][..],
            )),
            Some(("list", attr)) => Some(("list", attr, builtins::LIST_METHODS)),
            Some(("range", attr)) => Some(("range", attr, range::METHODS)),
            Some((stdlib::fixed_structseq::SCHED_PARAM, attr)) => Some((
                stdlib::fixed_structseq::SCHED_PARAM,
                attr,
                &["__reduce__", "__replace__"][..],
            )),
            Some((class, attr)) if let Some(layout) = stdlib::os::stat::layout(class) => {
                Some((layout.class, attr, &["__reduce__"][..]))
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
            "object.__reduce_ex__" => ("object", "__reduce_ex__"),
            "int.__add__" => ("int", "__add__"),
            "int.__index__" => ("int", "__index__"),
            "bool.__repr__" => ("bool", "__repr__"),
            "int.__repr__" => ("int", "__repr__"),
            "float.__repr__" => ("float", "__repr__"),
            "int.__format__" => ("int", "__format__"),
            "str.__format__" => ("str", "__format__"),
            "str.__mod__" => ("str", "__mod__"),
            "str.__rmod__" => ("str", "__rmod__"),
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
            "object.__hash__" => ("object", "__hash__"),
            "object.__ne__" => ("object", "__ne__"),
            "object.__repr__" => ("object", "__repr__"),
            "object.__str__" => ("object", "__str__"),
            "function.__call__" => ("function", "__call__"),
            "classmethod.__get__" => ("classmethod", "__get__"),
            "method_descriptor.__get__" => ("method_descriptor", "__get__"),
            "wrapper_descriptor.__get__" => ("wrapper_descriptor", "__get__"),
            "str.__str__" => ("str", "__str__"),
            "str.__repr__" => ("str", "__repr__"),
            "bytes.__repr__" => ("bytes", "__repr__"),
            "dict.__or__" => ("dict", "__or__"),
            "dict.__ror__" => ("dict", "__ror__"),
            "dict.__ior__" => ("dict", "__ior__"),
            "dict.__eq__" => ("dict", "__eq__"),
            "dict.__repr__" => ("dict", "__repr__"),
            "mappingproxy.__repr__" => ("mappingproxy", "__repr__"),
            "mappingproxy.__str__" => ("mappingproxy", "__str__"),
            "tuple.__repr__" => ("tuple", "__repr__"),
            "set.__repr__" => ("set", "__repr__"),
            "frozenset.__repr__" => ("frozenset", "__repr__"),
            "types.SimpleNamespace.__init__" => (super::namespace::CLASS, "__init__"),
            "types.SimpleNamespace.__repr__" => (super::namespace::CLASS, "__repr__"),
            "types.SimpleNamespace.__eq__" => (super::namespace::CLASS, "__eq__"),
            "types.SimpleNamespace.__ne__" => (super::namespace::CLASS, "__ne__"),
            "tuple.__getitem__" => ("tuple", "__getitem__"),
            "tuple.__len__" => ("tuple", "__len__"),
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
                    | "__sizeof__"
                    | "__getstate__"
                    | "__reduce__"
                    | "__reduce_ex__"
                    | "count"
                    | "index"
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
        if self.method {
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
            owner if typing::ParameterClass::lookup(owner).is_some() => {
                ctx.builtin_type_name(receiver) == Some(owner)
            }
            "typing.TypeAliasType" => matches!(ctx.heap.get(receiver), Value::TypeAlias { .. }),
            "typing.Union" => matches!(ctx.heap.get(receiver), Value::Union(_)),
            "object" => true,
            "type" => ctx.is_type_object(receiver),
            "function" => matches!(ctx.heap.get(receiver), Value::Function { .. }),
            "classmethod" => matches!(ctx.heap.get(receiver), Value::ClassMethod { .. }),
            "functools.KeyWrapper" => matches!(ctx.heap.get(receiver), Value::KeyWrapper { .. }),
            super::namespace::CLASS => {
                ctx.instance_of(receiver, ctx.classes[super::namespace::CLASS])
            }
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
            "slice" => matches!(ctx.heap.get(receiver), Value::Slice(_)),
            "member_descriptor" => ctx.exact_type(receiver)? == ctx.builtin("member_descriptor"),
            "method_descriptor" | "wrapper_descriptor" => {
                ctx.builtin_type_name(receiver) == Some(self.owner)
            }
            "dict" => ctx.dict_storage(receiver).is_some(),
            "mappingproxy" => matches!(ctx.heap.get(receiver), Value::MappingProxy(_)),
            deque::ITERATOR | deque::REVERSE_ITERATOR => ctx.heap.get(receiver).intrinsic_type_name() == Some(self.owner),
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
            "__doc__" | "__text_signature__" if self.owner == deque::CLASS => {
                let member = deque::member(self.name)?;
                ctx.text(if name == "__doc__" { member.doc } else { member.signature })
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
            "__name__" => ctx.text(self.name),
            "__qualname__" => {
                let owner = if self.method
                    && let Some(receiver) = receiver
                {
                    match ctx.heap.get(receiver) {
                        _ if self.owner == "type" => ctx.class_qualname(receiver),
                        _ if matches!(self.owner, value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE) => self.owner.rsplit('.').next()?,
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
            "__objclass__" if receiver.is_none() || !self.method => ctx.builtin(self.owner),
            "__module__" if receiver.is_some() && self.method => NONE,
            "__text_signature__" if matches!(self.owner, value::REGEX_PATTERN_TYPE | value::REGEX_MATCH_TYPE) => {
                ctx.text(if self.name == "__copy__" { "($self, /)" } else { "($self, memo, /)" })
            }
            "__doc__" | "__text_signature__"
                if self.name == "__len__" && matches!(self.owner, "list" | "tuple") =>
            {
                ctx.text(if name == "__doc__" {
                    "Return len(self)."
                } else {
                    "($self, /)"
                })
            }
            "__doc__" if self.name == "__getattribute__" => ctx.text("Return getattr(self, name)."),
            "__text_signature__" if self.name == "__getattribute__" => ctx.text("($self, name, /)"),
            "__doc__" if self.owner == "itertools.repeat" && self.name == "__length_hint__" => {
                ctx.text("Private method returning an estimate of len(list(it)).")
            }
            "__text_signature__" if self.owner == "itertools.repeat" && self.name == "__length_hint__" => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.owner == "mappingproxy" && self.method => ctx.text(match self.name {
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
            "__doc__" if self.name == "__repr__" => ctx.text("Return repr(self)."),
            "__doc__" if self.name == "__iter__" => ctx.text("Implement iter(self)."),
            "__doc__" if self.name == "__next__" => ctx.text("Implement next(self)."),
            "__text_signature__" if matches!(self.name, "__iter__" | "__next__" | "__repr__") => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.name == "__contains__" => ctx.text("Return bool(key in self)."),
            "__text_signature__" if self.name == "__contains__" => ctx.text("($self, key, /)"),
            "__doc__" if self.name == "__neg__" => ctx.text("-self"),
            "__doc__" if self.name == "__pos__" => ctx.text("+self"),
            "__doc__" if self.name == "__int__" => ctx.text("int(self)"),
            "__doc__" if self.name == "__float__" => ctx.text("float(self)"),
            "__doc__" if self.name == "__abs__" => ctx.text("abs(self)"),
            "__text_signature__" if matches!(self.name, "__neg__" | "__pos__" | "__abs__" | "__int__" | "__float__") => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.name == "__index__" => ctx.text(
                "Return self converted to an integer, if self is suitable for use as an index into a list.",
            ),
            "__text_signature__" if self.name == "__index__" => ctx.text("($self, /)"),
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
            "__doc__" if self.owner == "dict" && self.name == "__or__" => ctx.text("Return self|value."),
            "__doc__" if self.owner == "dict" && self.name == "__ror__" => ctx.text("Return value|self."),
            "__text_signature__" if self.name == "__ior__" || (self.owner == "dict" && matches!(self.name, "__or__" | "__ror__")) => ctx.text("($self, value, /)"),
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
                    ("utctimetuple", "__doc__") if self.owner == "datetime.datetime" => "Return UTC time tuple, compatible with time.localtime().",
                    ("utctimetuple", _) if self.owner == "datetime.datetime" => "($self, /)",
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
                    ("__reduce__", "__doc__") if stdlib::os::stat::layout(self.owner).is_some() => return Some(NONE),
                    ("__reduce__", _) if stdlib::os::stat::layout(self.owner).is_some() => "($self, /)",
                    ("count", "__doc__") if self.owner == "tuple" => "Return number of occurrences of value.",
                    ("count", _) if self.owner == "tuple" => "($self, value, /)",
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
    let slot = match ctx.heap.get(descriptor) {
        Value::ClassMethod { .. } if class == "classmethod" => None,
        Value::HeapTypeMethod { name, .. }
            if class
                == if *name == "__repr__" {
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
            if let Some(slot) = slot
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
