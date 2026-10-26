//! Runtime type parameters and class-base aliases used by imported Python modules.

use std::collections::{HashMap, VecDeque};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

#[derive(Clone)]
pub(super) struct TypeVariable {
    pub name: Id,
    pub module: Id,
    pub bound: Id,
    pub constraints: Id,
    pub covariant: bool,
    pub contravariant: bool,
    pub infer_variance: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AliasKind {
    Builtin,
    Generic,
    Callable,
    AbcCallable,
}

impl AliasKind {
    pub(super) fn class(self) -> &'static str {
        match self {
            Self::Builtin => "types.GenericAlias",
            Self::Generic => "typing._GenericAlias",
            Self::Callable => "typing._CallableGenericAlias",
            Self::AbcCallable => "collections.abc._CallableGenericAlias",
        }
    }

    pub(super) fn is_typing_alias(self) -> bool {
        matches!(self, Self::Generic | Self::Callable)
    }

    pub(super) fn type_name(self) -> &'static str {
        self.class().rsplit_once('.').unwrap().1
    }
}

impl Alias {
    pub(super) fn repr(&self, ctx: &Context) -> String {
        let type_repr = |id| match ctx.heap.get(id) {
            Value::Ellipsis => "...".to_owned(),
            Value::Native(name) => name.clone(),
            Value::Class { name, scope, .. } => format!("{}.{name}", ctx.scope_data(*scope).module),
            _ => ctx.repr(id),
        };
        let Value::Tuple(args) = ctx.heap.get(self.args) else {
            unreachable!()
        };
        let mut args: Vec<_> = args.iter().map(|&id| type_repr(id)).collect();
        let origin = if self.kind == AliasKind::Callable {
            "typing.Callable".to_owned()
        } else {
            type_repr(self.origin)
        };
        let params = if matches!(self.kind, AliasKind::Callable | AliasKind::AbcCallable)
            && args.first().is_none_or(|arg| arg != "...")
        {
            let result = args.pop().expect("a callable has a result");
            format!("[{}], {result}", args.join(", "))
        } else if args.is_empty() {
            "()".to_owned()
        } else {
            args.join(", ")
        };
        format!("{origin}[{params}]")
    }
}

#[derive(Clone)]
pub(super) struct Alias {
    pub origin: Id,
    pub args: Id,
    pub parameters: Id,
    pub kind: AliasKind,
}

#[derive(Default)]
pub(super) struct Registry {
    aliases: VecDeque<Id>,
    overloads: HashMap<(String, String), Vec<(u32, Id)>>,
}

impl Registry {
    pub(super) fn roots(&self) -> impl Iterator<Item = &Id> {
        self.aliases
            .iter()
            .chain(self.overloads.values().flatten().map(|(_, value)| value))
    }
}

pub(super) fn named_class(ctx: &mut Context, module: &str, name: &str) -> Id {
    let key = format!("{module}.{name}");
    if let Some(&class) = ctx.classes.get(&key) {
        return class;
    }
    let scope = ctx.scope(module, None);
    let module = ctx.text(module);
    ctx.bind(scope, "__module__", module);
    let class = ctx
        .allocate_class(name.to_owned(), scope, Vec::new())
        .expect("object has a linearization");
    ctx.classes.insert(key, class);
    class
}

fn variables(ctx: &Context, args: &[Id]) -> Vec<Id> {
    let mut vars = Vec::new();
    for &arg in args {
        let members = match ctx.heap.get(arg) {
            Value::TypeVar(_) => std::slice::from_ref(&arg),
            Value::GenericAlias(alias) => match ctx.heap.get(alias.parameters) {
                Value::Tuple(members) => members.as_slice(),
                _ => unreachable!(),
            },
            _ => continue,
        };
        for &param in members {
            if !vars.contains(&param) {
                vars.push(param);
            }
        }
    }
    vars
}

