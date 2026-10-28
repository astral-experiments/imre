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
        let method = match symbol {
            "list.append" => Some("append"),
            "list.pop" => Some("pop"),
            _ => None,
        };
        if let Some(name) = method {
            return Some(Self {
                owner: "list",
                name,
                method: true,
            });
        }
        let (owner, name) = match symbol {
            "object.__hash__" => ("object", "__hash__"),
            "slice.__hash__" => ("slice", "__hash__"),
            "method_descriptor.__get__" => ("method_descriptor", "__get__"),
            "str.__repr__" => ("str", "__repr__"),
            "bytes.__repr__" => ("bytes", "__repr__"),
            "bytearray.__repr__" => ("bytearray", "__repr__"),
            "dict.__repr__" => ("dict", "__repr__"),
            "collections.defaultdict.__repr__" => ("collections.defaultdict", "__repr__"),
            "list.__repr__" => ("list", "__repr__"),
            "tuple.__repr__" => ("tuple", "__repr__"),
            "set.__repr__" => ("set", "__repr__"),
            "frozenset.__repr__" => ("frozenset", "__repr__"),
            "list.__init__" => ("list", "__init__"),
            "tuple.__getitem__" => ("tuple", "__getitem__"),
            "_thread._local.__getattribute__" => (thread_local::CLASS, "__getattribute__"),
            "_thread._local.__setattr__" => (thread_local::CLASS, "__setattr__"),
            "_thread._local.__delattr__" => (thread_local::CLASS, "__delattr__"),
            _ => return None,
        };
        Some(Self {
            owner,
            name,
            method: false,
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

    pub(super) fn method_repr(self) -> Option<String> {
        self.method
            .then(|| format!("<method '{}' of '{}' objects>", self.name, self.owner))
    }

    pub(super) fn bind(self, ctx: &mut Context, descriptor: Id, receiver: Id) -> Result<Id> {
        if receiver == NONE {
            return Ok(descriptor);
        }
        let accepts = match self.owner {
            "object" => true,
            "slice" => matches!(ctx.heap.get(receiver), Value::Slice(_)),
            "method_descriptor" => ctx.builtin_type_name(receiver) == Some("method_descriptor"),
            "dict" => ctx.dict_storage(receiver).is_some(),
            "collections.defaultdict" => ctx.defaultdict_factory(receiver).is_some(),
            "str" | "bytes" | "bytearray" | "list" | "tuple" | "set" | "frozenset" => {
                ctx.builtin_slot_accepts(receiver, self.owner)
            }
            _ => matches!(ctx.heap.get(receiver), Value::ThreadLocal(_)),
        };
        if !accepts {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                    self.name,
                    self.owner,
                    ctx.type_name(receiver)
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
            "__doc__" | "__text_signature__" if self.method => {
                let value = match (self.name, name) {
                    ("append", "__doc__") => "Append object to the end of the list.",
                    ("pop", "__doc__") => {
                        "Remove and return item at index (default last).\n\nRaises IndexError if list is empty or index is out of range."
                    }
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
        Value::Native(symbol) => BuiltinDescriptor::lookup(symbol),
        _ => None,
    }
    .filter(|slot| slot.class() == class)
    .ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "descriptor '__get__' requires a '{class}' object but received a '{}'",
                ctx.builtin_type_name(descriptor)
                    .unwrap_or_else(|| ctx.type_name(descriptor))
            ),
        )
    })?;
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
        [receiver] | [receiver, _] => slot.bind(ctx, descriptor, receiver),
        _ => Err(Error::new(
            "TypeError",
            format!(
                "__get__ expected at most 2 arguments, got {}",
                arguments.len()
            ),
        )),
    }
}
