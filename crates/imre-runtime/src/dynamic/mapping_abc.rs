//! Mapping ABC class identities, virtual dictionary registration, and inherited operations.
use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value};

/// Return the raw methods used by each structural collection ABC.
fn structural_methods(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "Iterable" => Some(&["__iter__"]),
        "Sized" => Some(&["__len__"]),
        "Container" => Some(&["__contains__"]),
        "Collection" => Some(&["__len__", "__iter__", "__contains__"]),
        _ => None,
    }
}

/// Class namespaces bootstrapped here provide ABC methods without native instance storage.
pub(super) fn bootstrapped(name: &str) -> bool {
    matches!(
        name,
        "Sized" | "Iterable" | "Container" | "Collection" | "Mapping" | "MutableMapping"
    )
}

/// Allocate the real class objects with their CPython direct bases and C3 lineage.
pub(super) fn class(ctx: &mut Context, name: &str) -> Result<Id> {
    let key = format!("collections.abc.{name}");
    if let Some(&class) = ctx.classes.get(&key) {
        return Ok(class);
    }
    let (parents, abstract_methods, methods): (&[&str], &[&str], &[&str]) = match name {
        "Sized" => (&[], &["__len__"], &[]),
        "Iterable" => (&[], &["__iter__"], &[]),
        "Container" => (&[], &["__contains__"], &[]),
        "Collection" => (&["Sized", "Iterable", "Container"], &[], &[]),
        "Mapping" => (
            &["Collection"],
            &["__getitem__"],
            &["__contains__", "__eq__", "get", "items", "keys", "values"],
        ),
        "MutableMapping" => (
            &["Mapping"],
            &["__setitem__", "__delitem__"],
            &["pop", "popitem", "clear", "update", "setdefault"],
        ),
        _ => unreachable!("registered mapping ABC"),
    };
    let mut bases = Vec::with_capacity(parents.len());
    for &parent in parents {
        bases.push(class(ctx, parent)?);
    }
    let scope = ctx.scope("collections.abc", None);
    let module = ctx.text("collections.abc");
    ctx.bind(scope, "__module__", module);
    let qualname = ctx.text(name);
    ctx.bind(scope, "__qualname__", qualname);
    if name == "Mapping" {
        ctx.bind(scope, "__reversed__", NONE);
    }
    if structural_methods(name).is_some() {
        let function = ctx.builtin(&format!("abc.hook.{name}.__subclasshook__"));
        let hook = super::method_decorator::class_method(ctx, function)?;
        ctx.bind(scope, "__subclasshook__", hook);
    }
    for &method in abstract_methods {
        let function = ctx.builtin(&format!("abc.abstract.{name}.{method}"));
        ctx.bind(scope, method, function);
    }
    for &method in methods {
        let function = ctx.builtin(&format!("abc.mapping.{method}"));
        ctx.bind(scope, method, function);
    }
    let result = ctx.allocate_class(name.to_owned(), scope, bases)?;
    ctx.classes.insert(key, result);
    ctx.init_abstract_class(result, scope)?;
    Ok(result)
}

