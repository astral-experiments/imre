//! Non-owning handles observed by native Python code.
use super::Context;
use super::value::{Error, Id, NONE, NOT_IMPLEMENTED, Result, Value};

pub(super) const PROXY_TYPE: &str = "weakref.ProxyType";
pub(super) const PROXY_METHODS: &[&str] = &["__bytes__", "__repr__", "__reversed__"];
pub(super) const CALLABLE_PROXY_TYPE: &str = "weakref.CallableProxyType";
pub(super) const CALLABLE_PROXY_METHODS: &[&str] = &["__repr__"];
pub(super) const REFERENCE_HASH: &str = "weakref.ReferenceType.__hash__";
pub(super) const REFERENCE_TYPE: &str = "weakref.ReferenceType";
pub(super) const REFERENCE_METHODS: &[&str] = &[
    "__new__", "__init__", "__call__", "__eq__", "__ne__", "__hash__", "__repr__",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Reference,
    Proxy,
    CallableProxy,
}

impl Kind {
    pub fn class(self) -> &'static str {
        match self {
            Self::Reference => REFERENCE_TYPE,
            Self::Proxy => PROXY_TYPE,
            Self::CallableProxy => CALLABLE_PROXY_TYPE,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Reference {
    pub kind: Kind,
    pub internal: bool,
    pub target: Option<Id>,
    pub callback: Id,
    pub hash: Option<i64>,
}

impl Value {
    pub(super) fn is_weak_proxy(&self) -> bool {
        matches!(self, Self::Weak(wr) if wr.kind != Kind::Reference)
    }

    fn is_weakrefable_native_base(&self) -> bool {
        matches!(self, Self::Native(name)
            if matches!(name.as_str(), "set" | "frozenset" | super::deque::CLASS | super::stdlib::functools::PARTIAL_CLASS))
    }

    /// Return the reference payload for an exact ref, proxy, or Python reference subclass.
    pub(super) fn weak_reference(&self) -> Option<&Reference> {
        match self {
            Self::Weak(reference) => Some(reference),
            Self::Instance { weak_reference, .. } => weak_reference.as_deref(),
            _ => None,
        }
    }

    pub(super) fn weak_reference_mut(&mut self) -> Option<&mut Reference> {
        match self {
            Self::Weak(reference) => Some(reference),
            Self::Instance { weak_reference, .. } => weak_reference.as_deref_mut(),
            _ => None,
        }
    }
}

impl Reference {
    pub(super) fn call(self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "weakref() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("weakref expected 0 arguments, got {}", args.len()),
            ));
        }
        Ok(self.target.unwrap_or(NONE))
    }

    fn repr(self, ctx: &mut Context, owner: Id) -> Result<Id> {
        let kind = if self.kind == Kind::Reference {
            "weakref"
        } else {
            "weakproxy"
        };
        let Some(target) = self.target else {
            return Ok(ctx.text(format!("<{kind} at {owner:#x}; dead>")));
        };
        ctx.with_weak_target(target, |ctx| {
            let class = ctx.exact_type(target)?;
            // Resolve the optional name on the type, bypassing instance-level attribute hooks.
            let name = if self.kind != Kind::Reference {
                None
            } else if let Some(slot) = ctx.class_attr(class, "__name__") {
                Some(ctx.bind_attribute(slot, target, class)?)
            } else {
                match ctx.heap.get(target) {
                    Value::Class { name, .. } => Some(ctx.text(name.to_owned())),
                    Value::Function { .. }
                    | Value::TypeVar(_)
                    | Value::ParamSpec(_)
                    | Value::TypeVarTuple { .. } => Some(ctx.lookup_attr(target, "__name__")?),
                    _ if matches!(
                        ctx.builtin_type_name(target),
                        Some("type" | "builtin_function_or_method")
                    ) =>
                    {
                        Some(ctx.lookup_attr(target, "__name__")?)
                    }
                    _ => None,
                }
            };
            let class_name = super::stdlib::qualified_type(ctx, class)?;
            let prefix = ctx.text(format!("<{kind} at {owner:#x}; to '"));
            let mut text = ctx.concat_strings(prefix, class_name)?;
            let address = ctx.text(format!("' at {target:#x}"));
            text = ctx.concat_strings(text, address)?;
            let end = if let Some(name) = name.and_then(|name| ctx.string_storage(name)) {
                let opening = ctx.text(" (");
                text = ctx.concat_strings(text, opening)?;
                text = ctx.concat_strings(text, name)?;
                ")>"
            } else {
                ">"
            };
            let end = ctx.text(end);
            ctx.concat_strings(text, end)
        })
    }

    pub fn upgrade(self) -> Result<Id> {
        self.target.ok_or_else(|| {
            Error::new(
                "ReferenceError",
                "weakly-referenced object no longer exists",
            )
        })
    }
}

