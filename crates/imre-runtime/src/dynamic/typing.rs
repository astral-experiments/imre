//! Runtime type parameters and class-base aliases used by imported Python modules.

use std::collections::HashMap;

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
    /// The frozen set of (value, type) pairs, or NONE for unhashable arguments.
    Literal(Id),
}

impl AliasKind {
    pub(super) fn class(self) -> &'static str {
        match self {
            Self::Builtin => "types.GenericAlias",
            Self::Generic => "typing._GenericAlias",
            Self::Callable => "typing._CallableGenericAlias",
            Self::AbcCallable => "collections.abc._CallableGenericAlias",
            Self::Literal(_) => "typing._LiteralGenericAlias",
        }
    }

    pub(super) fn is_typing_alias(self) -> bool {
        matches!(self, Self::Generic | Self::Callable)
    }

    pub(super) fn type_name(self) -> &'static str {
        self.class().rsplit_once('.').unwrap().1
    }

    /// Parameterized aliases reject both public class checks, even as a union member.
    pub(super) fn class_check_error(self, operation: &str) -> Error {
        if self.is_typing_alias() || matches!(self, Self::Literal(_)) {
            Error::new(
                "TypeError",
                "Subscripted generics cannot be used with class and instance checks",
            )
        } else {
            Error::new(
                "TypeError",
                format!("{operation}() argument 2 cannot be a parameterized generic"),
            )
        }
    }
}

/// Recognize the original special form even when its public module name is rebound.
fn is_special_form(ctx: &Context, id: Id) -> bool {
    ctx.module_value("typing", "_SpecialForm")
        .is_some_and(|class| ctx.instance_of(id, class))
}

/// Append a type argument's special spelling if it has one.
pub(super) fn append_type_label(ctx: &Context, id: Id, out: &mut String) -> Result<bool> {
    use super::operators::append_rendered_text;

    let label = match ctx.heap.get(id) {
        Value::Ellipsis => "...",
        Value::Native(name) => name,
        Value::Class { name, scope, .. } => {
            let class_name = |attr| {
                ctx.own_class_attr(id, attr)
                    .and_then(|id| ctx.str_value(id).ok())
            };
            let module = class_name("__module__").unwrap_or(&ctx.scope_data(*scope).module);
            if module != "builtins" {
                append_rendered_text(out, module)?;
                append_rendered_text(out, ".")?;
            }
            class_name("__qualname__").unwrap_or(name)
        }
        Value::Instance { .. } if is_special_form(ctx, id) => {
            let Some(name) = ctx
                .raw_instance_field(id, "_name")
                .and_then(|field| ctx.str_value(field).ok())
            else {
                return Ok(false);
            };
            append_rendered_text(out, "typing.")?;
            name
        }
        _ => return Ok(false),
    };
    append_rendered_text(out, label)?;
    Ok(true)
}

impl Alias {
    /// Literal compares its type-sensitive members without regard to argument order.
    pub(super) fn comparison_parts(&self, other: &Self) -> Option<(Id, Id)> {
        if self.origin != other.origin {
            return None;
        }
        match (self.kind, other.kind) {
            (AliasKind::Literal(a), AliasKind::Literal(b)) => Some((a, b)),
            (a, b) if a == b => Some((self.args, other.args)),
            _ => None,
        }
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
    alias_caches: [Option<Id>; 2],
    overloads: HashMap<(String, String), Vec<(u32, Id)>>,
}

impl Registry {
    pub(super) fn roots(&self) -> impl Iterator<Item = &Id> {
        self.alias_caches
            .iter()
            .flatten()
            .chain(self.overloads.values().flatten().map(|(_, value)| value))
    }
}

pub(super) fn named_class(ctx: &mut Context, module: &str, name: &str) -> Id {
    let key = format!("{module}.{name}");
    if let Some(&class) = ctx.classes.get(&key) {
        return class;
    }
    let scope = ctx.scope(module, None);
    let bases = if key == "typing.Protocol" {
        let generic = named_class(ctx, "typing", "Generic");
        ctx.bind(scope, "_is_protocol", TRUE);
        ctx.bind(scope, "_is_runtime_protocol", FALSE);
        let parameters = ctx.alloc(Value::Tuple(Vec::new()));
        ctx.bind(scope, "__parameters__", parameters);
        let abstract_methods = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen: true,
        });
        ctx.bind(scope, "__abstractmethods__", abstract_methods);
        vec![generic]
    } else {
        Vec::new()
    };
    let module = ctx.text(module);
    ctx.bind(scope, "__module__", module);
    let class = ctx
        .allocate_class(name.to_owned(), scope, bases)
        .expect("native typing classes have a single base");
    ctx.classes.insert(key, class);
    class
}

