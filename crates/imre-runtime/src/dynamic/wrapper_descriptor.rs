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
            Some(("match", attr)) => {
                Some((value::REGEX_MATCH_TYPE, attr, stdlib::regex::MATCH_METHODS))
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
            Some(("functools.KeyWrapper", attr)) => Some((
                "functools.KeyWrapper",
                attr,
                &[
                    "__lt__", "__le__", "__eq__", "__ne__", "__gt__", "__ge__", "__call__",
                ][..],
            )),
            Some(("float", attr)) => Some(("float", attr, &["as_integer_ratio"][..])),
            Some(("bytes", attr)) => Some(("bytes", attr, &["endswith"][..])),
            Some(("bytearray", attr)) => Some(("bytearray", attr, builtins::BYTEARRAY_METHODS)),
            Some(("BaseException", attr)) => Some(("BaseException", attr, &["with_traceback"][..])),
            Some(("frame", attr)) => Some(("frame", attr, &["clear"][..])),
            Some(("set", attr)) => Some(("set", attr, &["issubset"][..])),
            Some(("frozenset", attr)) => Some(("frozenset", attr, &["issubset"][..])),
            Some(("list", attr)) => Some(("list", attr, builtins::LIST_METHODS)),
            Some(("range", attr)) => Some(("range", attr, range::METHODS)),
            Some((stdlib::os::stat::CLASS, attr)) => {
                Some((stdlib::os::stat::CLASS, attr, &["__reduce__"][..]))
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
                        "__reversed__" | "__reduce__" | "__fspath__" | "__enter__" | "__exit__"
                    ),
            });
        }
        let (owner, name) = match symbol {
            "object.__format__" => ("object", "__format__"),
            "int.__format__" => ("int", "__format__"),
            "str.__format__" => ("str", "__format__"),
            "str.isascii" => ("str", "isascii"),
            "str.isalpha" => ("str", "isalpha"),
            "str.isdigit" => ("str", "isdigit"),
            "str.islower" => ("str", "islower"),
            "str.zfill" => ("str", "zfill"),
            "float.__format__" => ("float", "__format__"),
            "object.__hash__" => ("object", "__hash__"),
            "object.__repr__" => ("object", "__repr__"),
            "function.__call__" => ("function", "__call__"),
            "classmethod.__get__" => ("classmethod", "__get__"),
            "method_descriptor.__get__" => ("method_descriptor", "__get__"),
            "str.__repr__" => ("str", "__repr__"),
            "bytes.__repr__" => ("bytes", "__repr__"),
            "dict.__repr__" => ("dict", "__repr__"),
            "collections.deque.__contains__" => (deque::CLASS, "__contains__"),
            "collections.defaultdict.__repr__" => ("collections.defaultdict", "__repr__"),
            "tuple.__repr__" => ("tuple", "__repr__"),
            "set.__repr__" => ("set", "__repr__"),
            "frozenset.__repr__" => ("frozenset", "__repr__"),
            "types.SimpleNamespace.__init__" => (super::namespace::CLASS, "__init__"),
            "types.SimpleNamespace.__repr__" => (super::namespace::CLASS, "__repr__"),
            "types.SimpleNamespace.__eq__" => (super::namespace::CLASS, "__eq__"),
            "types.SimpleNamespace.__ne__" => (super::namespace::CLASS, "__ne__"),
            "tuple.__getitem__" => ("tuple", "__getitem__"),
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
                "__format__" | "isascii" | "isalpha" | "isdigit" | "islower" | "zfill"
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

    pub(super) fn bind(self, ctx: &mut Context, descriptor: Id, receiver: Id) -> Result<Id> {
        if receiver == NONE {
            return Ok(descriptor);
        }
        let accepts = match self.owner {
            class if crate::objects::HashState::is_provider_class(class) => {
                ctx.type_name(receiver) == class
            }
            "object" => true,
            "function" => matches!(ctx.heap.get(receiver), Value::Function { .. }),
            "classmethod" => matches!(ctx.heap.get(receiver), Value::ClassMethod(_)),
            "functools.KeyWrapper" => matches!(ctx.heap.get(receiver), Value::KeyWrapper { .. }),
            super::namespace::CLASS => {
                ctx.instance_of(receiver, ctx.classes[super::namespace::CLASS])
            }
            "frame" => matches!(ctx.heap.get(receiver), Value::PythonFrame { .. }),
            "BaseException" => matches!(ctx.heap.get(receiver), Value::Raised(_)),
            stdlib::os::stat::CLASS => matches!(
                ctx.heap.get(receiver),
                Value::NamedTuple {
                    class: Some(stdlib::os::stat::CLASS),
                    ..
                }
            ),
            stdlib::os::scandir::ENTRY => matches!(ctx.heap.get(receiver), Value::DirEntry(_)),
            stdlib::os::scandir::ITER => matches!(ctx.heap.get(receiver), Value::Scandir(_)),
            value::REGEX_MATCH_TYPE => matches!(ctx.heap.get(receiver), Value::RegexMatch(_)),
            "range" => matches!(ctx.heap.get(receiver), Value::Range(_)),
            "slice" => matches!(ctx.heap.get(receiver), Value::Slice(_)),
            "method_descriptor" => ctx.builtin_type_name(receiver) == Some("method_descriptor"),
            "dict" => ctx.dict_storage(receiver).is_some(),
            deque::CLASS => matches!(ctx.heap.get(receiver), Value::Deque(_)),
            "collections.defaultdict" => ctx.defaultdict_factory(receiver).is_some(),
            "int" | "float" | "str" | "bytes" | "bytearray" | "list" | "tuple" | "set"
            | "frozenset" => ctx.builtin_slot_accepts(receiver, self.owner),
            _ => matches!(ctx.heap.get(receiver), Value::ThreadLocal(_)),
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
            "__name__" => ctx.text(self.name),
            "__qualname__" => {
                let owner = if self.method
                    && let Some(receiver) = receiver
                {
                    match ctx.heap.get(receiver) {
                        Value::NamedTuple { class: Some(stdlib::os::stat::CLASS), .. } => "stat_result",
                        Value::Instance { class, .. } => ctx
                            .class_attr(*class, "__qualname__")
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
            "__doc__" if self.name == "__call__" => ctx.text("Call self as a function."),
            "__text_signature__" if self.name == "__call__" => {
                ctx.text("($self, /, *args, **kwargs)")
            }
            "__doc__" if self.name == "__contains__" => ctx.text("Return bool(key in self)."),
            "__text_signature__" if self.name == "__contains__" => ctx.text("($self, key, /)"),
            "__doc__" if self.name == "__add__" => ctx.text("Return self+value."),
            "__doc__" if self.name == "__iadd__" => ctx.text("Implement self+=value."),
            "__text_signature__" if matches!(self.name, "__add__" | "__iadd__") => {
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
                    ("__reduce__", "__doc__") if self.owner == stdlib::os::stat::CLASS => return Some(NONE),
                    ("__reduce__", _) if self.owner == stdlib::os::stat::CLASS => "($self, /)",
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
                    ("isascii" | "isalpha" | "isdigit" | "islower", _) => "($self, /)",
                    ("zfill", "__doc__") => {
                        "Pad a numeric string with zeros on the left, to fill a field of the given width.\n\nThe string is never truncated."
                    }
                    ("zfill", _) => "($self, width, /)",
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
        Value::ClassMethod(_) if class == "classmethod" => None,
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