fn alias(ctx: &mut Context, kind: AliasKind, origin: Id, args: Vec<Id>) -> Id {
    if kind.is_typing_alias()
        && let Some(pos) = ctx.typing.aliases.iter().position(|&id| {
            let Value::GenericAlias(alias) = ctx.heap.get(id) else {
                unreachable!()
            };
            let Value::Tuple(previous) = ctx.heap.get(alias.args) else {
                unreachable!()
            };
            alias.kind == kind
                && alias.origin == origin
                && previous.len() == args.len()
                && previous
                    .iter()
                    .zip(&args)
                    .all(|(&left, &right)| ctx.equal(left, right))
        })
    {
        let id = ctx.typing.aliases.remove(pos).unwrap();
        ctx.typing.aliases.push_back(id);
        return id;
    }
    let args = ctx.alloc(Value::Tuple(args));
    let value = allocate_alias(ctx, kind, origin, args);
    if kind.is_typing_alias() {
        if ctx.typing.aliases.len() == 128 {
            ctx.typing.aliases.pop_front();
        }
        ctx.typing.aliases.push_back(value);
    }
    value
}

fn allocate_alias(ctx: &mut Context, kind: AliasKind, origin: Id, args: Id) -> Id {
    let Value::Tuple(items) = ctx.heap.get(args) else {
        unreachable!()
    };
    let params = variables(ctx, items);
    let parameters = ctx.alloc(Value::Tuple(params));
    ctx.alloc(Value::GenericAlias(Alias {
        origin,
        args,
        parameters,
        kind,
    }))
}

/// PEP 585 keeps a tuple supplied to the constructor as the alias's actual arguments.
pub(super) fn builtin_alias(ctx: &mut Context, origin: Id, args: Id) -> Id {
    let args = if matches!(ctx.heap.get(args), Value::Tuple(_)) {
        args
    } else {
        ctx.alloc(Value::Tuple(vec![args]))
    };
    allocate_alias(ctx, AliasKind::Builtin, origin, args)
}

/// Apply the subscription protocol for a supported type. Ordinary instance indexing
/// continues through the object runtime.
pub(super) fn subscribe(ctx: &mut Context, target: Id, index: Id) -> Option<Result<Id>> {
    if matches!(ctx.heap.get(target), Value::Class { .. })
        && ctx.class_attr(target, "__class_getitem__").is_some()
    {
        return Some(
            ctx.lookup_attr(target, "__class_getitem__")
                .and_then(|method| ctx.call(method, vec![index], vec![])),
        );
    }
    let kind = match ctx.heap.get(target) {
        Value::Class { .. } if ctx.classes.get("typing.Generic") == Some(&target) => {
            AliasKind::Generic
        }
        Value::Class { .. } if ctx.classes.get("collections.abc.Callable") == Some(&target) => {
            AliasKind::AbcCallable
        }
        Value::Native(n) if n == "typing.Callable" => AliasKind::Callable,
        Value::Native(n) if matches!(n.as_str(), "dict" | "list" | "tuple" | "set") => {
            AliasKind::Builtin
        }
        Value::Class { ancestors, .. } => {
            let mut supported = None;
            for &base in ancestors {
                if ctx.classes.get("typing.Generic") == Some(&base) {
                    supported = Some(AliasKind::Generic);
                    break;
                }
                if let Value::Native(name) = ctx.heap.get(base) {
                    supported = match name.as_str() {
                        "dict" | "list" | "tuple" | "set" => Some(AliasKind::Builtin),
                        _ => continue,
                    };
                    break;
                }
            }
            supported?
        }
        _ => return None,
    };
    if kind == AliasKind::Builtin {
        return Some(Ok(builtin_alias(ctx, target, index)));
    }
    let args = match ctx.heap.get(index) {
        Value::Tuple(items) => items.clone(),
        _ => vec![index],
    };
    Some((|| {
        if kind == AliasKind::Generic {
            if ctx.classes.get("typing.Generic") == Some(&target) {
                if args.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        "Parameter list to Generic[...] cannot be empty",
                    ));
                }
                if args
                    .iter()
                    .any(|&id| !matches!(ctx.heap.get(id), Value::TypeVar(_)))
                {
                    return Err(Error::new(
                        "TypeError",
                        "Parameters to Generic[...] must all be type variables or parameter specification variables.",
                    ));
                }
                if variables(ctx, &args).len() != args.len() {
                    return Err(Error::new(
                        "TypeError",
                        "Parameters to Generic[...] must all be unique",
                    ));
                }
            } else {
                let params = ctx.lookup_attr(target, "__parameters__")?;
                let required = ctx.items(params)?.len();
                if args.len() != required {
                    let which = if args.len() < required { "few" } else { "many" };
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "Too {which} arguments for {}; actual {}, expected {required}",
                            ctx.display(target),
                            args.len()
                        ),
                    ));
                }
            }
        }
        if matches!(kind, AliasKind::Callable | AliasKind::AbcCallable) {
            let [parameters, result] = args.as_slice() else {
                return Err(Error::new(
                    "TypeError",
                    "Callable must be used as Callable[[arg, ...], result].",
                ));
            };
            let actual = match ctx.heap.get(*parameters) {
                Value::Ellipsis => args,
                Value::List(items) | Value::Tuple(items) => {
                    let mut flat = items.clone();
                    flat.push(*result);
                    flat
                }
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        "Expected a list of types, an ellipsis, ParamSpec, or Concatenate",
                    ));
                }
            };
            let origin = named_class(ctx, "collections.abc", "Callable");
            return Ok(alias(ctx, kind, origin, actual));
        }
        Ok(alias(ctx, kind, target, args))
    })())
}

