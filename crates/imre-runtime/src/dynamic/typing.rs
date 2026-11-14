//! Runtime type parameters and class-base aliases used by imported Python modules.

mod builtin_substitution;
mod parameter_value;

pub(super) use parameter_value::{
    LazyEvaluation, ParameterField, ParameterValue, set_parameter_evaluator,
};

use crate::dynamic::value::SlotStorage;
use std::collections::HashSet;

use super::source_code::CONDITIONAL;
use super::value::EMPTY_TUPLE;
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
    Lazy(LazyEvaluation),
    Eager { value: Id, module: Id },
}

#[derive(Clone)]
pub(super) struct TypeVariable {
    pub name: Id,
    pub module: Id,
    pub bound: ParameterValue,
    pub constraints: ParameterValue,
    pub default: ParameterValue,
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

    /// Alias classes that inherit _GenericAlias.__iter__ without _NotIterable.
    pub(super) fn iterates_unpack(self) -> bool {
        matches!(
            self,
            Self::Generic | Self::Literal(_) | Self::Concatenate | Self::Unpack
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
    /// Local name overrides and the creation-time module of an unnamed generic alias.
    pub naming_fields: Option<Id>,
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
            parameters: NONE,
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
pub(super) fn generic_attr_target(
    ctx: &Context,
    mut obj: Id,
    name: &str,
    writing: bool,
) -> Result<Id> {
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
    pub(super) abc_callable_reducer: Option<Id>,
    no_default: Option<Id>,
    no_default_doc: Option<Id>,
}

impl Registry {
    pub(super) fn roots(&self) -> impl Iterator<Item = &Id> {
        self.alias_caches
            .iter()
            .flatten()
            .chain(self.callable_factory.iter())
            .chain(self.abc_callable_reducer.iter())
            .chain(self.no_default.iter())
            .chain(self.no_default_doc.iter())
    }
}

const NO_DEFAULT_DOC: &str = "The type of the NoDefault singleton.";

pub(super) fn no_default(ctx: &mut Context) -> Id {
    if let Some(value) = ctx.typing.no_default {
        return value;
    }
    let value = ctx.alloc(Value::NoDefault);
    ctx.typing.no_default = Some(value);
    ctx.typing.no_default_doc = Some(ctx.text(NO_DEFAULT_DOC));
    value
}

/// A pack iterates over one Unpack alias, exactly as an explicit Unpack[Ts] subscription.
pub(super) fn unpack_parameter(ctx: &mut Context, pack: Id) -> Result<Id> {
    let module = ctx.import_with_builtins("typing")?;
    let unpack = ctx.lookup_attr(module, "Unpack")?;
    ctx.get_item(unpack, pack)
}

/// Check the constructor's declared parameters without invoking tuple subclass overrides.
fn check_type_alias_params(ctx: &mut Context, parameters: Id) -> Result<Id> {
    let items = ctx
        .tuple_items(parameters)?
        .ok_or_else(|| Error::new("TypeError", "type_params must be a tuple"))?;
    if items.is_empty() {
        return Ok(EMPTY_TUPLE);
    }
    let mut default_seen = false;
    for param in items {
        let default = match ctx.heap.get(param) {
            Value::TypeVar(var) => var.default,
            Value::ParamSpec(spec) => spec.default,
            Value::TypeVarTuple { default, .. } => *default,
            _ => {
                let repr = ctx.checked_format(param, true)?;
                return Err(Error::new(
                    "TypeError",
                    format!("Expected a type param, got {repr}"),
                ));
            }
        };
        if !default.has_default(ctx) {
            if default_seen {
                let repr = ctx.checked_format(param, true)?;
                return Err(Error::new(
                    "TypeError",
                    format!("non-default type parameter '{repr}' follows default type parameter"),
                ));
            }
        } else {
            default_seen = true;
        }
    }
    Ok(parameters)
}

/// Wrap each pack in Unpack for __parameters__ and implicit Generic bases.
fn unpack_typevar_tuples(ctx: &mut Context, params: &mut [Id]) -> Result<bool> {
    let mut unpacked = false;
    for param in params {
        if matches!(ctx.heap.get(*param), Value::TypeVarTuple { .. }) {
            let item = unpack_parameter(ctx, *param)?;
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(item);
            *param = item;
            unpacked = true;
        }
    }
    Ok(unpacked)
}

/// Return the declared tuple itself unless a pack requires an unpacked copy.
fn unpacked_parameters(ctx: &mut Context, parameters: Id) -> Result<Id> {
    if let Value::Tuple(items) | Value::NamedTuple { values: items, .. } =
        ctx.heap.get(ctx.scalar_value(parameters))
        && !items
            .iter()
            .any(|&item| matches!(ctx.heap.get(item), Value::TypeVarTuple { .. }))
    {
        return Ok(parameters);
    }
    // A custom typing.Unpack can run Python while expanding a pack.
    ctx.heap.reserve_temporaries(1)?;
    ctx.heap.keep(parameters);
    let mut items = ctx
        .tuple_items(parameters)?
        .expect("type alias parameter tuple");
    if unpack_typevar_tuples(ctx, &mut items)? {
        Ok(ctx.alloc(Value::Tuple(items)))
    } else {
        Ok(parameters)
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
        let members = ctx.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        ctx.bind(scope, "__protocol_attrs__", members);
        let empty = ctx.alloc(Value::Tuple(Vec::new()));
        ctx.bind(scope, "__parameters__", empty);
        ctx.bind(scope, "__slots__", empty);
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
    if key == "typing.Generic" {
        let hook = ctx.builtin("typing.Generic.__init_subclass__");
        ctx.bind(scope, "__init_subclass__", hook);
    }
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

/// Match _Py_make_parameters without invoking tuple-subclass iteration for metadata.
fn collect_builtin_parameters(
    ctx: &mut Context,
    args: Vec<Id>,
    parameters: &mut Vec<Id>,
) -> Result<()> {
    // A lookup can clear a source list and collect its remaining elements.
    ctx.heap.reserve_temporaries(args.len())?;
    for &arg in &args {
        ctx.heap.keep(arg);
    }
    for arg in args {
        if ctx.is_type_object(arg) {
            continue;
        }
        let mark = ctx.heap.temporary_mark();
        let start = parameters.len();
        let members = if matches!(
            ctx.heap.get(arg),
            Value::TypeVar(_) | Value::TypeVarTuple { .. } | Value::ParamSpec(_)
        ) || ctx.optional_attr(arg, "__typing_subst__")?.is_some()
        {
            vec![arg]
        } else if let Some(metadata) = ctx.optional_attr(arg, "__parameters__")? {
            ctx.tuple_items(metadata)?.unwrap_or_default()
        } else {
            let items = if ctx.list_storage(arg).is_some() {
                Some(ctx.items(arg)?)
            } else {
                ctx.tuple_items(arg)?
            };
            if let Some(items) = items {
                ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
                    collect_builtin_parameters(ctx, items, parameters)
                })?;
            }
            Vec::new()
        };
        for member in members {
            if !parameters.contains(&member) {
                parameters.push(member);
            }
        }
        // Drop temporary descriptor results while keeping all newly discovered parameters.
        ctx.heap.release_temporaries_since(mark);
        ctx.heap.reserve_temporaries(parameters.len() - start)?;
        for &member in &parameters[start..] {
            ctx.heap.keep(member);
        }
    }
    Ok(())
}

/// Collect a snapshot, retaining only the tuple or exception across the native traversal.
fn native_parameters(ctx: &mut Context, args: Id, owner: Id) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(owner);
    let result = ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
        let items = ctx
            .tuple_items(args)?
            .expect("generic alias arguments are a tuple");
        let mut parameters = Vec::new();
        collect_builtin_parameters(ctx, items, &mut parameters)?;
        Ok(ctx.alloc(Value::Tuple(parameters)))
    });
    ctx.heap.release_temporaries_since(mark);
    match &result {
        Ok(parameters) => ctx.heap.keep(*parameters),
        Err(error) => {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
        }
    }
    result
}

