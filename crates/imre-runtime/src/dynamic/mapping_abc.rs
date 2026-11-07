//! Bootstrap the Callable ABC and maintain virtual ABC registrations.
use super::{Context, Error, FALSE, Id, NONE, Result, Value};

/// Typing can request the Callable origin before the Python collections ABCs load.
pub(super) fn callable(ctx: &mut Context) -> Result<Id> {
    const KEY: &str = "collections.abc.Callable";
    if let Some(&class) = ctx.classes.get(KEY) {
        return Ok(class);
    }
    let scope = ctx.scope("collections.abc", None);
    let module = ctx.text("collections.abc");
    ctx.bind(scope, "__module__", module);
    let qualname = ctx.text("Callable");
    ctx.bind(scope, "__qualname__", qualname);
    let slots = ctx.alloc(Value::Tuple(Vec::new()));
    ctx.bind(scope, "__slots__", slots);
    let alias = ctx.builtin("collections.abc._CallableGenericAlias");
    let method = super::method_decorator::class_method(ctx, alias)?;
    ctx.bind(scope, "__class_getitem__", method);
    let function = ctx.builtin("abc.hook.Callable.__subclasshook__");
    let hook = super::method_decorator::class_method(ctx, function)?;
    ctx.bind(scope, "__subclasshook__", hook);
    let method = ctx.builtin("abc.abstract.Callable.__call__");
    ctx.bind(scope, "__call__", method);
    let meta = ctx.abc_meta();
    let result = ctx.allocate_class("Callable".into(), scope, Vec::new())?;
    let Value::Class { metaclass, .. } = ctx.heap.get_mut(result) else {
        unreachable!()
    };
    *metaclass = Some(meta);
    ctx.classes.insert(KEY.into(), result);
    ctx.init_abstract_class(result, scope)?;
    Ok(result)
}

/// A subclass of a collections ABC inherits its metaclass's registration method.
pub(super) fn is_abc_class(ctx: &Context, cls: Id) -> bool {
    let Value::Class { ancestors, .. } = ctx.heap.get(cls) else {
        return false;
    };
    ctx.classes.iter().any(|(name, &abc)| {
        name.starts_with("collections.abc.") && (cls == abc || ancestors.contains(&abc))
    })
}

/// Follow virtual and nominal bases without inserting registered classes into a C3 MRO.
pub(super) fn registered_subclass(ctx: &Context, cls: Id, parent: Id) -> bool {
    if ctx.heap.abc_registrations.is_empty() || !matches!(ctx.heap.get(parent), Value::Class { .. })
    {
        return false;
    }
    let mut classes = vec![cls];
    let mut index = 0;
    while let Some(&current) = classes.get(index) {
        index += 1;
        for &[owner, member] in &ctx.heap.abc_registrations {
            if ctx.direct_subclass(current, member) && !classes.contains(&owner) {
                if ctx.direct_subclass(owner, parent) {
                    return true;
                }
                classes.push(owner);
            }
        }
    }
    false
}

/// Resolve explicit registry overrides before using an ABC's native registry.
pub(super) fn check_impl(ctx: &mut Context, class: Id) -> Result<()> {
    if ctx.class_attr(class, "_abc_impl").is_some()
        || (!ctx.is_bundled_abc(class) && !is_abc_class(ctx, class))
    {
        ctx.lookup_attr(class, "_abc_impl")?;
        return Err(Error::new("TypeError", "_abc_impl is set to a wrong type"));
    }
    Ok(())
}