/// Resolve PEP 560 hooks against one shared tuple of the original bases, before the class body.
pub(super) fn resolve_bases(ctx: &mut Context, bases: Id) -> Result<(Option<Id>, Vec<Id>)> {
    let raw = ctx.items(bases)?;
    let mut resolved = Vec::with_capacity(raw.len());
    let mut original = None;
    for &base in &raw {
        if matches!(ctx.heap.get(base), Value::Class { .. } | Value::Native(_)) {
            resolved.push(base);
            continue;
        }
        let method = match ctx.lookup_attr(base, "__mro_entries__") {
            Ok(method) => method,
            Err(error) if error.kind == "AttributeError" => {
                resolved.push(base);
                continue;
            }
            Err(error) => return Err(error),
        };
        let tuple = *original.get_or_insert_with(|| ctx.alloc(Value::Tuple(raw.clone())));
        let entries = ctx.call(method, vec![tuple], vec![])?;
        let Value::Tuple(entries) = ctx.heap.get(entries) else {
            return Err(Error::new(
                "TypeError",
                "__mro_entries__ must return a tuple",
            ));
        };
        resolved.extend_from_slice(entries);
    }
    Ok((original, resolved))
}

fn alias_bases(ctx: &mut Context, id: Id, bases: Id) -> Result<Id> {
    if matches!(ctx.heap.get(id), Value::TypeVar(_)) {
        return Err(Error::new(
            "TypeError",
            "Cannot subclass an instance of TypeVar",
        ));
    }
    let Value::GenericAlias(alias) = ctx.heap.get(id) else {
        unreachable!()
    };
    let (origin, kind) = (alias.origin, alias.kind);
    let mut result = vec![origin];
    if kind.is_typing_alias() {
        let generic = named_class(ctx, "typing", "Generic");
        let bases = ctx.items(bases)?;
        let i = bases
            .iter()
            .position(|base| *base == id)
            .ok_or_else(|| Error::new("ValueError", "tuple.index(x): x not in tuple"))?;
        let later_generic = bases[i + 1..].iter().any(|&base| {
            matches!(ctx.heap.get(base), Value::GenericAlias(a) if a.kind.is_typing_alias() && base != id)
        });
        if origin == generic
            && (later_generic
                || ctx
                    .classes
                    .get("typing.Protocol")
                    .is_some_and(|protocol| bases.contains(protocol)))
        {
            result.clear();
        }
        if kind == AliasKind::Callable {
            if bases.contains(&origin) {
                result.clear();
            }
            if !later_generic
                && !bases[i + 1..]
                    .iter()
                    .any(|&base| ctx.subclass(base, generic))
            {
                result.push(generic);
            }
        }
    }
    Ok(ctx.alloc(Value::Tuple(result)))
}