/// Store the outer lookup's result even if a descriptor reentered this same alias.
pub(super) fn builtin_parameters(ctx: &mut Context, owner: Id, args: Id, cached: Id) -> Result<Id> {
    if cached != NONE {
        return Ok(cached);
    }
    let result = native_parameters(ctx, args, owner);
    let cached = result.as_ref().copied().unwrap_or(NONE);
    match ctx.heap.get_mut(owner) {
        Value::GenericAlias(alias) => alias.parameters = cached,
        Value::Union(union) => union.parameters = cached,
        _ => unreachable!(),
    }
    result
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
    native_parameters(ctx, args, args)
}

/// Typing retries the constructor without caching when building or comparing its key raises TypeError.
fn alias(ctx: &mut Context, kind: AliasKind, origin: Id, args: Vec<Id>) -> Result<Id> {
    let args = ctx.alloc(Value::Tuple(args));
    let (index, make_alias, arguments) = match kind {
        AliasKind::Generic => (0, ctx.builtin("typing._GenericAlias"), vec![origin, args]),
        AliasKind::Callable => {
            let make_alias = ctx
                .typing
                .callable_factory
                .ok_or_else(|| Error::new("RuntimeError", "typing.Callable is not initialized"))?;
            let constructor = ctx.builtin("typing._CallableGenericAlias");
            (1, make_alias, vec![constructor, origin, args])
        }
        _ => {
            let parameters = if kind == AliasKind::AbcCallable {
                NONE
            } else {
                native_parameters(ctx, args, args)?
            };
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
        naming_fields: None,
    }))
}

/// Snapshot an unnamed typing alias's origin module after collecting its parameters.
fn capture_alias_module(ctx: &mut Context, alias: Id, origin: Id) -> Result<()> {
    let module = ctx.lookup_attr(origin, "__module__")?;
    let key = ctx.text("__module__");
    let fields = ctx.dict(vec![(key, module)]);
    let Value::GenericAlias(alias) = ctx.heap.get_mut(alias) else {
        unreachable!()
    };
    alias.naming_fields = Some(fields);
    Ok(())
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
    let (Value::GenericAlias(a), Value::GenericAlias(b)) = (
        ctx.heap.get(ctx.scalar_value(left)),
        ctx.heap.get(ctx.scalar_value(right)),
    ) else {
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
    let variables = collect_type_parameters(ctx, args, false)?;
    let alias = allocate_alias(
        ctx,
        AliasKind::Literal(members),
        origin,
        args,
        variables,
        None,
        true,
    );
    capture_alias_module(ctx, alias, origin)?;
    Ok(alias)
}

/// PEP 585 keeps a tuple supplied to the constructor as the alias's actual arguments.
pub(super) fn builtin_alias(ctx: &mut Context, origin: Id, args: Id) -> Id {
    let args = if matches!(ctx.heap.get(args), Value::Tuple(_)) {
        args
    } else {
        ctx.alloc(Value::Tuple(vec![args]))
    };
    allocate_alias(ctx, AliasKind::Builtin, origin, args, NONE, None, true)
}

/// Native class methods backed by CPython's Py_GenericAlias calling convention.
pub(super) fn generic_alias_method_owner(symbol: &str) -> Option<&'static str> {
    let owner = symbol.strip_suffix(".__class_getitem__")?;
    [
        "BaseExceptionGroup",
        super::defaultdict::CLASS,
        super::stdlib::functools::PARTIAL_CLASS,
        "_contextvars.ContextVar",
        "_contextvars.Token",
    ]
    .into_iter()
    .find(|&name| name == owner)
}