pub(super) fn variables(ctx: &Context, args: &[Id]) -> Vec<Id> {
    let mut vars = Vec::new();
    for &arg in args {
        let members = match ctx.heap.get(arg) {
            Value::TypeVar(_) => std::slice::from_ref(&arg),
            Value::GenericAlias(Alias { parameters, .. })
            | Value::Union(super::union::Union { parameters, .. }) => {
                match ctx.heap.get(*parameters) {
                    Value::Tuple(members) => members.as_slice(),
                    _ => unreachable!(),
                }
            }
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

/// Typing retries the constructor without caching when building or comparing its key raises TypeError.
fn alias(ctx: &mut Context, kind: AliasKind, origin: Id, args: Vec<Id>) -> Result<Id> {
    let args = ctx.alloc(Value::Tuple(args));
    let (index, constructor) = match kind {
        AliasKind::Generic => (0, "typing._GenericAlias"),
        AliasKind::Callable => (1, "typing._CallableGenericAlias"),
        _ => return Ok(allocate_alias(ctx, kind, origin, args)),
    };
    let cached = if let Some(id) = ctx.typing.alias_caches[index] {
        id
    } else {
        let function = ctx.builtin(constructor);
        let cache = ctx.cached_function(function);
        ctx.typing.alias_caches[index] = Some(cache);
        cache
    };
    let result = ctx.call(cached, vec![origin, args], Vec::new());
    if let Err(error) = &result {
        let type_error = ctx.builtin("TypeError");
        if ctx.raised_error_matches(error, type_error) {
            return Ok(allocate_alias(ctx, kind, origin, args));
        }
    }
    result
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

/// Build the same set of (value, type) pairs used for literal deduplication and hashing.
fn literal_keys(ctx: &mut Context, values: &[Id]) -> Result<(Id, Vec<Id>)> {
    let members = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: true,
    });
    ctx.heap.keep(members);
    let mut unique = Vec::new();
    for &value in values {
        let class = ctx.exact_type(value)?;
        let key = ctx.alloc(Value::Tuple(vec![value, class]));
        ctx.set_dictionary_key(members, key, NONE, None, false)?;
        if let Value::Set { entries, .. } = ctx.heap.get(members)
            && entries.len() != unique.len()
        {
            unique.push(value);
        }
    }
    Ok((members, unique))
}

/// Rebuild the comparison or hash keys because a user-defined __hash__ can change.
pub(super) fn literal_members(ctx: &mut Context, args: Id) -> Result<Id> {
    let values = ctx.items(args)?;
    Ok(literal_keys(ctx, &values)?.0)
}

pub(super) fn checked_alias_parts(
    ctx: &mut Context,
    left: Id,
    right: Id,
) -> Result<Option<(Id, Id)>> {
    let (Value::GenericAlias(a), Value::GenericAlias(b)) =
        (ctx.heap.get(left), ctx.heap.get(right))
    else {
        unreachable!()
    };
    if matches!(
        (a.kind, b.kind),
        (AliasKind::Literal(_), AliasKind::Literal(_))
    ) {
        let (left_args, right_args) = (a.args, b.args);
        return Ok(Some((
            literal_members(ctx, left_args)?,
            literal_members(ctx, right_args)?,
        )));
    }
    Ok(a.comparison_parts(b))
}

fn literal_alias(ctx: &mut Context, origin: Id, parameters: Id) -> Result<Id> {
    let mut flat = Vec::new();
    for value in ctx.items(parameters)? {
        if let Value::GenericAlias(Alias {
            kind: AliasKind::Literal(_),
            args,
            ..
        }) = ctx.heap.get(value)
        {
            flat.extend(ctx.items(*args)?);
        } else {
            flat.push(value);
        }
    }
    let members = match literal_keys(ctx, &flat) {
        Ok((members, unique)) => {
            flat = unique;
            members
        }
        Err(error) if error.kind == "TypeError" => NONE,
        Err(error) => return Err(error),
    };
    let args = ctx.alloc(Value::Tuple(flat));
    Ok(allocate_alias(
        ctx,
        AliasKind::Literal(members),
        origin,
        args,
    ))
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
    if let Value::GenericAlias(alias) = ctx.heap.get(target)
        && matches!(alias.kind, AliasKind::Literal(_))
        && matches!(ctx.heap.get(alias.parameters), Value::Tuple(params) if params.is_empty())
    {
        return Some(ctx.checked_format(target, true).and_then(|name| {
            Err(Error::new(
                "TypeError",
                format!("{name} is not a generic class"),
            ))
        }));
    }
    if matches!(ctx.heap.get(target), Value::Class { .. })
        && ctx.class_attr(target, "__class_getitem__").is_some()
    {
        return Some(
            ctx.lookup_attr(target, "__class_getitem__")
                .and_then(|method| ctx.call(method, vec![index], vec![])),
        );
    }
    let kind = match ctx.heap.get(target) {
        Value::Class { .. } if super::mapping_abc::is_generic_abc(ctx, target) => {
            AliasKind::Builtin
        }
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
            if ctx.classes.get("typing.Generic") == Some(&target)
                || ctx.classes.get("typing.Protocol") == Some(&target)
            {
                let Value::Class { name, .. } = ctx.heap.get(target) else {
                    unreachable!()
                };
                if args.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        format!("Parameter list to {name}[...] cannot be empty"),
                    ));
                }
                if args
                    .iter()
                    .any(|&id| !matches!(ctx.heap.get(id), Value::TypeVar(_)))
                {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "Parameters to {name}[...] must all be type variables or parameter specification variables."
                        ),
                    ));
                }
                if variables(ctx, &args).len() != args.len() {
                    return Err(Error::new(
                        "TypeError",
                        format!("Parameters to {name}[...] must all be unique"),
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
            return alias(ctx, kind, origin, actual);
        }
        alias(ctx, kind, target, args)
    })())
}