pub(super) fn export(ctx: &mut Context, name: &str) -> Option<Id> {
    let name = match name {
        "ref" | "ReferenceType" => Kind::Reference.class(),
        "ProxyType" => Kind::Proxy.class(),
        "CallableProxyType" => Kind::CallableProxy.class(),
        "proxy" | "getweakrefs" | "getweakrefcount" | "_remove_dead_weakref" => {
            return Some(ctx.builtin(&format!("_weakref.{name}")));
        }
        _ => return None,
    };
    Some(ctx.builtin(name))
}

impl Context {
    fn reference_target(
        &mut self,
        kind: Kind,
        args: &[Id],
        constructor: &str,
        subclass: Option<Id>,
    ) -> Result<Id> {
        let (target, callback) = reference_args(constructor, args)?;
        if !self.weakrefable(target) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "cannot create weak reference to '{}' object",
                    self.type_name(target)
                ),
            ));
        }
        // A proxy can be constructed without first importing its type from _weakref.
        self.builtin(kind.class());
        let wrapper = subclass.map(|class| self.instance(class, None));
        self.heap
            .new_weak_reference(target, callback, kind, wrapper)
    }

    #[inline]
    pub(super) fn weak_proxy_target(&self, obj: Id) -> Result<Id> {
        if self.heap.has_weak_values
            && let Value::Weak(wr) = self.heap.get(obj)
            && wr.kind != Kind::Reference
        {
            return wr.upgrade();
        }
        Ok(obj)
    }

    /// Hold the referent through a proxy operation that can run Python.
    pub(super) fn with_weak_target<T>(
        &mut self,
        target: Id,
        operation: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.heap.reserve_temporaries(1)?;
        let mark = self.heap.temporary_mark();
        self.heap.keep(target);
        let result = operation(self);
        self.heap.release_weak_target(mark, target);
        result
    }

    pub(super) fn class_instances_support_weakref(&self, class: Id) -> bool {
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return self.native_owns_layout_descriptor(class, "__weakref__");
        };
        if ancestors.iter().any(|&base| {
            matches!(self.heap.get(base), Value::Native(name) if super::slot_member::VARIABLE_SIZE_BASES.contains(&name.as_str()))
        }) {
            return false;
        }
        std::iter::once(&class)
            .chain(ancestors)
            .any(|&base| match self.heap.get(base) {
                Value::Class { scope, .. } => self
                    .scope_data(*scope)
                    .class_namespace
                    .as_deref()
                    .and_then(|namespace| namespace.slot_names.as_deref())
                    .is_none_or(|names| names.iter().any(|name| name == "__weakref__")),
                value => {
                    value.is_weakrefable_native_base()
                        || self.native_owns_layout_descriptor(base, "__weakref__")
                }
            })
    }

    /// A Python class introduces a descriptor only when it adds weak-reference storage.
    pub(super) fn install_weakref_descriptor(&mut self, class: Id, scope: Id) {
        if self.scope_name(scope, "__weakref__").is_some()
            || !self.class_instances_support_weakref(class)
        {
            return;
        }
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return;
        };
        if ancestors.iter().any(|&base| {
            self.class_instances_support_weakref(base)
                || self.heap.get(base).is_weakrefable_native_base()
        }) {
            return;
        }
        let descriptor = self.alloc(Value::LayoutDescriptor(class, "__weakref__"));
        self.bind(scope, "__weakref__", descriptor);
    }

    fn weakrefable(&self, value: Id) -> bool {
        match self.heap.get(value) {
            Value::Instance { class, .. } => self.class_instances_support_weakref(*class),
            Value::Raised(error) => error
                .class()
                .is_some_and(|class| self.class_instances_support_weakref(class)),
            Value::GenericAlias(_)
            | Value::Union(_)
            | Value::RegexPattern(_)
            | Value::Struct { .. }
            | Value::ParamSpec(_)
            | Value::TypeVar(_)
            | Value::TypeVarTuple { .. }
            | Value::Class { .. }
            | Value::Function { .. }
            | Value::Generator { .. }
            | Value::Coroutine { .. }
            | Value::AsyncGenerator { .. }
            | Value::Module { .. }
            | Value::Method { .. }
            | Value::Code(_)
            | Value::Set { .. }
            | Value::Deque(_)
            | Value::Array(_)
            | Value::MemoryView { .. }
            | Value::File(_)
            | Value::Partial { .. }
            | Value::Cached { .. }
            | Value::HandlerLock(_)
            | Value::PrimitiveLock(_)
            | Value::Native(_) => true,
            _ => false,
        }
    }

    fn reference_hash(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let Some((&reference, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "descriptor '__hash__' of 'weakref.ReferenceType' object needs an argument",
            ));
        };
        let wr = match self.heap.get(reference).weak_reference() {
            Some(wr) if wr.kind == Kind::Reference => *wr,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '__hash__' requires a 'weakref.ReferenceType' object but received a '{}'",
                        self.type_name(reference)
                    ),
                ));
            }
        };
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "wrapper __hash__() takes no keyword arguments",
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", rest.len()),
            ));
        }
        let hash = self.weak_reference_hash(reference, wr)?;
        Ok(self.int(hash))
    }

    pub(super) fn weak_reference_hash(&mut self, reference: Id, wr: Reference) -> Result<i64> {
        if let Some(hash) = wr.hash {
            return Ok(hash);
        }
        let target = wr
            .target
            .ok_or_else(|| Error::new("TypeError", "weak object has gone away"))?;
        self.heap.keep(target);
        let hash = self.dictionary_hash(target)?;
        self.heap
            .get_mut(reference)
            .weak_reference_mut()
            .unwrap()
            .hash = Some(hash);
        Ok(hash)
    }
}

