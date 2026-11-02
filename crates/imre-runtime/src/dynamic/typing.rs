//! Runtime type parameters and class-base aliases used by imported Python modules.

use std::collections::HashMap;

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

/// Namespace type calls validate native layouts before using ordinary member slots.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ClassLayout {
    Specialized,
    OrdinarySlots,
    TypeNamespace,
}

/// Statements retain a callable evaluator. The constructor receives an already evaluated value.
#[derive(Clone, Copy)]
pub(super) enum AliasDefinition {
    Lazy {
        parameters: Id,
        evaluator: Id,
        cached: Option<Id>,
    },
    Eager {
        value: Id,
        module: Id,
    },
}

#[derive(Clone)]
pub(super) struct TypeVariable {
    pub name: Id,
    pub module: Id,
    pub bound: Id,
    pub constraints: Id,
    pub default: Id,
    pub covariant: bool,
    pub contravariant: bool,
    pub infer_variance: bool,
    pub declared: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AliasKind {
    Builtin,
    Generic,
    Callable,
    AbcCallable,
    Concatenate,
    Unpack,
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
            Self::Concatenate => "typing._ConcatenateGenericAlias",
            Self::Unpack => "typing._UnpackGenericAlias",
        }
    }

    pub(super) fn is_typing_alias(self) -> bool {
        matches!(
            self,
            Self::Generic | Self::Callable | Self::Unpack | Self::Concatenate
        )
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
    pub(super) fn metadata(&self) -> impl Iterator<Item = (&'static str, Id)> {
        [
            ("__origin__", self.origin),
            ("__args__", self.args),
            ("__parameters__", self.parameters),
        ]
        .into_iter()
    }

    /// Literal compares its type-sensitive members without regard to argument order.
    pub(super) fn comparison_parts(&self, other: &Self) -> Option<(Id, Id)> {
        if self.origin != other.origin || self.unpacked != other.unpacked {
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
    pub name: Option<Id>,
    pub instantiable: bool,
    pub unpacked: bool,
}

impl Alias {
    pub(super) fn is_builtin_alias(&self) -> bool {
        matches!(self.kind, AliasKind::Builtin | AliasKind::AbcCallable)
    }

    /// GenericAlias iteration yields a new base GenericAlias, including for an already-starred alias.
    pub(super) fn unpacked_copy(&self) -> Self {
        Self {
            kind: AliasKind::Builtin,
            unpacked: true,
            name: None,
            ..self.clone()
        }
    }

    /// Instance fields shared by parameterized typing aliases.
    pub(super) fn instance_fields(&self) -> [(&'static str, Id); 5] {
        [
            ("__args__", self.args),
            ("__origin__", self.origin),
            ("__parameters__", self.parameters),
            ("_name", self.name.unwrap_or(NONE)),
            ("_inst", if self.instantiable { TRUE } else { FALSE }),
        ]
    }
}

/// Walk nested Literal origins without consuming native stack frames.
pub(super) fn literal_origin(ctx: &Context, mut origin: Id) -> Result<(Id, usize)> {
    let mut depth = 0;
    while let Value::GenericAlias(alias) = ctx.heap.get(origin)
        && matches!(alias.kind, AliasKind::Literal(_))
    {
        depth += 1;
        ctx.check_call_depth(depth)?;
        origin = alias.origin;
    }
    Ok((origin, depth))
}

/// Typing aliases delegate non-dunder attribute names to the origin.
pub(super) fn forwards_to_origin(name: &str) -> bool {
    !(name.starts_with("__") && name.ends_with("__"))
}

/// Resolve ordinary attributes of parameterized typing generics on their origin.
pub(super) fn generic_attr_target(ctx: &Context, mut obj: Id, name: &str, writing: bool) -> Result<Id> {
    let mut depth = 0;
    while let Value::GenericAlias(alias) = ctx.heap.get(obj)
        && alias.kind == AliasKind::Generic
        && match name {
            "_name" | "_inst" => false,
            "_nparams" | "_defaults" => !writing,
            "copy_with" | "_determine_new_args" | "_make_substitution" => writing,
            _ => forwards_to_origin(name),
        }
    {
        depth += 1;
        ctx.check_call_depth(depth)?;
        obj = alias.origin;
    }
    Ok(obj)
}

#[derive(Default)]
pub(super) struct Registry {
    alias_caches: [Option<Id>; 2],
    pub(super) callable_factory: Option<Id>,
    no_default: Option<Id>,
    overloads: HashMap<(String, String), Vec<(u32, Id)>>,
}

impl Registry {
    pub(super) fn roots(&self) -> impl Iterator<Item = &Id> {
        self.alias_caches
            .iter()
            .flatten()
            .chain(self.callable_factory.iter())
            .chain(self.no_default.iter())
            .chain(self.overloads.values().flatten().map(|(_, value)| value))
    }
}

pub(super) fn no_default(ctx: &mut Context) -> Id {
    if let Some(value) = ctx.typing.no_default {
        return value;
    }
    let value = ctx.alloc(Value::NoDefault);
    ctx.typing.no_default = Some(value);
    value
}

/// A pack iterates over one Unpack alias, exactly as an explicit Unpack[Ts] subscription.
pub(super) fn unpack_parameter(ctx: &mut Context, pack: Id) -> Result<Id> {
    let module = ctx.module("typing")?;
    let unpack = ctx.lookup_attr(module, "Unpack")?;
    ctx.get_item(unpack, pack)
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
        let members = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        ctx.bind(scope, "__protocol_attrs__", members);
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
    if key == "typing.Generic" {
        // The native base provides neither instance dictionary nor weak-reference storage.
        let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_namespace.as_mut().unwrap().slot_names = Some([].into());
    }
    ctx.classes.insert(key, class);
    class
}

pub(super) fn variables(ctx: &Context, args: &[Id]) -> Vec<Id> {
    let mut vars = Vec::new();
    for &arg in args {
        let members = match ctx.heap.get(arg) {
            Value::TypeVar(_) | Value::ParamSpec(_) | Value::TypeVarTuple { .. } => {
                std::slice::from_ref(&arg)
            }
            Value::GenericAlias(Alias { parameters, .. })
            | Value::TypeAlias {
                definition: AliasDefinition::Lazy { parameters, .. },
                ..
            }
            | Value::Union(super::union::Union { parameters, .. }) => {
                // Like _Py_make_parameters, ignore non-tuple metadata supplied by Python.
                match ctx.heap.get(ctx.scalar_value(*parameters)) {
                    Value::Tuple(members) | Value::NamedTuple { values: members, .. } => {
                        members.as_slice()
                    }
                    _ => continue,
                }
            }
            Value::Instance { class, .. }
                if ctx.module_value("typing", "_AnnotatedAlias") == Some(*class) =>
            {
                match ctx
                    .raw_instance_field(arg, "__parameters__")
                    .map(|id| ctx.heap.get(id))
                {
                    Some(Value::Tuple(members)) => members.as_slice(),
                    _ => continue,
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

/// CPython's typing aliases and Generic subclasses share the source typing collector.
fn collect_type_parameters(ctx: &mut Context, args: Id, enforce_order: bool) -> Result<Id> {
    if let Some(collector) = ctx.module_value("typing", "_collect_type_parameters") {
        return ctx.call(
            collector,
            vec![args],
            vec![(
                "enforce_default_ordering".into(),
                super::builtins::boolean(enforce_order),
            )],
        );
    }
    // Native bootstrap can create aliases before loading the source typing module.
    Ok(native_parameters(ctx, args))
}

fn native_parameters(ctx: &mut Context, args: Id) -> Id {
    let Value::Tuple(items) = ctx.heap.get(args) else {
        unreachable!()
    };
    let parameters = variables(ctx, items);
    ctx.alloc(Value::Tuple(parameters))
}

/// Typing retries the constructor without caching when building or comparing its key raises TypeError.
fn alias(ctx: &mut Context, kind: AliasKind, origin: Id, args: Vec<Id>) -> Result<Id> {
    let args = ctx.alloc(Value::Tuple(args));
    let (index, make_alias, arguments) = match kind {
        AliasKind::Generic => (0, ctx.builtin("typing._GenericAlias"), vec![origin, args]),
        AliasKind::Callable => {
            let make_alias = ctx.typing.callable_factory.ok_or_else(|| {
                Error::new("RuntimeError", "typing.Callable is not initialized")
            })?;
            let constructor = ctx.builtin("typing._CallableGenericAlias");
            (1, make_alias, vec![constructor, origin, args])
        }
        _ => {
            let parameters = native_parameters(ctx, args);
            return Ok(allocate_alias(
                ctx, kind, origin, args, parameters, None, true,
            ));
        }
    };
    let cached = if let Some(id) = ctx.typing.alias_caches[index] {
        id
    } else {
        let cache = ctx.cached_function(make_alias);
        ctx.typing.alias_caches[index] = Some(cache);
        cache
    };
    let result = ctx.call(cached, arguments.clone(), Vec::new());
    if let Err(error) = &result {
        let type_error = ctx.builtin("TypeError");
        if ctx.raised_error_matches(error, type_error) {
            return ctx.call(make_alias, arguments, Vec::new());
        }
    }
    result
}

fn allocate_alias(
    ctx: &mut Context,
    kind: AliasKind,
    origin: Id,
    args: Id,
    parameters: Id,
    name: Option<Id>,
    instantiable: bool,
) -> Id {
    ctx.alloc(Value::GenericAlias(Alias {
        origin,
        args,
        parameters,
        kind,
        name,
        instantiable,
        unpacked: false,
    }))
}

/// Return the single declared argument of a typing.Unpack alias.
fn unpack_arg(ctx: &Context, alias: &Alias) -> Option<Id> {
    if alias.kind != AliasKind::Unpack {
        return None;
    }
    let Value::Tuple(args) = ctx.heap.get(alias.args) else {
        return None;
    };
    args.first().copied()
}

fn unpacked_tuple_args(ctx: &Context, arg: Id) -> Result<Id> {
    let Value::GenericAlias(inner) = ctx.heap.get(arg) else {
        return Ok(NONE);
    };
    if ctx.builtins.get("tuple") == Some(&inner.origin) {
        Ok(inner.args)
    } else {
        Err(Error::new(
            "TypeError",
            "Unpack[...] must be used with a tuple type",
        ))
    }
}

fn is_unpacked_pack(ctx: &Context, arg: Id) -> bool {
    matches!(ctx.heap.get(arg), Value::GenericAlias(alias)
        if unpack_arg(ctx, alias).is_some_and(|id| matches!(ctx.heap.get(id), Value::TypeVarTuple { .. })))
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
    let variables = native_parameters(ctx, args);
    Ok(allocate_alias(
        ctx,
        AliasKind::Literal(members),
        origin,
        args,
        variables,
        None,
        true,
    ))
}

/// PEP 585 keeps a tuple supplied to the constructor as the alias's actual arguments.
pub(super) fn builtin_alias(ctx: &mut Context, origin: Id, args: Id) -> Id {
    let args = if matches!(ctx.heap.get(args), Value::Tuple(_)) {
        args
    } else {
        ctx.alloc(Value::Tuple(vec![args]))
    };
    let parameters = native_parameters(ctx, args);
    allocate_alias(
        ctx,
        AliasKind::Builtin,
        origin,
        args,
        parameters,
        None,
        true,
    )
}

/// Construct the collections.abc subclass of GenericAlias, including direct keyword calls.
pub(super) fn abc_callable_alias(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    const NAME: &str = "_CallableGenericAlias.__new__";
    let fields = super::builtins::bind_method_arguments(NAME, args, kwargs, ["origin", "args"], 2)?;
    let [Some(origin), Some(pair)] = fields else {
        let missing: Vec<_> = ["origin", "args"]
            .into_iter()
            .zip(fields)
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect();
        return Err(super::calls::missing_arguments_error(
            NAME,
            "positional",
            &missing,
        ));
    };
    let tuple = ctx.builtin("tuple");
    if !ctx.instance_of(pair, tuple) {
        return Err(Error::new("TypeError", CALLABLE_SHAPE_ERROR));
    }
    let length = super::builtins::call(ctx, "len", &[pair], &[])?;
    if ctx.as_integer(length)? != 2 {
        return Err(Error::new("TypeError", CALLABLE_SHAPE_ERROR));
    }
    let args = ctx.unpack_items(pair, 2, None)?;
    if matches!(ctx.heap.get(pair), Value::Tuple(_))
        && !matches!(ctx.heap.get(args[0]), Value::List(_) | Value::Tuple(_))
        && is_param_expr(ctx, args[0])
    {
        let parameters = native_parameters(ctx, pair);
        return Ok(allocate_alias(
            ctx,
            AliasKind::AbcCallable,
            origin,
            pair,
            parameters,
            None,
            true,
        ));
    }
    callable_alias(ctx, AliasKind::AbcCallable, origin, args)
}

const CALLABLE_SHAPE_ERROR: &str = "Callable must be used as Callable[[arg, ...], result].";

fn callable_alias(ctx: &mut Context, kind: AliasKind, origin: Id, args: Vec<Id>) -> Result<Id> {
    let [parameters, result] = args.as_slice() else {
        return Err(Error::new("TypeError", CALLABLE_SHAPE_ERROR));
    };
    if kind == AliasKind::Callable {
        // Cache the spelling supplied by the caller before typing converts its arguments.
        let parameters = if ctx.list_storage(*parameters).is_some() {
            let items = ctx.items(*parameters)?;
            ctx.alloc(Value::Tuple(items))
        } else {
            *parameters
        };
        return alias(ctx, kind, origin, vec![parameters, *result]);
    }
    let tuple = ctx.builtin("tuple");
    let actual = if ctx.list_storage(*parameters).is_some() || ctx.instance_of(*parameters, tuple) {
        let mut flat = ctx.items(*parameters)?;
        flat.push(*result);
        flat
    } else if is_param_expr(ctx, *parameters) {
        args
    } else {
        let expected = "Expected a list of types, an ellipsis, ParamSpec, or Concatenate";
        if kind == AliasKind::AbcCallable {
            let got = ctx.checked_format(*parameters, false)?;
            return Err(Error::new("TypeError", format!("{expected}. Got {got}")));
        }
        return Err(Error::new("TypeError", expected));
    };
    alias(ctx, kind, origin, actual)
}

fn typing_alias_helper(ctx: &mut Context, name: &str, args: Vec<Id>) -> Result<Id> {
    let method = ctx.module_value("typing", name).ok_or_else(|| {
        Error::new(
            "AttributeError",
            format!("module 'typing' has no attribute '{name}'"),
        )
    })?;
    ctx.call(method, args, Vec::new())
}

pub(super) fn is_param_expr(ctx: &Context, arg: Id) -> bool {
    match ctx.heap.get(arg) {
        Value::Ellipsis | Value::ParamSpec(_) | Value::List(_) | Value::Tuple(_) => true,
        Value::GenericAlias(alias) => alias.kind == AliasKind::Concatenate,
        _ => false,
    }
}

fn helper_method(ctx: &mut Context, receiver: Id, helper: &str) -> Option<Id> {
    let callable = ctx.module_value("typing", helper)?;
    Some(ctx.alloc(Value::Method {
        callable,
        receiver,
        explicit: false,
    }))
}

/// Apply the subscription protocol for a supported type. Ordinary instance indexing
/// continues through the object runtime.
pub(super) fn subscribe(ctx: &mut Context, target: Id, index: Id) -> Option<Result<Id>> {
    if let Value::TypeAlias { definition, .. } = ctx.heap.get(target) {
        if let AliasDefinition::Lazy { parameters, .. } = definition
            && matches!(ctx.heap.get(*parameters), Value::Tuple(items) if !items.is_empty())
        {
            return Some(Ok(builtin_alias(ctx, target, index)));
        }
        return Some(Err(Error::new(
            "TypeError",
            "Only generic type aliases are subscriptable",
        )));
    }
    if let Value::GenericAlias(alias) = ctx.heap.get(target)
        && alias.kind == AliasKind::Builtin
    {
        return Some(ctx.module("typing").and_then(|_| {
            ctx.call_typing_helper("_specialize_generic_alias_uncached", vec![target, index])
        }));
    }
    if is_unpacked_pack(ctx, target) {
        return Some(Ok(index));
    }
    if let Value::GenericAlias(alias) = ctx.heap.get(target)
        && (alias.name.is_some()
            || matches!(
                alias.kind,
                AliasKind::Concatenate | AliasKind::Callable | AliasKind::AbcCallable
            )
            || (alias.kind == AliasKind::Generic
                && matches!(ctx.heap.get(alias.origin), Value::Class { .. })))
    {
        return Some(typing_alias_helper(
            ctx,
            "_specialize_generic_alias",
            vec![target, index],
        ));
    }
    if let Value::GenericAlias(alias) = ctx.heap.get(target)
        && matches!(alias.kind, AliasKind::Literal(_) | AliasKind::Unpack)
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
        Value::Class { .. } if ctx.classes.get("typing.Generic") == Some(&target) => {
            AliasKind::Generic
        }
        Value::Native(n) if n == "typing.Callable" => AliasKind::Callable,
        Value::Native(n)
            if matches!(
                n.as_str(),
                "dict"
                    | "list"
                    | "tuple"
                    | "set"
                    | "type"
                    | super::value::REGEX_PATTERN_TYPE
                    | super::value::REGEX_MATCH_TYPE
            ) =>
        {
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
                if args.iter().any(|&id| {
                    !matches!(ctx.heap.get(id), Value::TypeVar(_) | Value::ParamSpec(_))
                        && !is_unpacked_pack(ctx, id)
                }) {
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
                let parameters = ctx.items(params)?;
                if parameters.iter().any(|&id| match ctx.heap.get(id) {
                    Value::ParamSpec(_) | Value::TypeVarTuple { .. } => true,
                    Value::TypeVar(var) => !matches!(ctx.heap.get(var.default), Value::NoDefault),
                    _ => false,
                }) {
                    let constructor = ctx.builtin("typing._GenericAlias");
                    return typing_alias_helper(
                        ctx,
                        "_variadic_class_getitem",
                        vec![target, index, constructor],
                    );
                }
                let required = parameters.len();
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
        if kind == AliasKind::Callable {
            let origin = super::mapping_abc::class(ctx, "Callable")?;
            return callable_alias(ctx, kind, origin, args);
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
    if matches!(
        ctx.heap.get(id),
        Value::TypeVar(_)
            | Value::ParamSpec(_)
            | Value::ParamSpecMarker(_, _)
            | Value::TypeVarTuple { .. }
    ) {
        return Err(Error::new(
            "TypeError",
            format!("Cannot subclass an instance of {}", ctx.type_name(id)),
        ));
    }
    if matches!(ctx.heap.get(id), Value::Native(name) if name == "typing.Callable") {
        let base_tuple = bases;
        let bases = ctx.items(base_tuple)?;
        let callable = super::mapping_abc::class(ctx, "Callable")?;
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
    if matches!(ctx.heap.get(id), Value::GenericAlias(alias) if alias.name.is_some()) {
        return typing_alias_helper(ctx, "_named_alias_bases", vec![id, bases]);
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
    bind_generic_parameters(ctx, scope, generic, &raw)
}

pub(super) fn bind_generic_parameters(
    ctx: &mut Context,
    scope: Id,
    generic: Id,
    raw: &[Id],
) -> Result<()> {
    if raw.contains(&generic) {
        return Err(Error::new("TypeError", "Cannot inherit from plain Generic"));
    }
    let bases = ctx.alloc(Value::Tuple(raw.to_vec()));
    let parameters = collect_type_parameters(ctx, bases, true)?;
    let mut explicit = None;
    for &id in raw {
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
        let missing = ctx
            .items(parameters)?
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
        parameters
    };
    ctx.bind(scope, "__parameters__", params);
    Ok(())
}

/// Members physically stored on a TypeVar and returned unbound by class lookup.
pub(super) const VARIANCE_FIELDS: [&str; 3] =
    ["__covariant__", "__contravariant__", "__infer_variance__"];

/// Metadata for implemented class-owned methods and read-only fields of native type parameters.
pub(super) struct ParameterClass {
    pub(super) owner: &'static str,
    pub(super) methods: &'static [&'static str],
    fields: &'static [&'static str],
    doc: &'static str,
}

impl ParameterClass {
    pub(super) fn lookup(name: &str) -> Option<Self> {
        let (owner, methods, fields, doc): (_, &[_], &[_], _) = match name {
            "typing.TypeVar" => (
                "typing.TypeVar",
                &["__typing_subst__", "has_default"],
                &VARIANCE_FIELDS[..],
                include_str!("../../native/typing-TypeVar-doc.txt"),
            ),
            "typing.TypeVarTuple" => (
                "typing.TypeVarTuple",
                &["__iter__", "__reduce__", "has_default"],
                &["__name__"],
                include_str!("../../native/typing-TypeVarTuple-doc.txt"),
            ),
            "typing.ParamSpec" => (
                "typing.ParamSpec",
                &[
                    "__typing_subst__",
                    "__typing_prepare_subst__",
                    "has_default",
                    "__reduce__",
                ],
                &[
                    "__name__",
                    "__bound__",
                    "__covariant__",
                    "__contravariant__",
                    "__infer_variance__",
                ],
                include_str!("../../native/typing-ParamSpec-doc.txt"),
            ),
            "typing.ParamSpecArgs" => (
                "typing.ParamSpecArgs",
                &["__eq__", "__ne__"],
                &["__origin__"],
                include_str!("../../native/typing-ParamSpecArgs-doc.txt"),
            ),
            "typing.ParamSpecKwargs" => (
                "typing.ParamSpecKwargs",
                &["__eq__", "__ne__"],
                &["__origin__"],
                include_str!("../../native/typing-ParamSpecKwargs-doc.txt"),
            ),
            _ => return None,
        };
        Some(Self {
            owner,
            methods,
            fields,
            doc,
        })
    }

    pub(super) fn install(&self, ctx: &mut Context, class: Id) {
        let mut entries = Vec::new();
        for &name in self.methods {
            let method = ctx.builtin(&format!("{}.{name}", self.owner));
            entries.push((ctx.text(name), method));
        }
        for &name in self.fields {
            let member = ctx.readonly_member(self.owner, name);
            entries.push((ctx.text(name), member));
        }
        if matches!(
            self.owner,
            "typing.ParamSpecArgs" | "typing.ParamSpecKwargs"
        ) {
            entries.push((ctx.text("__hash__"), NONE));
        }
        entries.push((ctx.text("__doc__"), ctx.text(self.doc)));
        entries.push((ctx.text("__module__"), ctx.text("typing")));
        let dictionary = ctx.dict(entries);
        ctx.native_type_dicts.insert(class, dictionary);
    }
}

/// Expose the implemented members owned by each alias class through its mapping proxy.
pub(super) fn install_alias_class(ctx: &mut Context, owner: &str, class: Id) {
    let mut entries = Vec::new();
    match owner {
        "typing.TypeAliasType" => {
            let owner = "typing.TypeAliasType";
            for field in [
                "__module__",
                "__value__",
                "__parameters__",
                "__type_params__",
                "evaluate_value",
            ] {
                let get = ctx.builtin(&format!("{owner}.{field}"));
                let member = ctx.alloc(Value::GetSetDescriptor {
                    get,
                    set: None,
                    owner,
                    name: field,
                });
                entries.push((ctx.text(field), member));
            }
            let name = ctx.readonly_member(owner, "__name__");
            entries.push((ctx.text("__name__"), name));
            let reduce = ctx.builtin("typing.TypeAliasType.__reduce__");
            entries.push((ctx.text("__reduce__"), reduce));
            entries.push((
                ctx.text("__doc__"),
                ctx.text(include_str!("../../native/typing-TypeAliasType-doc.txt")),
            ));
        }
        "typing._GenericAlias" | "typing._UnpackGenericAlias" => {
            entries.push((ctx.text("__doc__"), NONE));
            entries.push((ctx.text("__module__"), ctx.text("typing")));
            if owner == "typing._UnpackGenericAlias" {
                for name in [
                    "__typing_unpacked_tuple_args__",
                    "__typing_is_unpacked_typevartuple__",
                ] {
                    let getter = ctx.builtin(&format!("{owner}.{name}"));
                    let key = ctx.text(name);
                    let property = super::property::Property::readonly(getter, key);
                    entries.push((key, ctx.alloc(Value::Property(property))));
                }
            }
        }
        _ => return,
    }
    let dictionary = ctx.dict(entries);
    ctx.native_type_dicts.insert(class, dictionary);
}

/// These two functions are defined by the source typing module after its native classes exist.
pub(super) fn include_generic_alias_methods(
    ctx: &mut Context,
    class: Id,
    dictionary: Id,
) -> Result<()> {
    for name in ["copy_with", "__reduce__"] {
        if ctx.own_class_attr(class, name).is_none()
            && let Some(method) = generic_alias_method(ctx, name)
        {
            let key = ctx.text(name);
            ctx.put_item(dictionary, key, method)?;
        }
    }
    Ok(())
}

fn generic_alias_method(ctx: &Context, name: &str) -> Option<Id> {
    match name {
        "copy_with" => ctx.class_attr(ctx.module_value("typing", "_SpecialGenericAlias")?, name),
        "__reduce__" => ctx.module_value("typing", "_reduce_generic_alias"),
        _ => None,
    }
}

pub(super) fn attribute(ctx: &mut Context, value: Id, name: &str) -> Option<Result<Id>> {
    if name != "__name__"
        && matches!(ctx.heap.get(value), Value::Native(class) if class.starts_with("typing.") && !(class == "typing.TypeAliasType" && name == "__module__"))
        && let Some(&dictionary) = ctx.native_type_dicts.get(&value)
        && let Some(member) = ctx.dict_named_item(dictionary, name)
    {
        return Some(Ok(member));
    }
    let attr = match ctx.heap.get(value) {
        Value::Native(class) if class == "collections.abc._CallableGenericAlias"
            && matches!(name, "__base__" | "__bases__" | "__mro__") =>
        {
            if name == "__base__" {
                ctx.builtin("types.GenericAlias")
            } else {
                ctx.builtin_hierarchy_tuple(value, name)
            }
        }
        Value::Native(class)
            if matches!(
                class.as_str(),
                "typing._GenericAlias" | "typing._UnpackGenericAlias"
            ) && matches!(name, "copy_with" | "__reduce__") =>
        {
            generic_alias_method(ctx, name)?
        }
        Value::GenericAlias(alias)
            if matches!(
                alias.kind,
                AliasKind::Generic | AliasKind::Callable | AliasKind::Unpack
            ) && name == "copy_with" =>
        {
            let callable = generic_alias_method(ctx, name)?;
            ctx.alloc(Value::Method {
                callable,
                receiver: value,
                explicit: false,
            })
        }
        Value::TypeVar(_)
        | Value::ParamSpec(_)
        | Value::ParamSpecMarker(_, _)
        | Value::TypeVarTuple { .. }
        | Value::GenericAlias(_)
            if name == "__mro_entries__" =>
        {
            let callable = ctx.builtin("typing.alias_mro_entries");
            ctx.alloc(Value::Method {
                callable,
                receiver: value,
                explicit: false,
            })
        }
        Value::Native(class) if class == "typing._ConcatenateGenericAlias" => match name {
            "__base__" => ctx.builtin("typing._GenericAlias"),
            "__bases__" => {
                let parent = ctx.builtin("typing._GenericAlias");
                ctx.alloc(Value::Tuple(vec![parent]))
            }
            "copy_with" => ctx.module_value("typing", "_concatenate_copy_with")?,
            "__module__" => ctx.text("typing"),
            _ => return None,
        },
        Value::Native(class) if class == "typing.Callable" && name == "__mro_entries__" => {
            let callable = ctx.builtin("typing.alias_mro_entries");
            ctx.alloc(Value::Method {
                callable,
                receiver: value,
                explicit: false,
            })
        }
        Value::Native(class)
            if class == "typing.TypeAliasType" && matches!(name, "__name__" | "__qualname__") =>
        {
            ctx.text("TypeAliasType")
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
        Value::Native(class) if name == "__reduce__" && class == "typing._CallableGenericAlias" => {
            ctx.module_value("typing", "_reduce_callable_alias")?
        }
        Value::GenericAlias(alias)
            if name == "__reduce__"
                && matches!(alias.kind, AliasKind::Generic | AliasKind::Callable) =>
        {
            let class = ctx.builtin(alias.kind.class());
            return Some(
                ctx.lookup_attr(class, name)
                    .and_then(|reducer| ctx.bind_attribute(reducer, value, class)),
            );
        }
        Value::NoDefault if name == "__reduce__" => {
            let callable = ctx.builtin("typing.NoDefault.__reduce__");
            ctx.alloc(Value::Method {
                callable,
                receiver: value,
                explicit: false,
            })
        }
        Value::Native(class) if class == "NoDefaultType" && name == "__module__" => {
            ctx.text("builtins")
        }
        Value::TypeVarTuple {
            name: label,
            module,
            default,
            declared,
        } => match name {
            "__name__" => *label,
            "__module__" => *module,
            "__default__" => *default,
            "evaluate_default" if *declared => NONE,
            "evaluate_default" => ctx.alloc(Value::ConstEvaluator(*default)),
            "has_default" | "__reduce__" | "__iter__" => {
                let callable = ctx.builtin(&format!("typing.TypeVarTuple.{name}"));
                ctx.alloc(Value::Method {
                    callable,
                    receiver: value,
                    explicit: false,
                })
            }
            _ => return None,
        },
        Value::TypeAlias {
            name: label,
            definition,
        } => match (name, *definition) {
            ("__name__", _) => *label,
            ("__parameters__" | "__type_params__", AliasDefinition::Lazy { parameters, .. }) => {
                parameters
            }
            ("__parameters__" | "__type_params__", AliasDefinition::Eager { .. }) => {
                ctx.heap.packed_tuple(Vec::new(), false)
            }
            ("__module__", AliasDefinition::Eager { module, .. }) => module,
            ("__module__", AliasDefinition::Lazy { evaluator, .. }) => {
                return Some(ctx.lookup_attr(evaluator, "__module__"));
            }
            ("evaluate_value", AliasDefinition::Eager { value, .. }) => {
                ctx.alloc(Value::ConstEvaluator(value))
            }
            ("evaluate_value", AliasDefinition::Lazy { evaluator, .. }) => evaluator,
            (
                "__value__",
                AliasDefinition::Eager { value, .. }
                | AliasDefinition::Lazy {
                    cached: Some(value),
                    ..
                },
            ) => value,
            ("__value__", AliasDefinition::Lazy { evaluator, .. }) => {
                ctx.heap.keep(value);
                return Some(
                    ctx.call(evaluator, Vec::new(), Vec::new())
                        .inspect(|&result| {
                            let Value::TypeAlias {
                                definition: AliasDefinition::Lazy { cached, .. },
                                ..
                            } = ctx.heap.get_mut(value)
                            else {
                                unreachable!()
                            };
                            *cached = Some(result);
                        }),
                );
            }
            ("__reduce__", _) => {
                let callable = ctx.builtin("typing.TypeAliasType.__reduce__");
                ctx.alloc(Value::Method {
                    callable,
                    receiver: value,
                    explicit: false,
                })
            }
            _ => return None,
        },
        Value::TypeVar(var) => match name {
            "evaluate_bound" | "evaluate_constraints" | "evaluate_default" if var.declared => NONE,
            "__name__" => var.name,
            "__module__" => var.module,
            "__bound__" => var.bound,
            "__constraints__" => var.constraints,
            "__default__" => var.default,
            "evaluate_default" => ctx.alloc(Value::ConstEvaluator(var.default)),
            "__typing_subst__" | "has_default" => {
                let callable = ctx.builtin(&format!("typing.TypeVar.{name}"));
                ctx.alloc(Value::Method {
                    callable,
                    receiver: value,
                    explicit: false,
                })
            }
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
        Value::GenericAlias(a) if matches!(a.kind, AliasKind::Literal(_)) => {
            if let Some((_, field)) = a
                .instance_fields()
                .into_iter()
                .find(|(key, _)| *key == name)
            {
                field
            } else if forwards_to_origin(name) {
                let origin = literal_origin(ctx, a.origin);
                return Some(origin.and_then(|(origin, _)| ctx.lookup_attr(origin, name)));
            } else {
                return None;
            }
        }
        Value::GenericAlias(a) => match name {
            "__dir__"
                if matches!(
                    a.kind,
                    AliasKind::Generic | AliasKind::Callable | AliasKind::Unpack
                ) =>
            {
                let callable = ctx.builtin("typing._GenericAlias.__dir__");
                ctx.alloc(Value::Method {
                    callable,
                    receiver: value,
                    explicit: false,
                })
            }

            "__unpacked__" if a.is_builtin_alias() => {
                if a.unpacked {
                    TRUE
                } else {
                    FALSE
                }
            }
            "__typing_unpacked_tuple_args__" if a.is_builtin_alias() => {
                if a.unpacked && ctx.builtins.get("tuple") == Some(&a.origin) {
                    a.args
                } else {
                    NONE
                }
            }
            "__typing_is_unpacked_typevartuple__" if a.kind == AliasKind::Unpack => {
                if is_unpacked_pack(ctx, value) {
                    TRUE
                } else {
                    FALSE
                }
            }
            "__typing_unpacked_tuple_args__" if let Some(arg) = unpack_arg(ctx, a) => {
                return Some(unpacked_tuple_args(ctx, arg));
            }
            "__name__" | "__qualname__" | "__module__" if a.kind == AliasKind::Unpack => {
                return Some(ctx.lookup_attr(a.origin, name));
            }
            "__name__" | "__qualname__" | "_name" if let Some(label) = a.name => label,
            "__module__" if a.name.is_some() => ctx.text("typing"),
            "__name__" | "__qualname__" if a.kind == AliasKind::Concatenate => {
                return Some(ctx.lookup_attr(a.origin, "__name__"));
            }
            "copy_with" if a.kind == AliasKind::Concatenate => {
                helper_method(ctx, value, "_concatenate_copy_with")?
            }
            name if a.kind.is_typing_alias()
                && let Some((_, value)) = a
                    .instance_fields()
                    .into_iter()
                    .find(|(field, _)| *field == name) =>
            {
                value
            }
            "__doc__" if a.kind.is_typing_alias() => {
                let class = ctx.builtin(a.kind.class());
                ctx.own_class_attr(class, "__doc__")?
            }
            attr if let Some((_, value)) = a.metadata().find(|(name, _)| *name == attr) => value,
            attr if (matches!(
                a.kind,
                AliasKind::Callable | AliasKind::Unpack | AliasKind::Concatenate
            ) || (a.kind != AliasKind::Generic && a.name.is_some()))
                && forwards_to_origin(attr) =>
            {
                let origin = a.origin;
                return Some(
                    ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
                        ctx.lookup_attr(origin, attr)
                    }),
                );
            }
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

/// Create an unbounded compiler-declared pack, whose missing default has no evaluator.
pub(super) fn declared_pack(ctx: &mut Context, name: Id) -> Id {
    let module = ctx.text("typing");
    let default = no_default(ctx);
    ctx.alloc(Value::TypeVarTuple {
        name,
        module,
        default,
        declared: true,
    })
}

/// Create the lexical scope for a declaration's type parameters.
pub(super) fn parameter_scope(ctx: &mut Context, outer: Id, names: Id, class: bool) -> Result<Id> {
    let enclosing = ctx.scope_data(outer);
    let parent = if enclosing.class_body.is_some() {
        enclosing.parent.unwrap_or(outer)
    } else {
        outer
    };
    let module = enclosing.module.clone();
    let scope = ctx.scope(&module, Some(parent));
    let typing = ctx.text("typing");
    let mut params = Vec::new();
    for (binding, declaration) in ctx
        .dict_entries(names)
        .expect("declared type parameters")
        .to_vec()
    {
        let param = match ctx.heap.get(declaration) {
            Value::TypeVarTuple { .. } | Value::ParamSpec(_) => declaration,
            _ => typevar(
                ctx,
                &[declaration],
                &[("infer_variance".to_owned(), TRUE)],
                typing,
                true,
            )?,
        };
        let binding = ctx.str_value(binding)?.to_owned();
        ctx.bind(scope, &binding, param);
        params.push(param);
    }
    let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    frame.closure.declared.extend(frame.names.keys().cloned());
    frame.closure.declared_complete = true;
    let declared = ctx.alloc(Value::Tuple(params.clone()));
    ctx.bind(scope, ".type_params", declared);
    if class {
        // __type_params__ retains the pack itself; the implicit base is Generic[*Ts].
        for param in &mut params {
            if matches!(ctx.heap.get(*param), Value::TypeVarTuple { .. }) {
                *param = unpack_parameter(ctx, *param)?;
            }
        }
        let generic = named_class(ctx, "typing", "Generic");
        let base = alias(ctx, AliasKind::Generic, generic, params)?;
        ctx.bind(scope, ".generic_base", base);
    }
    Ok(scope)
}

fn typevar(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
    module: Id,
    declared: bool,
) -> Result<Id> {
    let mut name = args.first().copied();
    let mut bound = NONE;
    let mut default = no_default(ctx);
    let mut covariant = false;
    let mut contravariant = false;
    let mut infer_variance = false;
    for (key, val) in kwargs {
        match key.as_str() {
            "name" if name.is_none() => name = Some(*val),
            "bound" => bound = *val,
            "default" => default = *val,
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
    if infer_variance && (covariant || contravariant) {
        return Err(Error::new(
            "ValueError",
            "Variance cannot be specified with infer_variance.",
        ));
    }
    let has_bound = bound != NONE;
    if has_bound {
        let module = ctx.module("typing")?;
        let check = ctx.lookup_attr(module, "_type_check")?;
        let message = ctx.text("Bound must be a type.");
        bound = ctx.call(check, vec![bound, message], Vec::new())?;
    }
    if constraints.len() == 1 {
        return Err(Error::new(
            "TypeError",
            "A single constraint is not allowed",
        ));
    }
    if !constraints.is_empty() && has_bound {
        return Err(Error::new(
            "TypeError",
            "Constraints cannot be combined with bound=...",
        ));
    }
    let constraints = ctx.alloc(Value::Tuple(constraints.to_vec()));
    Ok(ctx.alloc(Value::TypeVar(TypeVariable {
        name,
        module,
        bound,
        constraints,
        default,
        covariant,
        contravariant,
        infer_variance,
        declared,
    })))
}

/// Parse annotation text without evaluating it or constructing Python AST objects.
fn parse_annotation_expression(
    source: &str,
) -> Result<ruff_python_parser::Parsed<ruff_python_ast::ModExpression>> {
    use ruff_python_ast::token::TokenKind;

    if source.contains('\0') {
        return Err(Error::new(
            "SyntaxError",
            "source code string cannot contain null bytes",
        ));
    }
    // CPython's tokenizer caps the combined nesting of (), [] and {} at 200.
    // Keep deeply nested input out of the recursive parser. Short references only lex once.
    const MAX_NESTING: usize = 200;
    if source.len() > MAX_NESTING {
        let mut lexer =
            ruff_python_parser::lexer::lex(source, ruff_python_parser::Mode::Expression);
        let mut depth = 0_usize;
        loop {
            match lexer.next_token() {
                TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                    depth += 1;
                    if depth > MAX_NESTING {
                        return Err(Error::new("SyntaxError", "too many nested parentheses"));
                    }
                }
                TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => {
                    depth = depth.saturating_sub(1)
                }
                TokenKind::EndOfFile | TokenKind::Unknown => break,
                _ => {}
            }
        }
    }
    ruff_python_parser::parse_expression(source)
        .map_err(|error| Error::new("SyntaxError", error.to_string()))
}

/// Check a forward reference without evaluating names or converting the tree into Python AST objects.
fn validate_forward_ref(source: &str) -> Result<()> {
    use ruff_python_ast::token::TokenKind;
    use ruff_python_ast::visitor::{self, Visitor};
    use ruff_python_ast::{Expr, ExprContext};
    use ruff_text_size::Ranged;
    use std::collections::HashSet;

    fn invalid_binding<'a>(mut names: impl Iterator<Item = &'a str>) -> bool {
        let mut seen = HashSet::new();
        names.any(|name| name == "__debug__" || !seen.insert(name))
    }

    #[derive(Default)]
    struct Bindings {
        invalid: bool,
    }
    impl<'a> Visitor<'a> for Bindings {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.invalid {
                return;
            }
            self.invalid = match expr {
                Expr::Call(call) => invalid_binding(
                    call.arguments
                        .keywords
                        .iter()
                        .filter_map(|kw| kw.arg.as_deref()),
                ),
                Expr::Lambda(lambda) => lambda.parameters.as_ref().is_some_and(|params| {
                    invalid_binding(params.iter().map(|p| p.name().as_str()))
                }),
                Expr::Name(name) => name.ctx == ExprContext::Store && name.id == "__debug__",
                Expr::Attribute(attr) => {
                    attr.ctx == ExprContext::Store && attr.attr.as_str() == "__debug__"
                }
                _ => false,
            };
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_expr(self, expr)
            });
        }
    }
    let parsed = parse_annotation_expression(source)?;
    let body = parsed.syntax().body.as_ref();
    let invalid = match body {
        Expr::Yield(_) => Some("'yield' outside function"),
        Expr::YieldFrom(_) => Some("'yield from' outside function"),
        Expr::Await(_) => Some("'await' outside function"),
        _ => None,
    };
    if let Some(message) = invalid {
        return Err(Error::new("SyntaxError", message));
    }
    // The grammar alone cannot validate nested coroutine/generator scopes or walrus
    // bindings across comprehension scopes. Those require a compile-time symbol table.
    let tokens = parsed.tokens();
    let comprehension = tokens.iter().any(|t| t.kind() == TokenKind::For);
    let lambda_body = body.as_lambda_expr().map(|lambda| lambda.body.range());
    if tokens.iter().any(|token| match token.kind() {
        TokenKind::Await | TokenKind::Async => true,
        TokenKind::Yield => {
            comprehension || !lambda_body.is_some_and(|r| r.contains_range(token.range()))
        }
        TokenKind::ColonEqual => comprehension,
        _ => false,
    }) {
        return Err(Error::new(
            "NotImplementedError",
            "forward reference requires scope validation",
        ));
    }
    let mut bindings = Bindings::default();
    bindings.visit_expr(body);
    if bindings.invalid {
        return Err(Error::new(
            "SyntaxError",
            "invalid binding in forward reference",
        ));
    }
    Ok(())
}

/// Finalize a temporary annotation name in place, including references retained by
/// a user callback or inside a previously constructed typing alias.
fn finish_field_ref(ctx: &mut Context, value: Id) -> Result<Id> {
    let invalid = || Error::new("TypeError", "expected a TypedDict annotation symbol");
    let Some(source) = ctx.module_value("typing", "_TypedDictSymbol") else {
        return Err(invalid());
    };
    let target = ctx
        .module_value("annotationlib", "ForwardRef")
        .ok_or_else(invalid)?;
    if !matches!(ctx.heap.get(value), Value::Instance { class, .. } if *class == source) {
        return Err(invalid());
    }
    // Module attributes can be rebound. Only the two ordinary, equally slotted
    // layouts can exchange their class pointer without moving any field storage.
    let object = ctx.builtin("object");
    let slots = |class, expected_name| {
        let Value::Class {
            name,
            scope,
            ancestors,
            io_kind: None,
            namedtuple_fields: None,
            ..
        } = ctx.heap.get(class)
        else {
            return None;
        };
        let layout = ctx.scope_data(*scope).class_namespace.as_ref()?;
        if name != expected_name
            || ancestors.as_slice() != [object]
            || layout.member_slots
            || ctx.class_attr(class, "__del__").is_some()
        {
            return None;
        }
        layout.slot_names.as_deref()
    };
    let source_slots = slots(source, "_TypedDictSymbol").ok_or_else(invalid)?;
    if slots(target, "ForwardRef") != Some(source_slots) {
        return Err(invalid());
    }
    let Value::Instance { class, .. } = ctx.heap.get_mut(value) else {
        unreachable!()
    };
    *class = target;
    ctx.heap.weak_edge_removed(source);
    Ok(value)
}

pub(super) fn call(
    ctx: &mut Context,
    operation: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(
        operation,
        "TypeVar.__typing_subst__"
            | "ParamSpec.__typing_subst__"
            | "ParamSpec.__typing_prepare_subst__"
    ) {
        let (owner, method) = operation.split_once('.').unwrap();
        let Some((&parameter, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("unbound method {operation}() needs an argument"),
            ));
        };
        if !matches!(
            (owner, ctx.heap.get(parameter)),
            ("TypeVar", Value::TypeVar(_)) | ("ParamSpec", Value::ParamSpec(_))
        ) {
            let actual = ctx
                .builtin_type_name(parameter)
                .unwrap_or_else(|| ctx.type_name(parameter));
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' for 'typing.{owner}' objects doesn't apply to a '{actual}' object"
                ),
            ));
        }
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{operation}() takes no keyword arguments"),
            ));
        }
        let helper = match (method, rest.len()) {
            ("__typing_subst__", 1) if owner == "TypeVar" => "_typevar_subst",
            ("__typing_subst__", 1) => "_paramspec_subst",
            ("__typing_prepare_subst__", 2) => "_paramspec_prepare_subst",
            ("__typing_subst__", n) => {
                return Err(Error::new(
                    "TypeError",
                    format!("{operation}() takes exactly one argument ({n} given)"),
                ));
            }
            (_, n) => {
                return Err(Error::new(
                    "TypeError",
                    format!("{method} expected 2 arguments, got {n}"),
                ));
            }
        };
        return typing_alias_helper(ctx, helper, args.to_vec());
    }
    if let ("_protocol_mro", [class], []) = (operation, args, kwargs) {
        if matches!(ctx.heap.get(*class), Value::Native(_)) {
            let bases = ctx.base_lineage(*class, true);
            return Ok(ctx.alloc(Value::Tuple(bases)));
        }
        return ctx.lookup_attr(*class, "__mro__");
    }
    if let ("_protocol_member", [class, attr, missing], []) = (operation, args, kwargs) {
        if let Value::Native(owner) = ctx.heap.get(*class) {
            let name = ctx.str_value(*attr)?;
            if let Some(member) = ctx.own_class_attr(*class, name) {
                return Ok(member);
            }
            let symbol = format!("{owner}.{name}");
            return Ok(
                if super::wrapper_descriptor::BuiltinDescriptor::lookup(&symbol).is_some() {
                    ctx.builtin(&symbol)
                } else {
                    *missing
                },
            );
        }
        let namespace = ctx.lookup_attr(*class, "__dict__")?;
        return match ctx.get_item(namespace, *attr) {
            Ok(member) => Ok(member),
            Err(error) if error.kind == "KeyError" => Ok(*missing),
            Err(error) => Err(error),
        };
    }
    if operation == "_new_typeddict" {
        let [name_id, root] = args else {
            return Err(Error::new(
                "TypeError",
                "expected TypedDict name and root flag",
            ));
        };
        if ctx.string_storage(*name_id).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "type.__new__() argument 1 must be str, not {}",
                    ctx.type_name(*name_id)
                ),
            ));
        }
        let name = ctx.str_value(*name_id)?.to_owned();
        let (module, base) = if *root == TRUE {
            (ctx.text("typing"), ctx.builtin("object"))
        } else {
            let caller = ctx.heap.caller_module_at(1).unwrap_or(NONE);
            let module = if ctx.truth(caller)? {
                caller
            } else {
                ctx.text("__main__")
            };
            (module, ctx.builtin("dict"))
        };
        let module_name = ctx.str_value(module).unwrap_or("builtins").to_owned();
        let scope = ctx.scope(&module_name, None);
        ctx.bind(scope, "__module__", module);
        ctx.bind(scope, "__qualname__", *name_id);
        let class = ctx.allocate_class(name, scope, vec![base])?;
        ctx.set_attr(class, "__name__", *name_id)?;
        let meta = ctx
            .module_value("typing", "_TypedDictMeta")
            .ok_or_else(|| Error::new("RuntimeError", "missing TypedDict metaclass"))?;
        let Value::Class { metaclass, .. } = ctx.heap.get_mut(class) else {
            unreachable!()
        };
        *metaclass = Some(meta);
        ctx.enable_class_annotation_provider(scope);
        return Ok(class);
    }
    match operation {
        "ParamSpec" => return super::paramspec::construct(ctx, args, kwargs),
        "ParamSpecArgs" => {
            return super::paramspec::construct_marker(
                ctx,
                super::paramspec::Marker::Args,
                args,
                kwargs,
            );
        }
        "ParamSpecKwargs" => {
            return super::paramspec::construct_marker(
                ctx,
                super::paramspec::Marker::Kwargs,
                args,
                kwargs,
            );
        }
        _ => {}
    }
    if operation == "TypeVarTuple" {
        let [name, default] = super::builtins::bind_method_arguments(
            "typevartuple",
            args,
            kwargs,
            ["name", "default"],
            1,
        )?;
        let name = name.ok_or_else(|| {
            Error::new(
                "TypeError",
                "typevartuple() missing required argument 'name' (pos 1)",
            )
        })?;
        if ctx.string_storage(name).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "typevartuple() argument 'name' must be str, not {}",
                    ctx.type_name(name)
                ),
            ));
        }
        let module = ctx.heap.caller_module();
        let default = default.unwrap_or_else(|| no_default(ctx));
        return Ok(ctx.alloc(Value::TypeVarTuple {
            name,
            module,
            default,
            declared: false,
        }));
    }
    if operation == "TypeVar" {
        let module = ctx.heap.caller_module();
        return typevar(ctx, args, kwargs, module, false);
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
    if matches!(
        operation,
        "_GenericAlias" | "_CallableGenericAlias" | "_ConcatenateGenericAlias"
    ) {
        let [origin, parameters, inst, name] = super::builtins::bind_method_arguments(
            operation,
            args,
            kwargs,
            ["origin", "args", "inst", "name"],
            2,
        )?;
        let (Some(origin), Some(parameters)) = (origin, parameters) else {
            return Err(Error::new(
                "TypeError",
                "_GenericAlias.__init__() requires an origin and arguments",
            ));
        };
        let args = if matches!(ctx.heap.get(parameters), Value::Tuple(_)) {
            parameters
        } else {
            ctx.alloc(Value::Tuple(vec![parameters]))
        };
        let name = name.filter(|&name| name != NONE);
        if let Some(name) = name {
            ctx.str_value(name)?;
        }
        let instantiable = ctx.truth(inst.unwrap_or(TRUE))?;
        let kind = match operation {
            "_CallableGenericAlias" => AliasKind::Callable,
            "_ConcatenateGenericAlias" => AliasKind::Concatenate,
            _ => AliasKind::Generic,
        };
        let enforce_order = ["typing.Generic", "typing.Protocol"]
            .iter()
            .any(|key| ctx.classes.get(*key) == Some(&origin));
        let parameters = collect_type_parameters(ctx, args, enforce_order)?;
        return Ok(allocate_alias(
            ctx,
            kind,
            origin,
            args,
            parameters,
            name,
            instantiable,
        ));
    }
    if operation == "TypeAliasType" {
        let [name, value, params] = super::builtins::bind_method_arguments(
            "typealias",
            args,
            kwargs,
            ["name", "value", "type_params"],
            2,
        )?;
        let name = name.ok_or_else(|| {
            Error::new(
                "TypeError",
                "typealias() missing required argument 'name' (pos 1)",
            )
        })?;
        let value = value.ok_or_else(|| {
            Error::new(
                "TypeError",
                "typealias() missing required argument 'value' (pos 2)",
            )
        })?;
        if ctx.string_storage(name).is_none() {
            let kind = if name == NONE {
                "None"
            } else {
                ctx.builtin_type_name(name)
                    .unwrap_or_else(|| ctx.type_name(name))
            };
            return Err(Error::new(
                "TypeError",
                format!("typealias() argument 'name' must be str, not {kind}"),
            ));
        }
        if let Some(params) = params {
            let Value::Tuple(items) = ctx.heap.get(ctx.scalar_value(params)) else {
                return Err(Error::new("TypeError", "type_params must be a tuple"));
            };
            if !items.is_empty() {
                return Err(Error::new(
                    "NotImplementedError",
                    "generic TypeAliasType construction",
                ));
            }
        }
        let module = ctx.heap.caller_module();
        return Ok(ctx.alloc(Value::TypeAlias {
            name,
            definition: AliasDefinition::Eager { value, module },
        }));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("unexpected keyword for typing.{operation}"),
        ));
    }
    match (operation, args) {
        ("_GenericAlias.__dir__", [receiver])
            if let Value::GenericAlias(alias) = ctx.heap.get(*receiver)
                && matches!(
                    alias.kind,
                    AliasKind::Generic | AliasKind::Callable | AliasKind::Unpack
                ) =>
        {
            super::builtins::dir::typing_alias_dir(ctx, &alias.clone())
        }

        (operation, [receiver])
            if let Some(field) = operation.strip_prefix("TypeAliasType.")
                && matches!(
                    field,
                    "__module__"
                        | "__value__"
                        | "__parameters__"
                        | "__type_params__"
                        | "evaluate_value"
                ) =>
        {
            ctx.read_member("typing.TypeAliasType", field, *receiver)
        }
        (operation, [receiver])
            if let Some(field) = operation.strip_prefix("_UnpackGenericAlias.")
                && matches!(
                    field,
                    "__typing_unpacked_tuple_args__" | "__typing_is_unpacked_typevartuple__"
                ) =>
        {
            let origin = ctx.lookup_attr(*receiver, "__origin__")?;
            let args = ctx.lookup_attr(*receiver, "__args__")?;
            if ctx.module_value("typing", "Unpack") != Some(origin)
                || !matches!(ctx.heap.get(args), Value::Tuple(items) if items.len() == 1)
            {
                return Err(Error::new("AssertionError", ""));
            }
            let index = ctx.int(0);
            let arg = ctx.get_item(args, index)?;
            if field == "__typing_is_unpacked_typevartuple__" {
                return Ok(super::builtins::boolean(matches!(
                    ctx.heap.get(arg),
                    Value::TypeVarTuple { .. }
                )));
            }
            unpacked_tuple_args(ctx, arg)
        }
        ("TypeAliasType.__reduce__", [alias])
            if let Value::TypeAlias { name, .. } = ctx.heap.get(*alias) =>
        {
            Ok(*name)
        }
        ("NoDefault.__reduce__", [value]) if matches!(ctx.heap.get(*value), Value::NoDefault) => {
            Ok(ctx.text("NoDefault"))
        }
        ("TypeVar.has_default", [var]) if let Value::TypeVar(var) = ctx.heap.get(*var) => Ok(
            super::builtins::boolean(!matches!(ctx.heap.get(var.default), Value::NoDefault)),
        ),
        ("TypeVar.has_default", _) => Err(Error::new(
            "TypeError",
            "TypeVar.has_default() takes no arguments",
        )),
        ("ParamSpec.has_default", [spec]) if let Value::ParamSpec(p) = ctx.heap.get(*spec) => Ok(
            super::builtins::boolean(!matches!(ctx.heap.get(p.default), Value::NoDefault)),
        ),
        ("ParamSpec.__reduce__", [spec]) if let Value::ParamSpec(p) = ctx.heap.get(*spec) => {
            Ok(p.name)
        }
        (
            "ParamSpecArgs.__eq__"
            | "ParamSpecKwargs.__eq__"
            | "ParamSpecArgs.__ne__"
            | "ParamSpecKwargs.__ne__",
            [left, right],
        ) => {
            let (owner, method) = operation.split_once('.').unwrap();
            let owner = format!("typing.{owner}");
            if !matches!(ctx.heap.get(*left), Value::ParamSpecMarker(kind, _) if kind.class() == owner)
            {
                let actual = match ctx.heap.get(*left) {
                    Value::ParamSpec(_)
                    | Value::ParamSpecMarker(_, _)
                    | Value::TypeVar(_)
                    | Value::TypeVarTuple { .. } => ctx.builtin_type_name(*left).unwrap(),
                    _ => ctx.type_name(*left),
                };
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '{method}' requires a '{owner}' object but received a '{actual}'"
                    ),
                ));
            }
            let op = if method == "__eq__" { "==" } else { "!=" };
            super::paramspec::compare(ctx, op, *left, *right)
        }
        ("TypeVarTuple.has_default", [pack])
            if let Value::TypeVarTuple { default, .. } = ctx.heap.get(*pack) =>
        {
            Ok(if matches!(ctx.heap.get(*default), Value::NoDefault) {
                FALSE
            } else {
                TRUE
            })
        }
        ("TypeVarTuple.__reduce__", [pack])
            if let Value::TypeVarTuple { name, .. } = ctx.heap.get(*pack) =>
        {
            Ok(*name)
        }
        ("TypeVarTuple.__iter__", [pack])
            if matches!(ctx.heap.get(*pack), Value::TypeVarTuple { .. }) =>
        {
            ctx.iterator(*pack)
        }
        ("TypeVarTuple.has_default" | "TypeVarTuple.__reduce__" | "TypeVarTuple.__iter__", _) => {
            Err(Error::new(
                "TypeError",
                format!("invalid arguments to {operation}"),
            ))
        }
        ("_UnpackGenericAlias", [origin, args]) => {
            if !matches!(ctx.heap.get(*args), Value::Tuple(items) if items.len() == 1) {
                return Err(Error::new("TypeError", "Unpack requires one argument"));
            }
            let parameters = collect_type_parameters(ctx, *args, false)?;
            Ok(allocate_alias(
                ctx,
                AliasKind::Unpack,
                *origin,
                *args,
                parameters,
                None,
                true,
            ))
        }
        ("Any", _) => Err(Error::new("TypeError", "Any cannot be instantiated")),
        ("_LiteralGenericAlias", [origin, parameters]) => literal_alias(ctx, *origin, *parameters),
        ("_LiteralGenericAlias", _) => Err(Error::new(
            "TypeError",
            "_GenericAlias.__init__() requires an origin and arguments",
        )),
        ("_finish_field_ref", [value]) => finish_field_ref(ctx, *value),
        ("_check_annotation_syntax", [text]) => {
            if matches!(ctx.heap.get(ctx.scalar_value(*text)), Value::SurrogateStr(_)) {
                ctx.encode_utf8_codepoints(*text, false, "strict")?;
            }
            parse_annotation_expression(ctx.str_value(*text)?)?;
            Ok(NONE)
        }
        ("_validate_forward_ref", [text]) => {
            validate_forward_ref(ctx.str_value(*text)?)?;
            Ok(NONE)
        }
        // Read the base type descriptor directly, bypassing a custom metaclass's
        // __annotations__ or __dict__. Static types have no annotation storage.
        ("_get_type_annotations", [class]) => match ctx.heap.get(*class) {
            Value::Class { scope, .. } => {
                ctx.class_annotation_attr(*class, *scope, "__annotations__")
            }
            Value::Native(name) if ctx.is_type_object(*class) => Err(Error::new(
                "AttributeError",
                format!("type object '{name}' has no attribute '__annotations__'"),
            )),
            _ => Err(Error::new("TypeError", "expected a type")),
        },
        ("_eval_type_expression", [text, globals, locals]) => {
            ctx.eval_type_expression(*text, *globals, *locals)
        }
        ("get_origin" | "get_args", [v])
            if matches!(ctx.heap.get(*v), Value::Instance { .. })
                && ctx
                    .module_value("typing", "_AnnotatedAlias")
                    .is_some_and(|class| ctx.instance_of(*v, class)) =>
        {
            if operation == "get_origin" {
                let typing = ctx.module("typing")?;
                ctx.lookup_attr(typing, "Annotated")
            } else {
                let origin = ctx.lookup_attr(*v, "__origin__")?;
                // Keep the origin rooted across a metadata descriptor lookup.
                let args = ctx.alloc(Value::Tuple(vec![origin]));
                let metadata = ctx.lookup_attr(*v, "__metadata__")?;
                ctx.op("+", args, metadata, NONE)
            }
        }
        ("get_origin", [v]) => Ok(match ctx.heap.get(*v) {
            Value::ParamSpecMarker(_, origin) => *origin,
            Value::Instance { .. }
                if ctx
                    .module_value("typing", "_SpecialGenericAlias")
                    .is_some_and(|class| ctx.instance_of(*v, class)) =>
            {
                ctx.lookup_attr(*v, "__origin__")?
            }
            Value::GenericAlias(a) => a.origin,
            Value::Union(_) => super::union::class(ctx),
            Value::Class { .. } if ctx.classes.get("typing.Generic") == Some(v) => *v,
            Value::Native(name) if name == "typing.Callable" => {
                super::mapping_abc::class(ctx, "Callable")?
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
                let (result, params) = args
                    .split_last()
                    .ok_or_else(|| Error::new("IndexError", "tuple index out of range"))?;
                if matches!(params, [param] if is_param_expr(ctx, *param)) {
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
        ("Union.__origin__", [receiver]) => {
            ctx.read_member("typing.Union", "__origin__", *receiver)
        }
        ("Union.__parameters__", [receiver]) => {
            ctx.read_member("typing.Union", "__parameters__", *receiver)
        }
        ("Union.__mro_entries__", [id, _]) if matches!(ctx.heap.get(*id), Value::Union(_)) => {
            Err(Error::new(
                "TypeError",
                format!("Cannot subclass {}", ctx.checked_format(*id, true)?),
            ))
        }
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
    /// Validate the resolved bases of an explicit Protocol declaration as in CPython 3.14.
    pub(super) fn validate_protocol_bases(&mut self, bases: &[Id]) -> Result<()> {
        let generic = self.classes["typing.Generic"];
        let object = self.builtin("object");
        for &base in bases {
            if base == object || base == generic {
                continue;
            }
            let module = self
                .own_class_attr(base, "__module__")
                .and_then(|module| self.str_value(module).ok());
            if matches!(
                (module, self.class_name(base)),
                (
                    Some("collections.abc"),
                    "Callable"
                        | "Awaitable"
                        | "Iterable"
                        | "Iterator"
                        | "AsyncIterable"
                        | "AsyncIterator"
                        | "Hashable"
                        | "Sized"
                        | "Container"
                        | "Collection"
                        | "Reversible"
                        | "Buffer"
                ) | (
                    Some("contextlib"),
                    "AbstractContextManager" | "AbstractAsyncContextManager"
                ) | (Some("io"), "Reader" | "Writer")
                    | (Some("os"), "PathLike")
            ) {
                continue;
            }
            if self.subclass(base, generic)
                && let Some(marked) = self.class_attr(base, "_is_protocol")
                && self.truth(marked)?
            {
                continue;
            }
            return Err(Error::new(
                "TypeError",
                format!(
                    "Protocols can only inherit from other protocols, got {}",
                    self.checked_format(base, true)?
                ),
            ));
        }
        Ok(())
    }

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

    pub(super) fn call_typing_helper(&mut self, name: &str, args: Vec<Id>) -> Result<Id> {
        let helper = self
            .module_value("typing", name)
            .ok_or_else(|| Error::new("RuntimeError", format!("missing typing.{name}")))?;
        self.call(helper, args, Vec::new())
    }

    /// abc and functools perform internal checks while walking a class hierarchy.
    pub(super) fn check_runtime_protocol(&mut self, class: Id, subclass: bool) -> Result<()> {
        if self.is_protocol_declaration(class) {
            let caller = self
                .heap
                .caller_module_at(0)
                .and_then(|module| self.str_value(module).ok());
            if !matches!(caller, None | Some("abc" | "functools")) {
                self.call_typing_helper(
                    "_check_protocol",
                    vec![class, if subclass { TRUE } else { FALSE }],
                )?;
            }
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
            Value::TypeAlias { .. }
            | Value::TypeVar(_)
            | Value::ParamSpec(_)
            | Value::ParamSpecMarker(_, _)
            | Value::TypeVarTuple { .. }
            | Value::NoDefault => Err(Error::new(
                "TypeError",
                "isinstance() arg 2 must be a type, a tuple of types, or a union",
            )),
            Value::GenericAlias(alias) => Err(alias.kind.class_check_error("isinstance")),
            Value::Native(name) if name == "typing.Any" => Err(Error::new(
                "TypeError",
                "typing.Any cannot be used with isinstance()",
            )),
            Value::Native(name) if name == "typing.Callable" => {
                let origin = super::mapping_abc::class(self, "Callable")?;
                self.python_instance_check(value, origin)
            }
            _ => {
                // An exact instance wins before the class's metaclass can intercept the check.
                if self.heap.get(value).instance_class() == Some(class) {
                    return Ok(true);
                }
                // A declared Protocol uses typing's stricter public check, even when it
                // inherits ABCMeta's native implementation.
                if let Some(check) = self.bound_slot(class, "__instancecheck__")?
                    && !(self.is_protocol_declaration(class)
                        && self.slot(class, "__instancecheck__")
                            == self.builtins.get("abc.__instancecheck__").copied())
                {
                    let result = self.call(check, vec![value], Vec::new())?;
                    return self.truth(result);
                }
                if self.is_protocol_declaration(class) {
                    self.check_runtime_protocol(class, false)?;
                    let actual = self.exact_type(value)?;
                    let reported = self.lookup_attr(value, "__class__")?;
                    if reported != actual {
                        if !matches!(
                            self.builtin_type_name(reported),
                            Some("type" | "typing._AnyMeta")
                        ) {
                            return Err(Error::new(
                                "TypeError",
                                "issubclass() arg 1 must be a class",
                            ));
                        }
                        if self.abc_subclass_check(reported, class)? {
                            return Ok(true);
                        }
                    }
                    if self.abc_subclass_check(actual, class)? {
                        return Ok(true);
                    }
                    let matched =
                        self.call_typing_helper("_protocol_instance", vec![class, value])?;
                    return self.truth(matched);
                }
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

    /// ABCMeta checks the instance's reported class first, then its actual type if different.
    pub(super) fn abc_meta_instance_check(&mut self, value: Id, class: Id) -> Result<Id> {
        super::mapping_abc::check_impl(self, class)?;
        let reported = match self.lookup_attr(value, "__class__") {
            Ok(reported) => reported,
            // A few native values can be called before the runtime can expose their type.
            Err(error)
                if error.kind == "NotImplementedError"
                    && self.classes.get("collections.abc.Callable") == Some(&class)
                    && self.heap.get(value).instance_class().is_none()
                    && self.builtin_type_name(value).is_none() =>
            {
                return Ok(if self.callable(value) { TRUE } else { FALSE });
            }
            Err(error) => return Err(error),
        };
        self.heap.keep(reported);
        let actual = self.exact_type(value)?;
        let check = |ctx: &mut Self, child| {
            if ctx.heap.get(class).instance_class().is_some()
                || ctx.class_attr(class, "__subclasscheck__").is_some()
            {
                let method = ctx.lookup_attr(class, "__subclasscheck__")?;
                return ctx.call(method, vec![child], Vec::new());
            }
            // Synthetic protocol declarations have no stored metaclass.
            ctx.abc_meta_subclass_check(child, class)
        };
        let result = check(self, reported)?;
        if reported != actual && !self.truth(result)? {
            check(self, actual)
        } else {
            Ok(result)
        }
    }

    /// The base ABC hook bypasses typing's public protocol guards and custom metaclass hooks.
    pub(super) fn abc_meta_subclass_check(&mut self, child: Id, class: Id) -> Result<Id> {
        if !self.is_type_object(child) {
            return Err(Error::new(
                "TypeError",
                "issubclass() arg 1 must be a class",
            ));
        }
        super::mapping_abc::check_impl(self, class)?;
        self.abc_subclass_check(child, class)
            .map(super::builtins::boolean)
    }

    /// Identify the marker substituted by typing.NamedTuple.__mro_entries__.
    pub(super) fn namedtuple_base(&self, bases: &[Id]) -> Option<Id> {
        self.builtins
            .get("typing.NamedTuple")
            .copied()
            .filter(|marker| bases.contains(marker))
    }

    /// Freeze the fields of a new declaration and replace its marker with the actual tuple base.
    fn namedtuple_layout(
        &mut self,
        name: &str,
        bases: &mut [Id],
        scope: Id,
        namespace: Option<Id>,
    ) -> Result<Option<std::rc::Rc<[String]>>> {
        if let Some(marker) = self.namedtuple_base(bases) {
            let generic = self.classes.get("typing.Generic").copied();
            if bases
                .iter()
                .any(|&base| base != marker && Some(base) != generic)
            {
                return Err(Error::new(
                    "TypeError",
                    "can only inherit from a NamedTuple type and Generic",
                ));
            }
            let tuple = self.builtin("tuple");
            for base in bases.iter_mut() {
                if *base == marker {
                    *base = tuple;
                }
            }
            let explicit = self.scope_name(scope, "__annotations__");
            let (fields, provider) = if explicit.is_none()
                && let Some(original) = self.scope_name(scope, "__annotate_func__")
            {
                let annotate = self.call_typing_helper("_namedtuple_annotate", vec![original])?;
                self.heap.keep(annotate);
                let forwardref = self.int(3);
                let fields = self.call(annotate, vec![forwardref], Vec::new())?;
                (Some(fields), Some(annotate))
            } else {
                (explicit, None)
            };
            let mut named_fields = Vec::new();
            if let Some(fields) = fields {
                self.heap.keep(fields);
                let keys = self.items(fields)?;
                for &key in &keys {
                    self.heap.keep(key);
                }
                for key in keys {
                    named_fields.push(super::namedtuple::string(self, key)?);
                }
            }
            let annotate = if let Some(provider) = provider {
                provider
            } else {
                let annotate = self
                    .call_typing_helper("_make_eager_annotate", explicit.into_iter().collect())?;
                self.heap.keep(annotate);
                if explicit.is_some() {
                    let key = self.heap.code_name("__annotations__");
                    self.remove_binding(scope, "__annotations__", key)?;
                }
                self.enable_class_annotation_provider(scope);
                annotate
            };
            let mut defaults = Vec::new();
            for field in &named_fields {
                if let Some(value) = self.scope_name(scope, field) {
                    defaults.push(value);
                } else if !defaults.is_empty() {
                    return Err(Error::new(
                        "TypeError",
                        format!("non-default field {field} cannot follow default fields"),
                    ));
                }
            }
            super::namedtuple::validate_fields(name, &named_fields, false)?;
            if let Some(namespace) = namespace {
                // The factory installs user members one at a time after creating its tuple type.
                let entries = self
                    .dict_entries(namespace)
                    .expect("NamedTuple declaration")
                    .to_vec();
                for (key, _) in entries {
                    if let Ok(attr) = self.str_value(key)
                        && !matches!(
                            attr,
                            "__module__"
                                | "__name__"
                                | "__annotations__"
                                | "__annotate__"
                                | "__annotate_func__"
                                | "__annotations_cache__"
                        )
                    {
                        let attr = attr.to_owned();
                        self.remove_binding(scope, &attr, key)?;
                    }
                }
            }
            if generic.is_some_and(|g| bases.contains(&g))
                && self.scope_name(scope, "__class_getitem__").is_none()
            {
                let getitem = self
                    .module_value("typing", "_namedtuple_generic_getitem")
                    .ok_or_else(|| {
                        Error::new("RuntimeError", "missing NamedTuple subscription hook")
                    })?;
                self.bind_local(scope, "__class_getitem__", getitem);
            }
            let fields =
                super::namedtuple::bind_field_metadata(self, scope, &named_fields, &defaults);
            let defaults = self.alloc(Value::Tuple(defaults));
            let new =
                super::namedtuple::bind_new(self, scope, name, &named_fields, Some(defaults))?;
            self.bind_local(scope, "__annotate_func__", annotate);
            self.set_attr(new, "__annotate__", annotate)?;
            if self.scope_name(scope, "__repr__").is_none() {
                let name = self.text(name);
                super::namedtuple::bind_repr(self, scope, name, fields)?;
            }
            super::namedtuple::bind_fields(self, scope, &named_fields);
            return Ok(Some(named_fields.into()));
        }
        Ok(None)
    }

    pub(super) fn declared_slot_names(
        &mut self,
        slots: Option<Id>,
        base: Id,
    ) -> Result<Option<std::rc::Rc<[String]>>> {
        let Some(slots) = slots else {
            return Ok(None);
        };
        let mut items = Vec::new();
        if self.string_storage(slots).is_some() {
            items.push(slots);
        } else {
            let iterator = self.iterator(slots)?;
            self.heap.keep(iterator);
            while let Some(slot) = self.next_item(iterator)? {
                self.heap.keep(slot);
                items.push(slot);
            }
        }
        Ok(Some(self.checked_slot_names(base, &items)?.into()))
    }

    /// Preserve the actual direct bases as well as the immutable C3 tuple exposed by type.
    pub(super) fn allocate_class(&mut self, name: String, scope: Id, bases: Vec<Id>) -> Result<Id> {
        self.allocate_class_with_slots(name, scope, bases, ClassLayout::Specialized, None)
    }

    pub(super) fn allocate_class_with_slots(
        &mut self,
        name: String,
        scope: Id,
        bases: Vec<Id>,
        layout: ClassLayout,
        namedtuple_namespace: Option<Id>,
    ) -> Result<Id> {
        let mut bases = self.validate_class_bases(bases, layout)?;
        let declared_fields =
            self.namedtuple_layout(&name, &mut bases, scope, namedtuple_namespace)?;
        let ancestors = self.linearized_bases(&bases, false)?;
        let layout_base = self.member_layout_base(&bases)?;
        let slots = self.declared_slot_names(self.scope_name(scope, "__slots__"), layout_base)?;
        let class = self.allocate_prepared_class(name, scope, bases, ancestors, layout, slots)?;
        if let Some(fields) = declared_fields {
            let Value::Class {
                namedtuple_fields, ..
            } = self.heap.get_mut(class)
            else {
                unreachable!()
            };
            *namedtuple_fields = Some(fields);
        }
        Ok(class)
    }

    pub(super) fn validate_class_bases(
        &mut self,
        mut bases: Vec<Id>,
        layout: ClassLayout,
    ) -> Result<Vec<Id>> {
        for &base in &bases {
            if self.classes.get("typing.Union") == Some(&base) {
                return Err(Error::new(
                    "TypeError",
                    "type 'typing.Union' is not an acceptable base type",
                ));
            }
            if matches!(self.heap.get(base), Value::Native(name) if name == "typing._UnpackGenericAlias")
            {
                return Err(Error::new(
                    "TypeError",
                    "Cannot subclass special typing classes",
                ));
            }
            if let Some(name) = self.heap.get(base).hash_type() {
                return Err(Error::new(
                    "TypeError",
                    format!("type '{name}' is not an acceptable base type"),
                ));
            }
            if let Value::Native(name) = self.heap.get(base)
                && (super::stdlib::fixed_structseq::fixed_type(name).is_some()
                    || crate::objects::HashState::is_provider_class(name)
                    || super::builtins::is_final_iterator_type(name)
                    || matches!(
                        name.as_str(),
                        "typing.TypeVar"
                            | "typing.ParamSpec"
                            | "typing.ParamSpecArgs"
                            | "typing.ParamSpecKwargs"
                            | "constevaluator"
                            | "typing.TypeVarTuple"
                            | "typing.TypeAliasType"
                            | "NoDefaultType"
                            | "datetime.timezone"
                            | super::stdlib::flags::TYPE
                            | "bool"
                            | "range"
                            | "memoryview"
                            | "slice"
                            | "function"
                            | "builtin_function_or_method"
                            | "method"
                            | "method-wrapper"
                            | "code"
                            | "frame"
                            | "traceback"
                            | "cell"
                            | "dict_keys"
                            | "dict_values"
                            | "dict_items"
                            | "odict_keys"
                            | "odict_values"
                            | "odict_items"
                            | "coroutine"
                            | "async_generator"
                            | "mappingproxy"
                            | "classmethod_descriptor"
                            | "getset_descriptor"
                            | "method_descriptor"
                            | "wrapper_descriptor"
                            | "NoneType"
                            | "NotImplementedType"
                            | "ellipsis"
                            | super::value::REGEX_PATTERN_TYPE
                            | super::value::REGEX_MATCH_TYPE
                            | super::value::TEMPLATE_ITER_TYPE
                            | super::value::TEMPLATE_TYPE
                            | super::value::INTERPOLATION_TYPE
                            | super::namedtuple::GETTER
                            | super::thread_lock::CLASS
                            | "functools.KeyWrapper"
                            | "member_descriptor"
                    ))
            {
                return Err(Error::new(
                    "TypeError",
                    format!("type '{name}' is not an acceptable base type"),
                ));
            }
        }
        if layout == ClassLayout::TypeNamespace {
            self.check_namespace_layouts(&bases)?;
        }
        if bases.is_empty() {
            bases.push(self.builtin("object"));
        }
        // A mixed layout needs CPython's solid-base check before choosing a native allocator.
        if bases.len() > 1
            && bases
                .iter()
                .any(|&base| self.has_base(base, super::thread_local::CLASS))
        {
            return Err(Error::new(
                "NotImplementedError",
                "multiple bases for a thread-local subclass are unsupported",
            ));
        }
        Ok(bases)
    }

    pub(super) fn allocate_prepared_class(
        &mut self,
        name: String,
        scope: Id,
        bases: Vec<Id>,
        ancestors: Vec<Id>,
        layout: ClassLayout,
        slot_names: Option<std::rc::Rc<[String]>>,
    ) -> Result<Id> {
        let mut mro = Vec::with_capacity(ancestors.len() + 1);
        mro.push(NONE);
        // Keep allocation and special-slot dispatch on their existing native layout.
        // The published MRO includes the declaring classes of inherited exception methods.
        if ancestors.iter().any(|&base| {
            matches!(self.heap.get(base), Value::Native(name) if super::builtins::is_exception(name))
        }) {
            mro.extend(self.linearized_bases(&bases, true)?);
        } else {
            mro.extend_from_slice(&ancestors);
        }
        // Other dunder names can replace special methods or class metadata. Keep
        // their existing layout until their type slots can be updated as well.
        let ordinary_slots = layout != ClassLayout::Specialized
            && slot_names.as_deref().is_some_and(|names| {
                names.iter().all(|name| {
                    !name.starts_with("__")
                        || !name.ends_with("__")
                        || matches!(name.as_str(), "__dict__" | "__weakref__")
                })
            });
        if layout == ClassLayout::TypeNamespace && slot_names.is_some() && !ordinary_slots {
            return Err(Error::new(
                "NotImplementedError",
                "dynamic type() does not support slots that replace special class attributes",
            ));
        }
        let member_slots = ordinary_slots
            || self
                .scope_data(scope)
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.member_slots);
        let members = if member_slots {
            self.prepare_slot_members(
                &name,
                scope,
                slot_names.as_deref().unwrap_or_default(),
                ordinary_slots,
            )?
        } else {
            Vec::new()
        };
        let namedtuple_fields = ancestors
            .iter()
            .find_map(|&base| match self.heap.get(base) {
                Value::Class {
                    namedtuple_fields, ..
                } => namedtuple_fields.clone(),
                _ => None,
            });
        // type.__new__ wraps plain functions only, before the class becomes observable.
        for name in ["__init_subclass__", "__class_getitem__"] {
            if let Some(function) = self.namespace_binding(self.scope_data(scope), name)
                && matches!(self.heap.get(function), Value::Function { .. })
            {
                let dictionary = self.dict(Vec::new());
                let method = self.alloc(Value::ClassMethod {
                    callable: function,
                    dictionary,
                });
                self.bind_local(scope, name, method);
            }
        }
        let class = self.alloc(Value::Class {
            io_kind: None,
            hash_type: None,
            metaclass: None,
            namedtuple_fields,
            name,
            scope,
            ancestors,
        });
        mro[0] = class;
        let bases = self.alloc(Value::Tuple(bases));
        let mro = self.alloc(Value::Tuple(mro));
        // A class-body __eq__ suppresses inherited hashing at type creation.
        if self
            .namespace_binding(self.scope_data(scope), "__eq__")
            .is_some()
            && self
                .namespace_binding(self.scope_data(scope), "__hash__")
                .is_none()
        {
            self.bind_local(scope, "__hash__", NONE);
        }
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        let namespace = fields.class_namespace.get_or_insert_with(Default::default);
        namespace.member_slots = member_slots;
        namespace.slot_names = slot_names;
        namespace.bases = bases;
        namespace.mro = mro;
        for field in members {
            let name = self.text(&field);
            let descriptor = self.alloc(Value::SlotMember { owner: class, name });
            self.bind_local(scope, &field, descriptor);
        }
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