/// Iterable and Container supply the generic alias inherited by the collection ABCs.
pub(super) fn is_generic_abc(ctx: &Context, class: Id) -> bool {
    ["collections.abc.Iterable", "collections.abc.Container"]
        .iter()
        .filter_map(|&name| ctx.classes.get(name))
        .any(|&base| ctx.direct_subclass(class, base))
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
    ctx.heap
        .protocol_subclass_cache
        .retain(|_, matched| *matched);
    ctx.heap.abc_cache_token = ctx.heap.abc_cache_token.wrapping_add(1);
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
    if let Some(owner) = name
        .strip_prefix("hook.")
        .and_then(|n| n.strip_suffix(".__subclasshook__"))
    {
        let Some((&receiver, args)) = args.split_first() else {
            return Err(Error::new("TypeError", "unbound subclass hook"));
        };
        let candidate = match (args, kwargs) {
            ([candidate], []) => *candidate,
            ([], [(key, candidate)]) if key == "C" => *candidate,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("{owner}.__subclasshook__() expects one class"),
                ));
            }
        };
        if receiver != class(ctx, owner)? {
            return Ok(super::value::NOT_IMPLEMENTED);
        }
        let methods = structural_methods(owner).expect("registered structural hook");
        let mut names = [NONE; 3];
        for (slot, &method) in names.iter_mut().zip(methods) {
            *slot = ctx.text(method);
        }
        return ctx.abc_check_methods(candidate, &names[..methods.len()]);
    }
    if name == "_abc_caches_clear"
        && let [owner] = args
        && kwargs.is_empty()
    {
        ctx.heap
            .protocol_subclass_cache
            .retain(|pair, _| pair[0] != *owner);
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
    if let Some(name) = name.strip_prefix("abstract.") {
        return match name {
            "Sized.__len__" => Ok(ctx.int(0)),
            "Container.__contains__" => Ok(FALSE),
            "Iterable.__iter__" => {
                let empty = ctx.alloc(Value::Tuple(Vec::new()));
                ctx.iterator(empty)
            }
            _ => Err(Error::new("KeyError", "")),
        };
    }
    if name == "mapping.__eq__" {
        let [Some(left), Some(right)] = super::builtins::bind_builtin_arguments(
            "Mapping.__eq__",
            args,
            kwargs,
            ["self", "other"],
            2,
        )?
        else {
            unreachable!("required mapping operands were checked")
        };
        let mapping = class(ctx, "Mapping")?;
        if !ctx.python_instance_check(right, mapping)? {
            return Ok(super::value::NOT_IMPLEMENTED);
        }
        // Mapping compares the results of each operand's public items() method.
        let snapshot = |ctx: &mut Context, obj| {
            let items = ctx.lookup_attr(obj, "items")?;
            let pairs = ctx.call(items, Vec::new(), Vec::new())?;
            super::builtins::call(ctx, "dict", &[pairs], &[])
        };
        let left = snapshot(ctx, left)?;
        let right = snapshot(ctx, right)?;
        return Ok(if ctx.checked_equal(left, right)? {
            TRUE
        } else {
            FALSE
        });
    }
    let Some((&object, values)) = args.split_first() else {
        return Err(Error::new("TypeError", "missing mapping receiver"));
    };
    let method = name
        .strip_prefix("mapping.")
        .expect("registered ABC method");
    if !kwargs.is_empty() && method != "update" {
        return Err(Error::new(
            "TypeError",
            format!("{method}() takes no keyword arguments"),
        ));
    }
    match (method, values) {
        ("keys" | "items" | "values", []) => {
            let name = match method {
                "keys" => "KeysView",
                "items" => "ItemsView",
                _ => "ValuesView",
            };
            let abc = ctx.module("_collections_abc")?;
            let class = ctx.lookup_attr(abc, name)?;
            ctx.call(class, vec![object], Vec::new())
        }
        ("get" | "__contains__" | "setdefault" | "pop", [key] | [key, _]) => {
            match ctx.get_item(object, *key) {
                Ok(value) => {
                    if method == "pop" {
                        ctx.delete_item(object, *key)?;
                    }
                    Ok(if method == "__contains__" {
                        TRUE
                    } else {
                        value
                    })
                }
                Err(e) if e.kind == "KeyError" => {
                    let default = values.get(1).copied();
                    match method {
                        "__contains__" => Ok(FALSE),
                        "pop" => default.ok_or(e),
                        "setdefault" => {
                            let value = default.unwrap_or(NONE);
                            ctx.put_item(object, *key, value)?;
                            Ok(value)
                        }
                        _ => Ok(default.unwrap_or(NONE)),
                    }
                }
                Err(error) => Err(error),
            }
        }
        ("popitem", []) => {
            let iter = ctx.op("iter", object, NONE, NONE)?;
            if ctx.op("more", iter, NONE, NONE)? == FALSE {
                return Err(Error::new("KeyError", ""));
            }
            let key = ctx.op("next", iter, NONE, NONE)?;
            let value = ctx.get_item(object, key)?;
            ctx.delete_item(object, key)?;
            Ok(ctx.alloc(Value::Tuple(vec![key, value])))
        }
        ("clear", []) => {
            let pop = ctx.lookup_attr(object, "popitem")?;
            loop {
                match ctx.call(pop, Vec::new(), Vec::new()) {
                    Ok(_) => (),
                    Err(e) if e.kind == "KeyError" => return Ok(NONE),
                    Err(e) => return Err(e),
                }
            }
        }
        ("update", [] | [_]) => {
            if let [input] = values
                && !ctx.for_each_mapping_item(
                    *input,
                    object,
                    super::calls::MappingOperation::Update,
                    |ctx, key, value, _hash| ctx.put_item(object, key, value),
                )?
            {
                for row in ctx.items(*input)? {
                    let fields = ctx.items(row)?;
                    let [key, value] = fields.as_slice() else {
                        return Err(Error::new("ValueError", "expected a key-value pair"));
                    };
                    ctx.put_item(object, *key, *value)?;
                }
            }
            for (key, value) in kwargs {
                let key = ctx.text(key);
                ctx.put_item(object, key, *value)?;
            }
            Ok(NONE)
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {method}()"),
        )),
    }
}
