//! Non-owning handles observed by native Python code.
use super::Context;
use super::value::{Error, Id, NONE, Result, Value};

pub(super) const REFERENCE_HASH: &str = "weakref.ReferenceType.__hash__";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Reference,
    Proxy,
    CallableProxy,
}

impl Kind {
    pub fn class(self) -> &'static str {
        match self {
            Self::Reference => "weakref.ReferenceType",
            Self::Proxy => "weakref.ProxyType",
            Self::CallableProxy => "weakref.CallableProxyType",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Reference {
    pub kind: Kind,
    pub target: Option<Id>,
    pub callback: Id,
    pub hash: Option<i64>,
}

impl Reference {
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

    fn class_instances_support_weakref(&self, class: Id) -> bool {
        let Value::Class { ancestors, .. } = self.heap.get(class) else {
            return false;
        };
        if ancestors.iter().any(|&base| {
            matches!(self.heap.get(base), Value::Native(name) if matches!(name.as_str(), "int" | "tuple" | "bytes"))
        }) {
            return false;
        }
        std::iter::once(&class)
            .chain(ancestors)
            .any(|&base| match self.heap.get(base) {
                Value::Native(name) => matches!(name.as_str(), "set" | "frozenset"),
                Value::Class { scope, .. } => match self.scope_name(*scope, "__slots__") {
                    None => true,
                    Some(slots) => match self.heap.get(slots) {
                        Value::Str(name) => name == "__weakref__",
                        Value::Tuple(names) | Value::List(names) => names.iter().any(|&name| {
                            self.str_value(name).is_ok_and(|name| name == "__weakref__")
                        }),
                        _ => false,
                    },
                },
                _ => false,
            })
    }

    fn weakrefable(&self, value: Id) -> bool {
        match self.heap.get(value) {
            Value::Instance { class, .. } => self.class_instances_support_weakref(*class),
            Value::Raised(error) => error
                .class
                .is_some_and(|class| self.class_instances_support_weakref(class)),
            Value::Class { .. }
            | Value::Function { .. }
            | Value::Module { .. }
            | Value::Method { .. }
            | Value::Code(_)
            | Value::Set { .. }
            | Value::Partial { .. }
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
        let wr = match self.heap.get(reference) {
            Value::Weak(wr) if wr.kind == Kind::Reference => *wr,
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
        let Value::Weak(wr) = self.heap.get_mut(reference) else {
            unreachable!()
        };
        wr.hash = Some(hash);
        Ok(hash)
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
            let [target, rest @ ..] = args else {
                return Err(Error::new(
                    "TypeError",
                    format!("{name} expected at least 1 argument, got 0"),
                ));
            };
            if rest.len() > 1 {
                return Err(Error::new(
                    "TypeError",
                    format!("{name} expected at most 2 arguments, got {}", args.len()),
                ));
            }
            if !ctx.weakrefable(*target) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "cannot create weak reference to '{}' object",
                        ctx.type_name(*target)
                    ),
                ));
            }
            let kind = if !proxy {
                Kind::Reference
            } else if ctx.callable(*target) {
                Kind::CallableProxy
            } else {
                Kind::Proxy
            };
            Ok(ctx
                .heap
                .new_weak_reference(*target, rest.first().copied().unwrap_or(NONE), kind))
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
            let refs = ctx.heap.weak_references(*target);
            if function.ends_with("count") {
                Ok(ctx.int(refs.len() as i64))
            } else {
                Ok(ctx.alloc(Value::List(refs.to_vec())))
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
            let entries = ctx.dict_entries(dict).expect("dict storage");
            if let Some(&(_, value)) = entries.iter().find(|(stored, _)| ctx.equal(*stored, *key)) {
                let Value::Weak(reference) = ctx.heap.get(value) else {
                    return Err(error("not a weakref"));
                };
                if reference.target.is_none() {
                    ctx.delete_item(dict, *key)?;
                }
            }
            Ok(NONE)
        }
        _ => unreachable!("registered weak reference operation"),
    }
}
