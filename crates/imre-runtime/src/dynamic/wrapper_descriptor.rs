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
            Some((owner, attr)) if let Some(class) = DictIterator::class(owner) => {
                Some((class, attr, DictIterator::methods(class)))
            }
            Some(("generator", attr)) => Some(("generator", attr, &["__iter__", "__next__"][..])),
            Some(("type", attr)) => Some(("type", attr, &["mro"][..])),
            Some(("float", attr)) => Some(("float", attr, &["as_integer_ratio"][..])),
            Some(("bytes", attr)) => Some(("bytes", attr, builtins::BYTES_METHODS)),
            Some(("bytearray", attr)) => Some(("bytearray", attr, builtins::BYTEARRAY_METHODS)),
            Some(("memoryview", attr)) => Some(("memoryview", attr, builtins::MEMORYVIEW_METHODS)),
            Some(("BaseException", attr)) => {
                Some(("BaseException", attr, &["add_note", "with_traceback"][..]))
            }
            Some(("frame", attr)) => Some(("frame", attr, &["clear"][..])),
            Some(("set", attr)) => Some((
                "set",
                attr,
                &["__reduce__", "difference", "issubset", "update"][..],
            )),
            Some(("frozenset", attr)) => Some((
                "frozenset",
                attr,
                &["__reduce__", "difference", "issubset"][..],
            )),
            Some(("list", attr)) => Some(("list", attr, builtins::LIST_METHODS)),
            Some(("range", attr)) => Some(("range", attr, range::METHODS)),
            Some((class, attr)) if let Some(layout) = stdlib::os::stat::layout(class) => {
                Some((layout.class, attr, &["__reduce__"][..]))
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
                    || matches!(
                        name,
                        "__reversed__"
                            | "__getinitargs__"
                            | "__reduce__"
                            | "__reduce_ex__"
                            | "__fspath__"
                            | "__format__"
                            | "__enter__"
                            | "__exit__"
                    ),
            });
        }
        let (owner, name) = match symbol {
            "object.__format__" => ("object", "__format__"),
            "object.__getstate__" => ("object", "__getstate__"),
            "object.__reduce__" => ("object", "__reduce__"),
            "object.__reduce_ex__" => ("object", "__reduce_ex__"),
            "int.__add__" => ("int", "__add__"),
            "bool.__repr__" => ("bool", "__repr__"),
            "int.__repr__" => ("int", "__repr__"),
            "float.__repr__" => ("float", "__repr__"),
            "int.__format__" => ("int", "__format__"),
            "str.__format__" => ("str", "__format__"),
            "str.isascii" => ("str", "isascii"),
            "str.isalpha" => ("str", "isalpha"),
            "str.isdigit" => ("str", "isdigit"),
            "str.title" => ("str", "title"),
            "str.swapcase" => ("str", "swapcase"),
            "str.islower" => ("str", "islower"),
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
            "str.__str__" => ("str", "__str__"),
            "str.__repr__" => ("str", "__repr__"),
            "bytes.__repr__" => ("bytes", "__repr__"),
            "dict.__ior__" => ("dict", "__ior__"),
            "dict.__eq__" => ("dict", "__eq__"),
            "dict.__repr__" => ("dict", "__repr__"),
            "mappingproxy.__repr__" => ("mappingproxy", "__repr__"),
            "mappingproxy.__str__" => ("mappingproxy", "__str__"),
            "collections.deque.__contains__" => (deque::CLASS, "__contains__"),
            "collections.deque.__repr__" => (deque::CLASS, "__repr__"),
            "collections.defaultdict.__repr__" => ("collections.defaultdict", "__repr__"),
            "tuple.__repr__" => ("tuple", "__repr__"),
            "set.__repr__" => ("set", "__repr__"),
            "frozenset.__repr__" => ("frozenset", "__repr__"),
            "types.SimpleNamespace.__init__" => (super::namespace::CLASS, "__init__"),
            "types.SimpleNamespace.__repr__" => (super::namespace::CLASS, "__repr__"),
            "types.SimpleNamespace.__eq__" => (super::namespace::CLASS, "__eq__"),
            "types.SimpleNamespace.__ne__" => (super::namespace::CLASS, "__ne__"),
            "tuple.__getitem__" => ("tuple", "__getitem__"),
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
                    | "__getstate__"
                    | "__reduce__"
                    | "__reduce_ex__"
                    | "index"
                    | "isascii"
                    | "isalpha"
                    | "isdigit"
                    | "islower"
                    | "istitle"
                    | "zfill"
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
            owner if stdlib::datetime::members(owner).is_some() => {
                ctx.instance_of(receiver, ctx.builtins[owner])
            }
            owner if DictIterator::class(owner).is_some() => {
                matches!(ctx.heap.get(receiver), Value::DictIterator(iter) if iter.class_name() == owner)
            }
            "generator" => matches!(ctx.heap.get(receiver), Value::Generator { .. }),
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
            class @ (stdlib::os::stat::CLASS | stdlib::os::stat::VFS_CLASS) => matches!(
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
            "method_descriptor" => ctx.builtin_type_name(receiver) == Some("method_descriptor"),
            "dict" => ctx.dict_storage(receiver).is_some(),
            "mappingproxy" => matches!(ctx.heap.get(receiver), Value::MappingProxy(_)),
            deque::CLASS => matches!(ctx.heap.get(receiver), Value::Deque(_)),
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
            "__doc__" if self.name == "__abs__" => ctx.text("abs(self)"),
            "__text_signature__" if matches!(self.name, "__neg__" | "__pos__" | "__abs__") => {
                ctx.text("($self, /)")
            }
            "__doc__" if self.name == "__add__" => ctx.text("Return self+value."),
            "__doc__" if self.name == "__radd__" => ctx.text("Return value+self."),
            "__doc__" if self.name == "__sub__" => ctx.text("Return self-value."),
            "__doc__" if self.name == "__rsub__" => ctx.text("Return value-self."),
            "__doc__" if self.name == "__mul__" => ctx.text("Return self*value."),
            "__doc__" if self.name == "__rmul__" => ctx.text("Return value*self."),
            "__doc__" if self.name == "__truediv__" => ctx.text("Return self/value."),
            "__doc__" if self.name == "__rtruediv__" => ctx.text("Return value/self."),
            "__doc__" if self.name == "__floordiv__" => ctx.text("Return self//value."),
            "__doc__" if self.name == "__rfloordiv__" => ctx.text("Return value//self."),
            "__doc__" if self.name == "__ior__" => ctx.text("Return self|=value."),
            "__text_signature__" if self.name == "__ior__" => ctx.text("($self, value, /)"),
            "__doc__" if self.name == "__iadd__" => ctx.text("Implement self+=value."),
            "__text_signature__" if matches!(self.name, "__add__" | "__radd__" | "__sub__" | "__rsub__" | "__iadd__" | "__mul__" | "__rmul__" | "__truediv__" | "__rtruediv__" | "__floordiv__" | "__rfloordiv__") => {
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
                    ("count", "__doc__") if self.owner == "bytes" => {
                        "Return the number of non-overlapping occurrences of subsection 'sub' in bytes B[start:end].\n\n  start\n    Optional start position. Default: start of the bytes.\n  end\n    Optional stop position. Default: end of the bytes."
                    }
                    ("find", "__doc__") if self.owner == "bytes" => {
                        "Return the lowest index in B where subsection 'sub' is found, such that 'sub' is contained within B[start,end].\n\n  start\n    Optional start position. Default: start of the bytes.\n  end\n    Optional stop position. Default: end of the bytes.\n\nReturn -1 on failure."
                    }
                    ("count" | "find", _) if self.owner == "bytes" => "($self, sub[, start[, end]], /)",
                    ("isascii", "__doc__") => {
                        "Return True if all characters in the string are ASCII, False otherwise.\n\nASCII characters have code points in the range U+0000-U+007F.\nEmpty string is ASCII too."
                    }
                    ("isalpha", "__doc__") => {
                        "Return True if the string is an alphabetic string, False otherwise.\n\nA string is alphabetic if all characters in the string are alphabetic and there\nis at least one character in the string."
                    }
                    ("isdigit", "__doc__") => {
                        "Return True if the string is a digit string, False otherwise.\n\nA string is a digit string if all characters in the string are digits and there\nis at least one character in the string."
                    }
                    ("islower", "__doc__") => {
                        "Return True if the string is a lowercase string, False otherwise.\n\nA string is lowercase if all cased characters in the string are lowercase and\nthere is at least one cased character in the string."
                    }
                    ("istitle", "__doc__") => {
                        "Return True if the string is a title-cased string, False otherwise.\n\nIn a title-cased string, upper- and title-case characters may only\nfollow uncased characters and lowercase characters only cased ones."
                    }
                    ("zfill", "__doc__") => {
                        "Pad a numeric string with zeros on the left, to fill a field of the given width.\n\nThe string is never truncated."
                    }
                    ("zfill", _) => "($self, width, /)",
                    ("title", "__doc__") => {
                        "Return a version of the string where each word is titlecased.\n\nMore specifically, words start with uppercased characters and all remaining\ncased characters have lower case."
                    }
                    ("swapcase", "__doc__") => {
                        "Convert uppercase characters to lowercase and lowercase characters to uppercase."
                    }
                    (
                        "isascii" | "isalpha" | "isdigit" | "islower" | "istitle" | "title"
                        | "swapcase",
                        _,
                    ) => "($self, /)",
                    ("mro", "__doc__") => "Return a type's method resolution order.",
                    ("mro", _) => "($self, /)",
                    ("append", "__doc__") => "Append object to the end of the list.",
                    ("pop", "__doc__") => {
                        "Remove and return item at index (default last).\n\nRaises IndexError if list is empty or index is out of range."
                    }
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
            if let Some(slot) = slot {
                return slot.bind(ctx, descriptor, receiver);
            }
            let owner = match arguments.get(1) {
                None | Some(&NONE) => ctx.exact_type(receiver)?,
                Some(&owner) => owner,
            };
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