/// Return the underlying PEP 585 alias while preserving the subtype object for callbacks.
pub(super) fn alias_payload(ctx: &Context, obj: Id) -> Option<Id> {
    let payload = ctx.scalar_value(obj);
    (payload != obj && matches!(ctx.heap.get(payload), Value::GenericAlias(_))).then_some(payload)
}

/// Rebuild PEP 585 aliases and their one-shot unpacking iterators through public constructors.
pub(super) fn reduce_builtin_alias(ctx: &mut Context, receiver: Id) -> Result<Id> {
    let (constructor, args) = match ctx.heap.get(ctx.scalar_value(receiver)) {
        Value::GenericAlias(alias) => {
            let (origin, args) = (alias.origin, alias.args);
            if alias.unpacked {
                let plain = builtin_alias(ctx, origin, args);
                let iterator = ctx.iterator(plain)?;
                ctx.heap.keep(iterator);
                (super::builtins::frame_builtin(ctx, "next")?, vec![iterator])
            } else {
                (ctx.exact_type(receiver)?, vec![origin, args])
            }
        }
        Value::GenericAliasIterator(_) => {
            // Looking up iter can advance the receiver through a custom builtins mapping.
            let iter = super::builtins::frame_builtin(ctx, "iter")?;
            let Value::GenericAliasIterator(source) = *ctx.heap.get(receiver) else {
                unreachable!()
            };
            let source = source.unwrap_or_else(|| ctx.heap.packed_tuple(Vec::new(), false));
            (iter, vec![source])
        }
        _ => unreachable!("validated generic alias or iterator"),
    };
    let args = ctx.alloc(Value::Tuple(args));
    Ok(ctx.alloc(Value::Tuple(vec![constructor, args])))
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
        return Ok(allocate_alias(
            ctx,
            AliasKind::AbcCallable,
            origin,
            pair,
            NONE,
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

pub(super) fn typing_alias_helper(ctx: &mut Context, name: &str, args: Vec<Id>) -> Result<Id> {
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
    if let Value::TypeAlias { parameters, .. } = ctx.heap.get(target) {
        if *parameters != EMPTY_TUPLE {
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
        let alias = alias.clone();
        return Some(builtin_substitution::specialize(ctx, target, &alias, index));
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
            || (alias.kind == AliasKind::Unpack
                && matches!(ctx.heap.get(alias.parameters), Value::Tuple(params) if !params.is_empty()))
            || (alias.kind == AliasKind::Generic
                && matches!(ctx.heap.get(alias.origin), Value::Class { .. })))
    {
        let class = ctx.builtin("typing._GenericAlias");
        if let Some(getitem) = ctx.own_class_attr(class, "__getitem__") {
            return Some(ctx.call(getitem, vec![target, index], Vec::new()));
        }
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
    let has_method = matches!(ctx.heap.get(target), Value::Class { .. } | Value::Native(_))
        && ctx.class_attr(target, "__class_getitem__").is_some();
    let kind = match ctx.heap.get(target) {
        _ if has_method => None,
        Value::Class { .. } if ctx.classes.get("typing.Generic") == Some(&target) => {
            Some(AliasKind::Generic)
        }
        Value::Native(n) if n == "typing.Callable" => Some(AliasKind::Callable),
        Value::Native(n)
            if matches!(
                n.as_str(),
                "dict"
                    | "list"
                    | "tuple"
                    | "set"
                    | "frozenset"
                    | "mappingproxy"
                    | "type"
                    | super::value::REGEX_PATTERN_TYPE
                    | super::value::REGEX_MATCH_TYPE
            ) =>
        {
            Some(AliasKind::Builtin)
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
            supported
        }
        _ => None,
    };
    let Some(kind) = kind else {
        if !has_method && !ctx.is_type_object(target) {
            return None;
        }
        return Some(match ctx.optional_attr(target, "__class_getitem__") {
            Ok(None | Some(NONE)) => {
                let name = ctx.class_name(target);
                let name = &name[..name.floor_char_boundary(200)];
                Err(Error::new(
                    "TypeError",
                    format!("type '{name}' is not subscriptable"),
                ))
            }
            Ok(Some(method)) => ctx.call(method, vec![index], Vec::new()),
            Err(error) => Err(error),
        });
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
                let mut seen = HashSet::new();
                if !args.iter().all(|&arg| {
                    let param = match ctx.heap.get(arg) {
                        Value::GenericAlias(alias) => unpack_arg(ctx, alias).unwrap_or(arg),
                        _ => arg,
                    };
                    seen.insert(param)
                }) {
                    return Err(Error::new(
                        "TypeError",
                        format!("Parameters to {name}[...] must all be unique"),
                    ));
                }
            } else if let Some(specialize) = ctx.module_value("typing", "_variadic_class_getitem") {
                let constructor = ctx.builtin("typing._GenericAlias");
                return ctx.call(specialize, vec![target, index, constructor], Vec::new());
            } else {
                // The typing module specializes its IO classes before installing its helper.
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
        if kind == AliasKind::Callable {
            let origin = super::mapping_abc::callable(ctx)?;
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
        let callable = super::mapping_abc::callable(ctx)?;
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

fn generic_init_subclass(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let Some(&cls) = args.first() else {
        return Err(Error::new(
            "TypeError",
            "descriptor '__init_subclass__' of 'typing.Generic' object needs an argument",
        ));
    };
    super::stdlib::datetime::validate_classmethod_receiver(
        ctx,
        "typing.Generic",
        "__init_subclass__",
        cls,
    )?;
    call_generic_helper(
        ctx,
        "_generic_init_subclass",
        args.to_vec(),
        kwargs.to_vec(),
    )
}

fn call_generic_helper(
    ctx: &mut Context,
    name: &str,
    args: Vec<Id>,
    kwargs: Vec<(String, Id)>,
) -> Result<Id> {
    let typing = ctx.import_with_builtins("typing")?;
    let helper = ctx.lookup_attr(typing, name)?;
    ctx.heap.keep(helper);
    ctx.call(helper, args, kwargs)
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
                &["__reduce__", "__typing_subst__", "has_default"],
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
        "types.GenericAlias" => {
            for name in ["__new__", "__reduce__"] {
                let method = ctx.builtin(&format!("{owner}.{name}"));
                entries.push((ctx.text(name), method));
            }
        }
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

/// GenericAlias forwards most names to the origin but keeps its own metadata and reduction.
pub(super) fn subclass_alias_attribute(
    ctx: &mut Context,
    value: Id,
    class: Id,
    name: &str,
) -> Option<Result<Id>> {
    let payload = alias_payload(ctx, value)?;
    let Value::GenericAlias(alias) = ctx.heap.get(payload) else {
        unreachable!()
    };
    match name {
        "__class__" | "__bases__" | "__copy__" | "__deepcopy__" | "__reduce__"
        | "__reduce_ex__" => None,
        "__origin__"
        | "__args__"
        | "__parameters__"
        | "__unpacked__"
        | "__typing_unpacked_tuple_args__"
        | "__mro_entries__" => {
            if ctx.class_attr(class, name).is_some() {
                return None;
            }
            Some(ctx.lookup_attr(payload, name))
        }
        _ => Some(ctx.lookup_attr(alias.origin, name)),
    }
}

pub(super) fn attribute(ctx: &mut Context, value: Id, name: &str) -> Option<Result<Id>> {
    if name != "__name__"
        && matches!(ctx.heap.get(value), Value::Native(class) if class.starts_with("typing.") && !(class == "typing.TypeAliasType" && name == "__module__"))
        && let Some(&dictionary) = ctx.native_type_dicts.get(&value)
        && let Some(member) = ctx.dict_named_item(dictionary, name)
    {
        return Some(Ok(match *ctx.heap.get(member) {
            Value::StaticMethod { callable, .. } => callable,
            _ => member,
        }));
    }
    let attr = match ctx.heap.get(value) {
        Value::Native(class)
            if name == "__iter__"
                && Context::native_base(class) == Some("typing._GenericAlias") =>
        {
            let base = ctx.builtin("typing._GenericAlias");
            ctx.own_class_attr(base, name)?
        }
        Value::Native(class) if name == "__iter__" && class == "typing._CallableGenericAlias" => {
            NONE
        }
        Value::GenericAlias(alias) if name == "__parameters__" && alias.is_builtin_alias() => {
            return Some(builtin_parameters(ctx, value, alias.args, alias.parameters));
        }
        Value::GenericAlias(alias) if name == "__iter__" && alias.kind == AliasKind::Callable => {
            NONE
        }
        Value::GenericAlias(alias) if name == "__iter__" && alias.kind.iterates_unpack() => {
            let class = ctx.builtin(alias.kind.class());
            return Some(
                ctx.lookup_attr(class, name)
                    .and_then(|method| ctx.bind_attribute(method, value, class)),
            );
        }
        Value::Native(class)
            if matches!(name, "__base__" | "__bases__" | "__mro__")
                && (ParameterClass::lookup(class).is_some()
                    || matches!(
                        class.as_str(),
                        "typing.Any"
                            | "typing._AnyMeta"
                            | "typing.TypeAliasType"
                            | "collections.abc._CallableGenericAlias"
                    )) =>
        {
            if name == "__base__" {
                ctx.builtin(Context::native_base(class).unwrap_or("object"))
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
        Value::Native(class)
            if matches!(
                class.as_str(),
                "typing._LiteralGenericAlias" | "typing._ConcatenateGenericAlias"
            ) && name == "__reduce__" =>
        {
            let base = ctx.builtin("typing._GenericAlias");
            return Some(ctx.lookup_attr(base, name));
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
        Value::Native(class)
            if class == "typing.Callable" && matches!(name, "_name" | "__qualname__") =>
        {
            ctx.text("Callable")
        }
        Value::Native(class) if class == "typing.Callable" && name == "__origin__" => {
            return Some(super::mapping_abc::callable(ctx));
        }
        Value::Native(class)
            if name == "__reduce__"
                && matches!(class.as_str(), "typing.Callable" | "typing._CallableType") =>
        {
            let parent = *ctx.classes.get("typing._SpecialGenericAlias")?;
            let reducer = ctx.class_attr(parent, name)?;
            let receiver = if class == "typing.Callable" {
                value
            } else {
                NONE
            };
            let owner = ctx.builtin("typing._CallableType");
            return Some(ctx.bind_attribute(reducer, receiver, owner));
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
            if class == "typing.TypeAliasType" && matches!(name, "__name__" | "__qualname__") =>
        {
            ctx.text("TypeAliasType")
        }
        Value::Native(class) if class == "types.GenericAlias" && name == "__new__" => {
            ctx.builtin("types.GenericAlias.__new__")
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
        Value::Native(class)
            if name == "__reduce__"
                && matches!(
                    class.as_str(),
                    "types.GenericAlias" | "generic_alias_iterator"
                ) =>
        {
            ctx.builtin(&format!("{class}.__reduce__"))
        }
        Value::GenericAliasIterator(_) if name == "__reduce__" => {
            let class = ctx.builtin("generic_alias_iterator");
            let reducer = ctx.builtin("generic_alias_iterator.__reduce__");
            return Some(ctx.bind_attribute(reducer, value, class));
        }
        Value::Native(class)
            if name == "__reduce__" && class == "collections.abc._CallableGenericAlias" =>
        {
            ctx.typing.abc_callable_reducer?
        }
        Value::Native(class) if name == "__reduce__" && class == "typing._CallableGenericAlias" => {
            ctx.module_value("typing", "_reduce_callable_alias")?
        }
        Value::GenericAlias(alias) if name == "__reduce__" => {
            let class = ctx.builtin(alias.kind.class());
            return Some(
                ctx.lookup_attr(class, name)
                    .and_then(|reducer| ctx.bind_attribute(reducer, value, class)),
            );
        }
        Value::NoDefault if name == "__doc__" => ctx.typing.no_default_doc?,
        Value::NoDefault if name == "__reduce__" => {
            let callable = ctx.builtin("typing.NoDefault.__reduce__");
            ctx.alloc(Value::Method {
                callable,
                receiver: value,
                explicit: false,
            })
        }
        Value::Native(class) if class == "NoDefaultType" && name == "__doc__" => {
            ctx.text(NO_DEFAULT_DOC)
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
            "__default__" => return Some(default.read(ctx, value, ParameterField::Default)),
            "evaluate_default" => default.evaluator(*declared, ParameterField::Default, ctx),
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
            parameters,
            definition,
        } => match (name, *definition) {
            ("__name__", _) => *label,
            ("__type_params__", _) => *parameters,
            ("__parameters__", _) => return Some(unpacked_parameters(ctx, *parameters)),
            ("__module__", AliasDefinition::Eager { module, .. }) => module,
            ("__module__", AliasDefinition::Lazy(lazy)) => {
                return Some(ctx.lookup_attr(lazy.evaluator, "__module__"));
            }
            ("evaluate_value", AliasDefinition::Eager { value, .. }) => {
                ctx.alloc(Value::ConstEvaluator(value))
            }
            ("evaluate_value", AliasDefinition::Lazy(lazy)) => lazy.evaluator,
            ("__value__", AliasDefinition::Eager { value, .. }) => value,
            ("__value__", AliasDefinition::Lazy(lazy)) => return Some(lazy.read(ctx, value, None)),
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
            "evaluate_bound" => var
                .bound
                .evaluator(var.declared, ParameterField::Bound, ctx),
            "evaluate_constraints" => {
                var.constraints
                    .evaluator(var.declared, ParameterField::Constraints, ctx)
            }
            "__name__" => var.name,
            "__module__" => var.module,
            "__bound__" => return Some(var.bound.read(ctx, value, ParameterField::Bound)),
            "__constraints__" => {
                return Some(
                    var.constraints
                        .read(ctx, value, ParameterField::Constraints),
                );
            }
            "__default__" => return Some(var.default.read(ctx, value, ParameterField::Default)),
            "evaluate_default" => var
                .default
                .evaluator(var.declared, ParameterField::Default, ctx),
            "__reduce__" | "__typing_subst__" | "has_default" => {
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
        Value::GenericAlias(a)
            if matches!(name, "__module__" | "__name__" | "__qualname__")
                && let Some(fields) = a.naming_fields =>
        {
            if let Some(local) = ctx.dict_named_item(fields, name) {
                local
            } else if name == "__module__" {
                let class = ctx.builtin(a.kind.class());
                return Some(ctx.lookup_attr(class, name));
            } else {
                let origin = a.origin;
                return Some(
                    ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
                        ctx.lookup_attr(origin, "__name__")
                    }),
                );
            }
        }
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
                class.supplied_bases.unwrap_or(class.bases)
            }
        }
        _ => return None,
    };
    Some(Ok(attr))
}

/// Create a compiler-declared pack before binding its optional default evaluator.
pub(super) fn declared_pack(ctx: &mut Context, name: Id) -> Id {
    let module = ctx.text("typing");
    let default = no_default(ctx);
    ctx.alloc(Value::TypeVarTuple {
        name,
        module,
        default: ParameterValue::Eager(default),
        declared: true,
    })
}

/// Create the lexical scope for a declaration's type parameters.
pub(super) fn parameter_scope(ctx: &mut Context, outer: Id, names: Id, class: bool) -> Result<Id> {
    let enclosing = ctx.scope_data(outer);
    let in_class = enclosing.in_class_body;
    let parent = if in_class {
        enclosing.parent.unwrap_or(outer)
    } else {
        outer
    };
    let module = enclosing.module.clone();
    let scope = ctx.scope(&module, Some(parent));
    if in_class && !class {
        capture_parameter_class(ctx, scope, outer, names);
    }
    initialize_parameters(ctx, scope, names, class)
}

/// A delayed parameter expression can read the enclosing class dictionary after the
/// class body has finished, even though ordinary lexical lookup skips that body.
fn capture_parameter_class(ctx: &mut Context, scope: Id, outer: Id, names: Id) {
    let class_cell = ctx.scope_data(outer).class_cell;
    let conditional = ctx
        .scope_data(outer)
        .conditional_annotation_cell()
        .filter(|_| ctx.dict_named_item(names, CONDITIONAL).is_none());
    let dictionary = ctx
        .dict_named_item(names, "__classdict__")
        .is_none()
        .then(|| ctx.class_annotation_dict_cell(outer));
    let Value::Scope(parameters) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    for (name, cell) in [("__classdict__", dictionary), (CONDITIONAL, conditional)] {
        if let Some(cell) = cell {
            parameters.closure.cells.insert(name.to_owned(), cell);
        }
    }
    parameters.class_cell = class_cell;
}

/// Bind the declared parameters directly in the compiled generic class header's activation.
pub(super) fn class_parameter_frame(ctx: &mut Context, scope: Id, names: Id) -> Result<Id> {
    let outer = ctx.scope_data(scope).parent.expect("generic class header");
    if ctx.scope_data(outer).in_class_body {
        let parent = ctx.scope_data(outer).parent;
        let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.parent = parent;
        capture_parameter_class(ctx, scope, outer, names);
    }
    initialize_parameters(ctx, scope, names, true)
}

fn initialize_parameters(ctx: &mut Context, scope: Id, names: Id, class: bool) -> Result<Id> {
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
    let declared = ctx.alloc(Value::Tuple(params));
    ctx.bind(scope, ".type_params", declared);
    let Value::Scope(frame) = ctx.heap.get_mut(scope) else {
        unreachable!()
    };
    let frame = frame.as_mut();
    frame
        .closure
        .declared
        .extend(frame.names.keys().map(|name| name.as_str().to_owned()));
    frame.closure.declared_complete = true;
    if class {
        // __type_params__ retains the pack itself; the implicit base is Generic[*Ts].
        let params = unpacked_parameters(ctx, declared)?;
        let generic = named_class(ctx, "typing", "Generic");
        let base = call_generic_helper(ctx, "_GenericAlias", vec![generic, params], Vec::new())?;
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
        bound: ParameterValue::Eager(bound),
        constraints: ParameterValue::Eager(constraints),
        default: ParameterValue::Eager(default),
        covariant,
        contravariant,
        infer_variance,
        declared,
    })))
}

/// Finalize a temporary annotation name in place, including references retained by
/// a user callback or inside a previously constructed typing alias.
fn finish_field_ref(ctx: &mut Context, value: Id) -> Result<Id> {
    let invalid = || Error::new("TypeError", "expected a TypedDict annotation symbol");
    let Some(source) = ctx.module_value("annotationlib", "_TypedDictSymbol") else {
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
    if operation == "Generic.__init_subclass__" {
        return generic_init_subclass(ctx, args, kwargs);
    }
    if matches!(
        operation,
        "TypeVar.__reduce__"
            | "TypeVar.__typing_subst__"
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
            ("__reduce__", 0) => return ctx.lookup_attr(parameter, "__name__"),
            ("__reduce__", n) => {
                return Err(Error::new(
                    "TypeError",
                    format!("{operation}() takes no arguments ({n} given)"),
                ));
            }
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
    if let ("_no_type_check_member", [class, attr], []) = (operation, args, kwargs) {
        let name = ctx.str_value(*attr)?.to_owned();
        return ctx.no_type_check_member(*class, &name);
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
            default: ParameterValue::Eager(default),
            declared: false,
        }));
    }
    if operation == "TypeVar" {
        let module = ctx.heap.caller_module();
        return typevar(ctx, args, kwargs, module, false);
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
        let result = allocate_alias(ctx, kind, origin, args, parameters, name, instantiable);
        if matches!(kind, AliasKind::Generic | AliasKind::Concatenate)
            && name.is_none_or(|name| ctx.str_value(name).is_ok_and(str::is_empty))
        {
            capture_alias_module(ctx, result, origin)?;
        }
        return Ok(result);
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
        let parameters = check_type_alias_params(ctx, params.unwrap_or(EMPTY_TUPLE))?;
        let module = ctx.heap.caller_module();
        return Ok(ctx.alloc(Value::TypeAlias {
            name,
            parameters,
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
        ("TypeVar.has_default", [var]) if let Value::TypeVar(var) = ctx.heap.get(*var) => {
            Ok(super::builtins::boolean(var.default.has_default(ctx)))
        }
        ("TypeVar.has_default", _) => Err(Error::new(
            "TypeError",
            "TypeVar.has_default() takes no arguments",
        )),
        ("ParamSpec.has_default", [spec]) if let Value::ParamSpec(p) = ctx.heap.get(*spec) => {
            Ok(super::builtins::boolean(p.default.has_default(ctx)))
        }
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
            Ok(super::builtins::boolean(default.has_default(ctx)))
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
            if matches!(
                ctx.heap.get(ctx.scalar_value(*text)),
                Value::SurrogateStr(_)
            ) {
                ctx.encode_utf8_codepoints(*text, false, "strict")?;
            }
            ctx.check_annotation(*text, false)?;
            Ok(NONE)
        }
        ("_starred_forward_ref", [original, rendered]) => {
            let source = ctx.str_value(*rendered)?;
            let entry = match ctx.heap.linked_sources.get(original) {
                Some(&super::LinkedSource::Expression(entry))
                    if ctx.str_value(*original)? == source
                        && imre_ast::forward_refs::is_starred(source) =>
                {
                    Some(entry)
                }
                _ => None,
            };
            let spelling = imre_ast::forward_refs::starred_expression(source);
            let expression = ctx.text(spelling);
            // Only the spelling produced from this particular checked string inherits its body.
            if let Some(entry) = entry {
                ctx.heap
                    .linked_sources
                    .insert(expression, super::LinkedSource::Expression(entry));
            }
            Ok(expression)
        }
        ("_validate_forward_ref", [text]) => {
            if matches!(
                ctx.heap.linked_sources.get(text),
                Some(super::LinkedSource::Expression(_))
            ) {
                return Ok(NONE);
            }
            ctx.check_annotation(*text, true)?;
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
        ("_install_any_namespace", [template]) => {
            let Value::Class { scope, .. } = *ctx.heap.get(*template) else {
                return Err(Error::new("TypeError", "expected a class"));
            };
            let class = ctx.builtin("typing.Any");
            let dictionary = ctx.class_namespace_dict(scope, false);
            // The constructor's super() and instance layout descriptors belong to the public class.
            if let Some(cell) = ctx.scope_data(scope).class_cell {
                ctx.heap.replace_cell(cell, Some(class));
            }
            for name in ["__dict__", "__weakref__"] {
                let key = ctx.heap.code_name(name);
                let descriptor = ctx.alloc(Value::LayoutDescriptor(class, name));
                ctx.put_item(dictionary, key, descriptor)?;
            }
            ctx.native_type_dicts.insert(class, dictionary);
            Ok(class)
        }
        // Keep the source method (and its defining globals) on the native alias class.
        ("_install_generic_alias_iter", [methods]) => {
            let class = ctx.builtin("typing._GenericAlias");
            let dictionary = *ctx.native_type_dicts.get(&class).ok_or_else(|| {
                Error::new(
                    "RuntimeError",
                    "missing native typing._GenericAlias dictionary",
                )
            })?;
            for name in ["__iter__", "__getitem__"] {
                if ctx.dict_named_item(dictionary, name).is_none() {
                    let method = ctx.lookup_attr(*methods, name)?;
                    let key = ctx.text(name);
                    ctx.put_item(dictionary, key, method)?;
                }
            }
            Ok(class)
        }
        ("_clear_alias_caches", []) => {
            for cache in ctx.typing.alias_caches.into_iter().flatten() {
                let clear = ctx.lookup_attr(cache, "cache_clear")?;
                ctx.call(clear, Vec::new(), Vec::new())?;
            }
            Ok(NONE)
        }
        ("_get_alias_kind", [value]) => Ok(match ctx.heap.get(*value) {
            Value::GenericAlias(alias) => {
                let kind = match alias.kind {
                    AliasKind::Builtin | AliasKind::AbcCallable => "builtin",
                    AliasKind::Generic | AliasKind::Callable | AliasKind::Literal(_) => "typing",
                    AliasKind::Concatenate => "concatenate",
                    AliasKind::Unpack => "unpack",
                };
                ctx.text(kind)
            }
            _ => NONE,
        }),
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
            Value::Native(name) if name == "typing.Callable" => super::mapping_abc::callable(ctx)?,
            _ => NONE,
        }),
        ("get_args", [v]) => Ok(match ctx.heap.get(*v) {
            Value::GenericAlias(a)
                if ctx.classes.get("collections.abc.Callable") == Some(&a.origin) =>
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
                && let Some(marked) = self.optional_attr(base, "_is_protocol")?
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
                    && self.class_attr(class, "_is_protocol") == Some(TRUE)
                    && self.subclass(class, protocol)
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
                let origin = super::mapping_abc::callable(self)?;
                self.python_instance_check(value, origin)
            }
            _ => {
                // An exact instance wins before the class's metaclass can intercept the check.
                let instance = self.heap.get(value);
                if instance.instance_class() == Some(class) {
                    return Ok(true);
                }
                // Plain builtin values keep their type in the value variant. Compare the
                // canonical class handle so a user class with the same name still dispatches.
                if matches!(
                    instance,
                    Value::None
                        | Value::Bool(_)
                        | Value::Int(_)
                        | Value::Float(_)
                        | Value::Complex(_)
                        | Value::Str(_)
                        | Value::SurrogateStr(_)
                        | Value::Bytes(_)
                        | Value::ByteArray(_)
                        | Value::List(_)
                        | Value::Tuple(_)
                        | Value::Dict(_)
                        | Value::Set { .. }
                ) && self
                    .builtin_type_name(value)
                    .and_then(|name| self.builtins.get(name))
                    == Some(&class)
                {
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
                if alias_payload(self, class).is_some() {
                    return Err(AliasKind::Builtin.class_check_error("isinstance"));
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
                if !self.is_type_object(class)
                    && self
                        .optional_attr(class, "__bases__")?
                        .is_none_or(|bases| self.tuple_elements(bases).is_none())
                {
                    return Err(Error::new(
                        "TypeError",
                        "isinstance() arg 2 must be a type, a tuple of types, or a union",
                    ));
                }
                if self.instance_of(value, class) {
                    return Ok(true);
                }
                // Fixed native values cannot override their reported class.
                let Some(actual) = self.heap.get(value).instance_class() else {
                    return Ok(false);
                };
                if !self.is_type_object(class) {
                    return Ok(false);
                }
                match self.lookup_attr(value, "__class__") {
                    Ok(reported) => Ok(reported != actual
                        && self.is_type_object(reported)
                        && self.direct_subclass(reported, class)),
                    Err(error) if self.is_attribute_error(&error) => Ok(false),
                    Err(error) => Err(error),
                }
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
            let new = super::namedtuple::bind_new(
                self,
                scope,
                name,
                &named_fields,
                fields,
                Some(defaults),
            )?;
            self.bind_local(scope, "__annotate_func__", annotate);
            self.set_attr(new, "__annotate__", annotate)?;
            let name = self.text(name);
            if self.scope_name(scope, "__repr__").is_none() {
                super::namedtuple::bind_repr(self, scope, name, fields)?;
            }
            super::namedtuple::bind_replace(self, scope, name, fields)?;
            super::namedtuple::bind_make(self, scope, name, fields)?;
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
            .map(|(class, _)| class)
    }

    pub(super) fn allocate_class_with_slots(
        &mut self,
        name: String,
        scope: Id,
        mut bases: Vec<Id>,
        layout: ClassLayout,
        namedtuple_namespace: Option<Id>,
    ) -> Result<(Id, Id)> {
        let declared_fields =
            self.namedtuple_layout(&name, &mut bases, scope, namedtuple_namespace)?;
        let bases = self.validate_class_bases(bases, layout)?;
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
        Ok((class, layout_base))
    }

    pub(super) fn validate_class_bases(
        &mut self,
        mut bases: Vec<Id>,
        layout: ClassLayout,
    ) -> Result<Vec<Id>> {
        for &base in &bases {
            if !self.is_type_object(base) {
                return Err(Error::new("TypeError", "bases must be types"));
            }
            for name in ["typing.Union", super::frame_locals::CLASS] {
                if self.classes.get(name) == Some(&base) {
                    return Err(Error::new(
                        "TypeError",
                        format!("type '{name}' is not an acceptable base type"),
                    ));
                }
            }
            if matches!(self.heap.get(base), Value::Native(name) if name == "typing._UnpackGenericAlias")
            {
                return Err(Error::new(
                    "TypeError",
                    "Cannot subclass special typing classes",
                ));
            }
            if let Value::Class {
                heap_type: Some(name),
                scope,
                ..
            } = self.heap.get(base)
                && !self
                    .scope_data(*scope)
                    .class_namespace
                    .as_ref()
                    .is_some_and(|ns| ns.subclassable_heap_type)
            {
                return Err(Error::new(
                    "TypeError",
                    format!("type '{name}' is not an acceptable base type"),
                ));
            }
            if let Value::Native(name) = self.heap.get(base)
                && (super::stdlib::fixed_structseq::fixed_type(name).is_some()
                    || super::stdlib::structseq::layout(name).is_some()
                    || crate::objects::HashState::is_provider_class(name)
                    || (cfg!(target_os = "linux") && name == "select.epoll")
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
                            | "_contextvars.Context"
                            | "_contextvars.ContextVar"
                            | "_contextvars.Token"
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
                            | super::value::ZSTD_COMPRESS_TYPE
                            | super::value::ZSTD_DECOMPRESS_TYPE
                            | super::value::BZ2_COMPRESS_TYPE
                            | super::value::BZ2_DECOMPRESS_TYPE
                            | super::value::LZMA_COMPRESS_TYPE
                            | super::value::LZMA_DECOMPRESS_TYPE
                            | super::value::REGEX_PATTERN_TYPE
                            | super::value::REGEX_MATCH_TYPE
                            | super::value::REGEX_SCANNER_TYPE
                            | super::value::TEMPLATE_ITER_TYPE
                            | super::value::TEMPLATE_TYPE
                            | super::value::INTERPOLATION_TYPE
                            | super::namedtuple::GETTER
                            | super::thread_handle::CLASS
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
        if let Some(public) =
            self.exception_mro(&bases, &ancestors, |ctx, base| ctx.base_lineage(base, true))?
        {
            mro.extend(public);
        } else {
            mro.extend_from_slice(&ancestors);
        }
        // Calls already resolve __call__ through descriptor binding. Other dunder
        // names can replace special methods or class metadata with their own layout.
        let module_layout = slot_names.as_deref().is_some_and(|slots| !slots.is_empty())
            && ancestors.iter().any(
                |&base| matches!(self.heap.get(base), Value::Native(name) if name == "module"),
            );
        let ordinary_slots = (layout != ClassLayout::Specialized || module_layout)
            && slot_names.as_deref().is_some_and(|names| {
                names.iter().all(|name| {
                    !name.starts_with("__")
                        || !name.ends_with("__")
                        || matches!(name.as_str(), "__call__" | "__dict__" | "__weakref__")
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
            self.prepare_slot_members(&name, scope, slot_names.as_deref().unwrap_or_default())?
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
        let pattern_flags = mro
            .iter()
            .skip(1)
            .map(|&base| self.class_pattern_flags(base))
            .find(|flags| !flags.is_empty())
            .unwrap_or_default();
        let class = self.alloc(Value::Class {
            pattern_flags,
            io_kind: None,
            heap_type: None,
            metaclass: None,
            namedtuple_fields,
            name,
            scope,
            ancestors,
        });
        mro[0] = class;
        let supplied_bases = self
            .scope_data(scope)
            .class_namespace
            .as_ref()
            .and_then(|ns| ns.supplied_bases);
        // Layout operations use raw positions without dispatching to a tuple subclass.
        let bases = supplied_bases
            .map(|tuple| self.scalar_value(tuple))
            .filter(|&storage| matches!(self.heap.get(storage), Value::Tuple(_)))
            .unwrap_or_else(|| self.alloc(Value::Tuple(bases)));
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
        let original_doc = self
            .namespace_binding(self.scope_data(scope), "__doc__")
            .and_then(|doc| self.string_storage(doc))
            .filter(|&doc| {
                self.str_value(doc)
                    .is_ok_and(|text| text.contains(super::value::TEXT_SIGNATURE_END))
            });
        let Value::Scope(fields) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        let namespace = fields.class_namespace.get_or_insert_with(Default::default);
        namespace.original_doc = original_doc;
        namespace.member_slots = member_slots;
        namespace.slot_names = slot_names;
        namespace.bases = bases;
        namespace.supplied_bases = supplied_bases.filter(|&tuple| tuple != bases);
        namespace.mro = mro;
        let module_start = if module_layout && !members.is_empty() {
            Some(self.module_slot_start(class)?)
        } else {
            None
        };
        for (index, field) in members.into_iter().enumerate() {
            let name = self.text(&field);
            let descriptor = self.alloc(Value::SlotMember {
                owner: class,
                name,
                storage: module_start.map_or(SlotStorage::Instance, |start| {
                    SlotStorage::Module(start + index)
                }),
            });
            self.bind_local(scope, &field, descriptor);
        }
        Ok(class)
    }
}