fn reference_args(name: &str, args: &[Id]) -> Result<(Id, Id)> {
    match args {
        [target] => Ok((*target, NONE)),
        [target, callback] => Ok((*target, *callback)),
        [] => Err(Error::new(
            "TypeError",
            format!("{name} expected at least 1 argument, got 0"),
        )),
        _ => Err(Error::new(
            "TypeError",
            format!("{name} expected at most 2 arguments, got {}", args.len()),
        )),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    function: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let error = |s| Error::new("TypeError", s);
    match function {
        REFERENCE_HASH => ctx.reference_hash(args, kwargs),
        "weakref.ProxyType.__bytes__" | "weakref.ProxyType.__reversed__" => {
            let method = function.rsplit('.').next().unwrap();
            let proxy =
                super::builtins::noargs_method_receiver(ctx, PROXY_TYPE, method, args, kwargs)?;
            let target = ctx.weak_proxy_target(proxy)?;
            ctx.with_weak_target(target, |ctx| {
                let hook = ctx.lookup_attr(target, method)?;
                ctx.call(hook, Vec::new(), Vec::new())
            })
        }
        "weakref.ReferenceType.__new__" => {
            let Some((&class, rest)) = args.split_first() else {
                return Err(error(
                    "weakref.ReferenceType.__new__(): not enough arguments",
                ));
            };
            let base = ctx.builtin(REFERENCE_TYPE);
            if !ctx.subclass(class, base) {
                if ctx.builtin_type_name(class) != Some("type") {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "weakref.ReferenceType.__new__(X): X is not a type object ({})",
                            ctx.type_name(class)
                        ),
                    ));
                }
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "weakref.ReferenceType.__new__({0}): {0} is not a subtype of weakref.ReferenceType",
                        ctx.class_name(class)
                    ),
                ));
            }
            ctx.reference_target(
                Kind::Reference,
                rest,
                "__new__",
                (class != base).then_some(class),
            )
        }
        "weakref.ReferenceType.__call__"
        | "weakref.ReferenceType.__repr__"
        | "weakref.ProxyType.__repr__"
        | "weakref.CallableProxyType.__repr__"
        | "weakref.ReferenceType.__init__"
        | "weakref.ReferenceType.__eq__"
        | "weakref.ReferenceType.__ne__" => {
            let (class, name) = function.rsplit_once('.').unwrap();
            let Some((&owner, rest)) = args.split_first() else {
                return Err(Error::new(
                    "TypeError",
                    format!("descriptor '{name}' of '{class}' object needs an argument"),
                ));
            };
            let Some(reference) = ctx
                .heap
                .get(owner)
                .weak_reference()
                .copied()
                .filter(|r| r.kind.class() == class)
            else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '{name}' requires a '{class}' object but received a '{}'",
                        ctx.type_name(owner)
                    ),
                ));
            };
            if name == "__call__" {
                return reference.call(rest, kwargs);
            }
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    match name {
                        "__init__" => "ref() takes no keyword arguments".to_owned(),
                        _ => format!("wrapper {name}() takes no keyword arguments"),
                    },
                ));
            }
            match name {
                "__repr__" => {
                    if !rest.is_empty() {
                        return Err(Error::new(
                            "TypeError",
                            format!("expected 0 arguments, got {}", rest.len()),
                        ));
                    }
                    reference.repr(ctx, owner)
                }
                "__init__" => {
                    reference_args(name, rest)?;
                    Ok(NONE)
                }
                _ => {
                    let [other] = rest else {
                        return Err(Error::new(
                            "TypeError",
                            format!("expected 1 argument, got {}", rest.len()),
                        ));
                    };
                    let Some(right) = ctx
                        .heap
                        .get(*other)
                        .weak_reference()
                        .copied()
                        .filter(|r| r.kind == Kind::Reference)
                    else {
                        return Ok(NOT_IMPLEMENTED);
                    };
                    if let (Some(a), Some(b)) = (reference.target, right.target) {
                        ctx.binary(if name == "__eq__" { "==" } else { "!=" }, a, b)
                    } else {
                        Ok(if (owner == *other) == (name == "__eq__") {
                            super::TRUE
                        } else {
                            super::FALSE
                        })
                    }
                }
            }
        }
        "weakref.ReferenceType" | "_weakref.proxy" => {
            let proxy = function == "_weakref.proxy";
            let name = if proxy { "proxy" } else { "__new__" };
            if !kwargs.is_empty() {
                return Err(error(if proxy {
                    "_weakref.proxy() takes no keyword arguments"
                } else {
                    "ref() takes no keyword arguments"
                }));
            }
            let kind = if !proxy {
                Kind::Reference
            } else if args.first().is_some_and(|&target| ctx.callable(target)) {
                Kind::CallableProxy
            } else {
                Kind::Proxy
            };
            ctx.reference_target(kind, args, name, None)
        }
        "weakref.ProxyType" | "weakref.CallableProxyType" => Err(Error::new(
            "TypeError",
            format!("cannot create '{function}' instances"),
        )),
        "_weakref.getweakrefs" | "_weakref.getweakrefcount" => {
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("{function}() takes no keyword arguments"),
                ));
            }
            let [target] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{function}() takes exactly one argument ({} given)",
                        args.len()
                    ),
                ));
            };
            let refs = ctx.heap.python_weak_references(*target);
            if function.ends_with("count") {
                Ok(ctx.int(refs.len() as i64))
            } else {
                let mut snapshot = Vec::new();
                snapshot
                    .try_reserve_exact(refs.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                snapshot.extend_from_slice(refs);
                Ok(ctx.alloc(Value::List(snapshot)))
            }
        }
        "_weakref._remove_dead_weakref" => {
            if !kwargs.is_empty() {
                return Err(error(
                    "_weakref._remove_dead_weakref() takes no keyword arguments",
                ));
            }
            let [dict, key] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "_remove_dead_weakref expected 2 arguments, got {}",
                        args.len()
                    ),
                ));
            };
            let Some(dict) = ctx.dict_storage(*dict) else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "_remove_dead_weakref() argument 1 must be dict, not {}",
                        ctx.type_name(*dict)
                    ),
                ));
            };
            if let Some(index) = ctx.find_dictionary_key(dict, *key, None)?.1 {
                let value = ctx.dict_entries(dict).expect("dict storage")[index].1;
                let Some(reference) = ctx.heap.get(value).weak_reference() else {
                    return Err(error("not a weakref"));
                };
                if reference.target.is_none() {
                    // A second key lookup could call Python and replace the dead reference.
                    ctx.heap.remove_table_entry(dict, index)?;
                    ctx.finalize_removed_dict_value(value);
                }
            }
            Ok(NONE)
        }
        _ => unreachable!("registered weak reference operation"),
    }
}