fn register(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut owner = args.first().copied();
    let mut subclass = args.get(1).copied();
    for (name, value) in kwargs {
        let slot = match name.as_str() {
            "cls" => &mut owner,
            "subclass" => &mut subclass,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("ABCMeta.register() got an unexpected keyword argument '{name}'"),
                ));
            }
        };
        if slot.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!("ABCMeta.register() got multiple values for argument '{name}'"),
            ));
        }
    }
    if args.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!(
                "ABCMeta.register() takes 2 positional arguments but {} were given",
                args.len()
            ),
        ));
    }
    let (Some(owner), Some(cls)) = (owner, subclass) else {
        let missing = match (owner, subclass) {
            (None, None) => "2 required positional arguments: 'cls' and 'subclass'",
            (None, Some(_)) => "1 required positional argument: 'cls'",
            _ => "1 required positional argument: 'subclass'",
        };
        return Err(Error::new(
            "TypeError",
            format!("ABCMeta.register() missing {missing}"),
        ));
    };
    if !matches!(ctx.builtin_type_name(cls), Some("type" | "typing._AnyMeta")) {
        return Err(Error::new("TypeError", "Can only register classes"));
    }
    let subclass = |ctx: &mut Context, child, parent| {
        if ctx.is_protocol_declaration(parent) {
            ctx.abc_subclass_check(child, parent)
        } else {
            ctx.python_subclass_check(child, parent)
        }
    };
    if subclass(ctx, cls, owner)? {
        return Ok(cls);
    }
    if subclass(ctx, owner, cls)? {
        return Err(Error::new(
            "RuntimeError",
            "Refusing to create an inheritance cycle",
        ));
    }
    // Check for an existing subclass and cycles before looking up the ABC registry.
    if !ctx.is_bundled_abc(owner) && !is_abc_class(ctx, owner) {
        if ctx.class_attr(owner, "_abc_impl").is_some() {
            return Err(Error::new("TypeError", "_abc_impl is set to a wrong type"));
        }
        let name = ctx.lookup_attr(owner, "__name__")?;
        let name = ctx.str_value(name)?;
        return Err(Error::new(
            "AttributeError",
            format!("type object '{name}' has no attribute '_abc_impl'"),
        ));
    }
    ctx.heap.abc_registrations.push([owner, cls]);
    ctx.heap.abc_subclass_cache.retain(|_, matched| *matched);
    ctx.heap.abc_cache_token = ctx.heap.abc_cache_token.wrapping_add(1);
    let flags = ctx.class_pattern_flags(owner);
    if !flags.is_empty() {
        ctx.heap.set_registered_pattern_flags(cls, flags);
    }
    Ok(cls)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "ABCMeta.__new__" {
        return ctx.abc_new(args, kwargs);
    }
    if name == "hook.Callable.__subclasshook__" {
        let Some((&receiver, args)) = args.split_first() else {
            return Err(Error::new("TypeError", "unbound subclass hook"));
        };
        let candidate = match (args, kwargs) {
            ([candidate], []) => *candidate,
            ([], [(key, candidate)]) if key == "C" => *candidate,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    "Callable.__subclasshook__() expects one class",
                ));
            }
        };
        if receiver != callable(ctx)? {
            return Ok(super::value::NOT_IMPLEMENTED);
        }
        let method = ctx.text("__call__");
        return ctx.abc_check_methods(candidate, &[method]);
    }
    if matches!(name, "_abc_registry_clear" | "_abc_caches_clear") {
        let [Some(owner)] = super::builtins::bind_builtin_arguments(
            &format!("ABCMeta.{name}"),
            args,
            kwargs,
            ["cls"],
            1,
        )?
        else {
            unreachable!("ABCMeta clearing methods require a class")
        };
        check_impl(ctx, owner)?;
        if name == "_abc_registry_clear" {
            ctx.heap.abc_registrations.retain(|pair| pair[0] != owner);
        } else {
            ctx.heap
                .abc_subclass_cache
                .retain(|pair, _| pair[0] != owner);
        }
        return Ok(NONE);
    }
    if name == "get_cache_token" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "_abc.get_cache_token() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_abc.get_cache_token() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        return Ok(ctx.alloc(Value::Int(crate::Int::new(ctx.heap.abc_cache_token.into()))));
    }
    if matches!(name, "__instancecheck__" | "__subclasscheck__") {
        let instance = name == "__instancecheck__";
        let argument = if instance { "instance" } else { "subclass" };
        let [Some(class), Some(object)] = super::builtins::bind_builtin_arguments(
            &format!("ABCMeta.{name}"),
            args,
            kwargs,
            ["cls", argument],
            2,
        )?
        else {
            unreachable!("both ABCMeta check arguments are required")
        };
        return if instance {
            ctx.abc_meta_instance_check(object, class)
        } else {
            ctx.abc_meta_subclass_check(object, class)
        };
    }
    if name == "register" {
        return register(ctx, args, kwargs);
    }
    debug_assert_eq!(name, "abstract.Callable.__call__");
    let keyword_self = kwargs.iter().any(|(name, _)| name == "self");
    if args.is_empty() && !keyword_self {
        Err(Error::new(
            "TypeError",
            "Callable.__call__() missing 1 required positional argument: 'self'",
        ))
    } else if !args.is_empty() && keyword_self {
        Err(Error::new(
            "TypeError",
            "Callable.__call__() got multiple values for argument 'self'",
        ))
    } else {
        Ok(FALSE)
    }
}