pub(super) fn class_parameters(
    ctx: &mut Context,
    scope: Id,
    original: Option<Id>,
    bases: &[Id],
) -> Result<()> {
    if let Some(original) = original {
        ctx.bind(scope, "__orig_bases__", original);
    }
    let Some(&generic) = ctx.classes.get("typing.Generic") else {
        return Ok(());
    };
    if !bases.iter().any(|&base| ctx.subclass(base, generic)) {
        return Ok(());
    }
    let raw = match original {
        Some(tuple) => ctx.items(tuple)?,
        None => bases.to_vec(),
    };
    if raw.contains(&generic) {
        return Err(Error::new("TypeError", "Cannot inherit from plain Generic"));
    }
    let vars = variables(ctx, &raw);
    let mut explicit = None;
    for id in raw {
        if let Value::GenericAlias(a) = ctx.heap.get(id)
            && a.origin == generic
            && explicit.replace(a.parameters).is_some()
        {
            return Err(Error::new(
                "TypeError",
                "Cannot inherit from Generic[...] multiple times.",
            ));
        }
    }
    let params = if let Some(params) = explicit {
        let declared = ctx.items(params)?;
        let missing = vars
            .iter()
            .filter(|v| !declared.contains(v))
            .map(|&v| ctx.display(v))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let args = declared.iter().map(|&v| ctx.display(v)).collect::<Vec<_>>();
            return Err(Error::new(
                "TypeError",
                format!(
                    "Some type variables ({}) are not listed in Generic[{}]",
                    missing.join(", "),
                    args.join(", ")
                ),
            ));
        }
        params
    } else {
        ctx.alloc(Value::Tuple(vars))
    };
    ctx.bind(scope, "__parameters__", params);
    Ok(())
}

pub(super) fn attribute(ctx: &mut Context, value: Id, name: &str) -> Option<Result<Id>> {
    let attr = match ctx.heap.get(value) {
        Value::TypeVar(_) | Value::GenericAlias(_) if name == "__mro_entries__" => {
            let callable = ctx.builtin("typing.alias_mro_entries");
            ctx.alloc(Value::Method {
                callable,
                receiver: value,
                explicit: false,
            })
        }
        Value::Native(class)
            if name == "__module__"
                && (class.starts_with("typing.")
                    || class == "types.GenericAlias"
                    || class == "collections.abc._CallableGenericAlias") =>
        {
            let module = class.rsplit_once('.').unwrap().0.to_owned();
            ctx.text(module)
        }
        Value::TypeVar(var) => match name {
            "__name__" => var.name,
            "__module__" => var.module,
            "__bound__" => var.bound,
            "__constraints__" => var.constraints,
            "__infer_variance__" => {
                if var.infer_variance {
                    TRUE
                } else {
                    FALSE
                }
            }
            "__covariant__" => {
                if var.covariant {
                    TRUE
                } else {
                    FALSE
                }
            }
            "__contravariant__" => {
                if var.contravariant {
                    TRUE
                } else {
                    FALSE
                }
            }
            _ => return None,
        },
        Value::GenericAlias(a) => match name {
            "__args__" => a.args,
            "__origin__" => a.origin,
            "__parameters__" => a.parameters,
            _ => return None,
        },
        Value::Class { scope, .. } if name == "__bases__" || name == "__mro__" => {
            let (bases, mro) = ctx.scope_data(*scope).class_lineage?;
            if name == "__mro__" { mro } else { bases }
        }
        _ => return None,
    };
    Some(Ok(attr))
}