/// Resolve PEP 560 hooks against one shared tuple of the original bases, before the class body.
pub(super) fn resolve_bases(ctx: &mut Context, bases: Id) -> Result<(Option<Id>, Vec<Id>)> {
    let raw = ctx.items(bases)?;
    let mut resolved = Vec::with_capacity(raw.len());
    let mut original = None;
    for &base in &raw {
        let resolved_type = match ctx.heap.get(base) {
            Value::Class { .. } => true,
            Value::Native(name) => name != "typing.Callable",
            _ => false,
        };
        if resolved_type {
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
    if matches!(ctx.heap.get(id), Value::Native(name) if name == "typing.Callable") {
        let base_tuple = bases;
        let bases = ctx.items(base_tuple)?;
        let callable = named_class(ctx, "collections.abc", "Callable");
        let generic = named_class(ctx, "typing", "Generic");
        let mut result = Vec::new();
        let mut has_callable = false;
        for &base in &bases {
            if base == callable {
                has_callable = true;
                break;
            }
            let equal = ctx.binary("==", base, callable)?;
            if ctx.truth(equal)? {
                has_callable = true;
                break;
            }
        }
        if !has_callable {
            result.push(callable);
        }
        let mut i = None;
        for (index, &base) in bases.iter().enumerate() {
            if base == id {
                i = Some(index);
                break;
            }
            let equal = ctx.binary("==", base, id)?;
            if ctx.truth(equal)? {
                i = Some(index);
                break;
            }
        }
        let i = i.ok_or_else(|| Error::new("ValueError", "tuple.index(x): x not in tuple"))?;
        let mut later_generic = false;
        for &base in &bases[i + 1..] {
            if matches!(ctx.heap.get(base), Value::Native(name) if name == "typing.Callable")
                || matches!(ctx.heap.get(base), Value::GenericAlias(a) if a.kind.is_typing_alias())
            {
                later_generic = true;
                break;
            }
            if matches!(ctx.heap.get(base), Value::Class { .. } | Value::Native(_)) {
                if ctx.subclass(base, generic) {
                    later_generic = true;
                    break;
                }
                continue;
            }
            let method = match ctx.lookup_attr(base, "__mro_entries__") {
                Ok(method) => method,
                Err(error) if ctx.is_attribute_error(&error) => continue,
                Err(error) => return Err(error),
            };
            let entries = ctx.call(method, vec![base_tuple], vec![])?;
            if let Value::Tuple(entries) = ctx.heap.get(entries)
                && entries.iter().any(|&entry| {
                    matches!(ctx.heap.get(entry), Value::Class { .. } | Value::Native(_))
                        && ctx.subclass(entry, generic)
                })
            {
                later_generic = true;
                break;
            }
        }
        if !later_generic {
            result.push(generic);
        }
        return Ok(ctx.alloc(Value::Tuple(result)));
    }
    let Value::GenericAlias(alias) = ctx.heap.get(id) else {
        unreachable!()
    };
    let (origin, kind) = (alias.origin, alias.kind);
    if matches!(kind, AliasKind::Literal(_)) || is_special_form(ctx, origin) {
        return Err(Error::new(
            "TypeError",
            format!("Cannot subclass {}", ctx.checked_format(id, true)?),
        ));
    }
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
        Value::Native(class) if class == "typing.Callable" && name == "__mro_entries__" => {
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
            let class = ctx.scope_data(*scope).class_namespace.as_ref()?;
            if name == "__mro__" {
                class.mro
            } else {
                class.bases
            }
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
    let module = ctx.heap.caller_module();
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
        ("_GenericAlias" | "_CallableGenericAlias", [origin, parameters]) => {
            let args = if matches!(ctx.heap.get(*parameters), Value::Tuple(_)) {
                *parameters
            } else {
                ctx.alloc(Value::Tuple(vec![*parameters]))
            };
            let kind = if operation == "_CallableGenericAlias" {
                AliasKind::Callable
            } else {
                AliasKind::Generic
            };
            Ok(allocate_alias(ctx, kind, *origin, args))
        }
        ("_LiteralGenericAlias", [origin, parameters]) => literal_alias(ctx, *origin, *parameters),
        ("_LiteralGenericAlias" | "_GenericAlias", _) => Err(Error::new(
            "TypeError",
            "_GenericAlias.__init__() requires an origin and arguments",
        )),
        ("get_origin", [v]) => Ok(match ctx.heap.get(*v) {
            Value::GenericAlias(a) => a.origin,
            Value::Union(_) => super::union::class(ctx),
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
            Value::Union(u) => u.args,
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
    /// Distinguish a declared protocol from Protocol itself and its concrete descendants.
    pub(super) fn is_protocol_declaration(&self, class: Id) -> bool {
        self.classes
            .get("typing.Protocol")
            .is_some_and(|&protocol| {
                class != protocol
                    && self.subclass(class, protocol)
                    && self.class_attr(class, "_is_protocol") == Some(TRUE)
            })
    }

    /// Reject direct runtime checks on protocol declarations before nominal matching.
    pub(super) fn require_checkable_protocol(&self, class: Id) -> Result<()> {
        if self.is_protocol_declaration(class) {
            return Err(Error::new(
                "TypeError",
                "Instance and class checks can only be used with @runtime_checkable protocols",
            ));
        }
        Ok(())
    }

    pub(super) fn python_instance_check(&mut self, value: Id, class: Id) -> Result<bool> {
        match self.heap.get(class) {
            Value::Union(union) => self.python_instance_check(value, union.args),
            Value::Tuple(classes) => {
                for class in classes.clone() {
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
            Value::GenericAlias(alias) => Err(alias.kind.class_check_error("isinstance")),
            Value::Native(name) if name == "typing.Any" => Err(Error::new(
                "TypeError",
                "typing.Any cannot be used with isinstance()",
            )),
            Value::Native(name) if name == "typing.Callable" => Ok(self.callable(value)),
            _ => {
                if let Some(check) = self.bound_slot(class, "__instancecheck__")? {
                    let result = self.call(check, vec![value], Vec::new())?;
                    return self.truth(result);
                }
                self.require_checkable_protocol(class)?;
                if self.has_abc_subclass_hook(class) {
                    let actual = self.exact_type(value)?;
                    if actual == class {
                        return Ok(true);
                    }
                    if let Some(result) = self.abc_hook_result(actual, class)? {
                        return Ok(result);
                    }
                }
                Ok(self.instance_of(value, class))
            }
        }
    }

    /// Freeze the declared field names or inherit the first named-tuple layout in the MRO.
    fn namedtuple_layout(
        &mut self,
        name: &str,
        bases: &[Id],
        ancestors: &[Id],
        scope: Id,
    ) -> Result<Option<std::rc::Rc<[String]>>> {
        if bases.iter().any(|&base| {
            matches!(self.heap.get(base), Value::Native(name) if name == "typing.NamedTuple")
        }) {
            let fields = self.scope_data(scope).names.get("__annotations__")
                .and_then(|&annotations| self.dict_entries(annotations))
                .unwrap_or_default();
            let named_fields: std::rc::Rc<[String]> = fields
                .iter()
                .filter_map(|&(key, _)| self.str_value(key).ok().map(str::to_owned))
                .collect();
            let mut defaults = Vec::new();
            for field in named_fields.iter() {
                if !ruff_python_stdlib::identifiers::is_identifier(field) {
                    return Err(Error::new("ValueError", format!("invalid named tuple field {field:?}")));
                }
                if let Some(value) = self.scope_name(scope, field) {
                    defaults.push(value);
                } else if !defaults.is_empty() {
                    return Err(Error::new("TypeError", format!("non-default field {field} cannot follow default fields")));
                }
            }
            let defaults = self.alloc(Value::Tuple(defaults));
            super::namedtuple::bind_new(self, scope, name, &named_fields, Some(defaults))?;
            if self.scope_name(scope, "__repr__").is_none() {
                let fields = named_fields.iter().map(|field| self.text(field)).collect();
                let fields = self.alloc(Value::Tuple(fields));
                let name = self.text(name);
                super::namedtuple::bind_repr(self, scope, "typing", name, fields)?;
            }
            super::namedtuple::bind_fields(self, scope, &named_fields);
            return Ok(Some(named_fields));
        }
        Ok(ancestors
            .iter()
            .find_map(|&base| match self.heap.get(base) {
                Value::Class {
                    namedtuple_fields, ..
                } => namedtuple_fields.clone(),
                _ => None,
            }))
    }

    fn declared_slot_names(&mut self, scope: Id) -> Result<Option<std::rc::Rc<[String]>>> {
        let Some(slots) = self.scope_name(scope, "__slots__") else {
            return Ok(None);
        };
        if let Ok(name) = self.str_value(slots) {
            return Ok(Some([name.to_owned()].into()));
        }
        let iterator = self.iterator(slots)?;
        self.heap.keep(iterator);
        let mut items = Vec::new();
        while let Some(slot) = self.next_item(iterator)? {
            self.heap.keep(slot);
            items.push(slot);
        }
        let mut names = Vec::with_capacity(items.len());
        for slot in items {
            let name = self.str_value(slot).map_err(|_| {
                Error::new(
                    "TypeError",
                    format!(
                        "__slots__ items must be strings, not '{}'",
                        self.type_name(slot)
                    ),
                )
            })?;
            names.push(name.to_owned());
        }
        Ok(Some(names.into()))
    }

    /// Preserve the actual direct bases as well as the immutable C3 tuple exposed by type.
    pub(super) fn allocate_class(
        &mut self,
        name: String,
        scope: Id,
        mut bases: Vec<Id>,
    ) -> Result<Id> {
        for &base in &bases {
            if matches!(self.heap.get(base), Value::Native(name) if name == super::thread_local::CLASS)
            {
                return Err(Error::new(
                    "NotImplementedError",
                    "thread-local subclasses require per-thread initialization",
                ));
            }
            if let Value::Native(name) = self.heap.get(base)
                && matches!(
                    name.as_str(),
                    "typing.TypeVar"
                        | "slice"
                        | "NoneType"
                        | "NotImplementedType"
                        | "ellipsis"
                        | super::namedtuple::GETTER
                        | "member_descriptor"
                )
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
        let slot_names = self.declared_slot_names(scope)?;
        let namedtuple_fields = self.namedtuple_layout(&name, &bases, &ancestors, scope)?;
        let class = self.alloc(Value::Class {
            namedtuple_fields,
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
        // A class-body __eq__ suppresses inherited hashing at type creation.
        if fields.names.contains_key("__eq__") && !fields.names.contains_key("__hash__") {
            fields.names.insert("__hash__".to_owned(), NONE);
        }
        let namespace = fields.class_namespace.get_or_insert_with(Default::default);
        namespace.slot_names = slot_names;
        namespace.bases = bases;
        namespace.mro = mro;
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
