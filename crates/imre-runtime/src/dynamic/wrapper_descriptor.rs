use super::*;

/// A builtin slot descriptor shared by the class and each bound method wrapper.
#[derive(Clone, Copy)]
pub(super) struct Wrapper {
    owner: &'static str,
    name: &'static str,
}

impl Wrapper {
    pub(super) fn lookup(symbol: &str) -> Option<Self> {
        let (owner, name) = match symbol {
            "object.__hash__" => ("object", "__hash__"),
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
        Some(Self { owner, name })
    }

    pub(super) fn bind(self, ctx: &mut Context, descriptor: Id, receiver: Id) -> Result<Id> {
        if receiver == NONE {
            return Ok(descriptor);
        }
        let accepts = match self.owner {
            "object" => true,
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

    pub(super) fn attribute(self, ctx: &mut Context, name: &str) -> Option<Id> {
        Some(match name {
            "__name__" => ctx.text(self.name),
            "__qualname__" => ctx.text(format!("{}.{}", self.owner.rsplit('.').next()?, self.name)),
            "__objclass__" => ctx.builtin(self.owner),
            _ => return None,
        })
    }
}

/// Implement the descriptor's explicit __get__ with Python's receiver and owner rules.
pub(super) fn descriptor_get(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&descriptor, arguments)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "descriptor '__get__' of 'wrapper_descriptor' object needs an argument",
        ));
    };
    let slot = match ctx.heap.get(descriptor) {
        Value::Native(symbol) => Wrapper::lookup(symbol),
        _ => None,
    }
    .ok_or_else(|| {
        Error::new(
            "TypeError",
            format!(
                "descriptor '__get__' requires a 'wrapper_descriptor' object but received a '{}'",
                ctx.type_name(descriptor)
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