fn typevar(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut name = args.first().copied();
    let mut bound = NONE;
    let mut covariant = false;
    let mut contravariant = false;
    let mut infer_variance = false;
    for (key, val) in kwargs {
        match key.as_str() {
            "name" if name.is_none() => name = Some(*val),
            "bound" => bound = *val,
            "covariant" => covariant = ctx.truth(*val)?,
            "contravariant" => contravariant = ctx.truth(*val)?,
            "infer_variance" => infer_variance = ctx.truth(*val)?,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("invalid TypeVar keyword {key:?}"),
                ));
            }
        }
    }
    let name = name.ok_or_else(|| {
        Error::new(
            "TypeError",
            "typevar() missing required argument 'name' (pos 1)",
        )
    })?;
    if ctx.str_value(name).is_err() {
        return Err(Error::new(
            "TypeError",
            format!(
                "typevar() argument 'name' must be str, not {}",
                ctx.type_name(name)
            ),
        ));
    }
    let constraints = args.get(1..).unwrap_or_default();
    if covariant && contravariant {
        return Err(Error::new(
            "ValueError",
            "Bivariant types are not supported.",
        ));
    }
    if constraints.len() == 1 {
        return Err(Error::new(
            "TypeError",
            "A single constraint is not allowed",
        ));
    }
    if infer_variance && (covariant || contravariant) {
        return Err(Error::new(
            "ValueError",
            "Variance cannot be specified with infer_variance.",
        ));
    }
    if !constraints.is_empty() && bound != NONE {
        return Err(Error::new(
            "TypeError",
            "Constraints cannot be combined with bound=...",
        ));
    }
    let module = ctx.scope_data(ctx.heap.current_scope()).module.clone();
    let module = ctx.text(module);
    let constraints = ctx.alloc(Value::Tuple(constraints.to_vec()));
    Ok(ctx.alloc(Value::TypeVar(TypeVariable {
        name,
        module,
        bound,
        constraints,
        covariant,
        contravariant,
        infer_variance,
    })))
}

pub(super) fn call(
    ctx: &mut Context,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if operation == "TypeVar" {
        return typevar(ctx, args, kwargs);
    }
    if operation == "cast" {
        let [typ, value] = super::builtins::bind("cast()", args, kwargs, ["typ", "val"])?;
        if let (Some(_), Some(value)) = (typ, value) {
            return Ok(value);
        }
        return Err(Error::new(
            "TypeError",
            "cast() requires a type and a value",
        ));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("unexpected keyword for typing.{operation}"),
        ));
    }
    match (operation, args) {
        ("Any", _) => Err(Error::new("TypeError", "Any cannot be instantiated")),
        ("get_origin", [v]) => Ok(match ctx.heap.get(*v) {
            Value::GenericAlias(a) => a.origin,
            Value::Class { .. } if ctx.classes.get("typing.Generic") == Some(v) => *v,
            Value::Native(name) if name == "typing.Callable" => {
                named_class(ctx, "collections.abc", "Callable")
            }
            _ => NONE,
        }),
        ("get_args", [v]) => Ok(match ctx.heap.get(*v) {
            Value::GenericAlias(a)
                if matches!(a.kind, AliasKind::Callable | AliasKind::AbcCallable) =>
            {
                let Value::Tuple(args) = ctx.heap.get(a.args) else {
                    unreachable!()
                };
                let (result, params) = args.split_last().expect("a callable has a result");
                if matches!(params, [param] if matches!(ctx.heap.get(*param), Value::Ellipsis)) {
                    a.args
                } else {
                    let result = *result;
                    let params = ctx.alloc(Value::List(params.to_vec()));
                    ctx.alloc(Value::Tuple(vec![params, result]))
                }
            }
            Value::GenericAlias(a) => a.args,
            _ => ctx.alloc(Value::Tuple(vec![])),
        }),
        ("alias_mro_entries", [id, bases]) => alias_bases(ctx, *id, *bases),
        ("overload", [v]) => {
            if let Some((key, line)) = ctx.overload_key(*v) {
                let group = ctx.typing.overloads.entry(key).or_default();
                crate::objects::register_overload(group, line, *v);
            }
            Ok(ctx.builtin("typing._overload_dummy"))
        }
        ("get_overloads", [v]) => {
            let functions = ctx
                .overload_key(*v)
                .and_then(|(key, _)| ctx.typing.overloads.get(&key))
                .into_iter()
                .flatten()
                .map(|&(_, f)| f)
                .collect();
            Ok(ctx.alloc(Value::List(functions)))
        }
        ("clear_overloads", []) => {
            ctx.typing.overloads.clear();
            Ok(NONE)
        }
        ("_overload_dummy", _) => Err(Error::new(
            "NotImplementedError",
            crate::objects::OVERLOAD_DUMMY_ERROR,
        )),
        _ => Err(Error::new(
            "TypeError",
            format!("invalid call to typing.{operation}"),
        )),
    }
}

