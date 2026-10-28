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
            &["__contains__", "get", "items", "keys", "values"],
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
    if structural_methods(name).is_some() {
        let function = ctx.builtin(&format!("abc.hook.{name}.__subclasshook__"));
        let hook = ctx.alloc(Value::ClassMethod(function));
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
    ctx.init_abstract_class(result, name, scope)?;
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

fn register(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let owner = args[0];
    let mut cls = args.get(1).copied();
    for (name, value) in kwargs {
        if name != "subclass" {
            return Err(Error::new(
                "TypeError",
                format!("ABCMeta.register() got an unexpected keyword argument '{name}'"),
            ));
        }
        if cls.replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                "ABCMeta.register() got multiple values for argument 'subclass'",
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
    let cls = cls.ok_or_else(|| {
        Error::new(
            "TypeError",
            "ABCMeta.register() missing 1 required positional argument: 'subclass'",
        )
    })?;
    if ctx.builtin_type_name(cls) != Some("type") {
        return Err(Error::new("TypeError", "Can only register classes"));
    }
    if ctx.python_subclass_check(cls, owner)? {
        return Ok(cls);
    }
    if ctx.python_subclass_check(owner, cls)? {
        return Err(Error::new(
            "RuntimeError",
            "Refusing to create an inheritance cycle",
        ));
    }
    ctx.heap.abc_registrations.push([owner, cls]);
    Ok(cls)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
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
            let kind = match method {
                "keys" => super::value::DictViewKind::Keys,
                "items" => super::value::DictViewKind::Items,
                _ => super::value::DictViewKind::Values,
            };
            Ok(ctx.alloc(Value::MappingView {
                mapping: object,
                kind,
            }))
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

pub(super) fn view_items(
    ctx: &mut Context,
    mapping: Id,
    kind: super::value::DictViewKind,
) -> Result<Vec<Id>> {
    use super::value::DictViewKind;
    let iter = ctx
        .bound_slot(mapping, "__iter__")?
        .expect("mapping __iter__");
    let source = ctx.call(iter, Vec::new(), Vec::new())?;
    let keys = ctx.items(source)?;
    if matches!(kind, DictViewKind::Keys) {
        return Ok(keys);
    }
    keys.into_iter()
        .map(|key| {
            let value = ctx.get_item(mapping, key)?;
            Ok(match kind {
                DictViewKind::Values => value,
                DictViewKind::Items => ctx.alloc(Value::Tuple(vec![key, value])),
                DictViewKind::Keys => unreachable!(),
            })
        })
        .collect()
}