impl Context {
    pub(super) fn python_instance_check(&self, value: Id, class: Id) -> Result<bool> {
        match self.heap.get(class) {
            Value::Tuple(classes) => {
                for &class in classes {
                    if self.python_instance_check(value, class)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Value::TypeVar(_) => Err(Error::new(
                "TypeError",
                "isinstance() arg 2 must be a type, a tuple of types, or a union",
            )),
            Value::GenericAlias(alias) => Err(Error::new(
                "TypeError",
                if alias.kind.is_typing_alias() {
                    "Subscripted generics cannot be used with class and instance checks"
                } else {
                    "isinstance() argument 2 cannot be a parameterized generic"
                },
            )),
            Value::Native(name) if name == "typing.Any" => Err(Error::new(
                "TypeError",
                "typing.Any cannot be used with isinstance()",
            )),
            Value::Native(name) if name == "typing.Callable" => Ok(self.callable(value)),
            _ => Ok(self.instance_of(value, class)),
        }
    }

    /// Preserve the actual direct bases as well as the immutable C3 tuple exposed by type.
    pub(super) fn allocate_class(
        &mut self,
        name: String,
        scope: Id,
        mut bases: Vec<Id>,
    ) -> Result<Id> {
        for &base in &bases {
            if let Value::Native(name) = self.heap.get(base)
                && matches!(name.as_str(), "typing.TypeVar" | "slice")
            {
                return Err(Error::new(
                    "TypeError",
                    format!("type '{name}' is not an acceptable base type"),
                ));
            }
        }
        if bases.is_empty() {
            bases.push(self.builtin("object"));
        }
        let ancestors = self.linearized_bases(&bases)?;
        let mut mro = Vec::with_capacity(ancestors.len() + 1);
        mro.push(NONE);
        mro.extend_from_slice(&ancestors);
        let class = self.alloc(Value::Class {
            name,
            scope,
            ancestors,
        });
        mro[0] = class;
        let bases = self.alloc(Value::Tuple(bases));
        let mro = self.alloc(Value::Tuple(mro));
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        fields.class_lineage = Some((bases, mro));
        Ok(class)
    }

    fn overload_key(&self, function: Id) -> Option<((String, String), u32)> {
        let value = match self.heap.get(function) {
            Value::Method { callable, .. } | Value::StaticMethod { callable, .. } => {
                self.heap.get(*callable)
            }
            value => value,
        };
        let Value::Function { fields, code, .. } = value else {
            return None;
        };
        Some((
            (
                self.str_value(fields.module).ok()?.to_owned(),
                self.str_value(fields.qualname).ok()?.to_owned(),
            ),
            self.code_object(*code).first_line,
        ))
    }
}
