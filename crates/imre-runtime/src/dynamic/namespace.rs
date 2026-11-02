//! The boxed types.SimpleNamespace class and its Python-visible descriptors.

use super::{Context, Error, Id, Result, Value};

pub(super) const CLASS: &str = "types.SimpleNamespace";

pub(super) fn class(ctx: &mut Context) -> Result<Id> {
    if let Some(&class) = ctx.classes.get(CLASS) {
        return Ok(class);
    }
    let scope = ctx.scope("types", None);
    let module = ctx.text("types");
    ctx.bind(scope, "__module__", module);
    let name = ctx.text("SimpleNamespace");
    ctx.bind(scope, "__qualname__", name);
    for slot in ["__init__", "__repr__", "__eq__", "__ne__", "__reduce__", "__replace__"] {
        let method = ctx.builtin(&format!("{CLASS}.{slot}"));
        ctx.bind(scope, slot, method);
    }
    let get = ctx.builtin("types.SimpleNamespace.__dict__");
    let set = ctx.builtin("types.SimpleNamespace.readonly_dict");
    let descriptor = ctx.alloc(Value::GetSetDescriptor {
        get,
        set: Some(set),
        owner: CLASS,
        name: "__dict__",
    });
    ctx.bind(scope, "__dict__", descriptor);
    let object = ctx.builtin("object");
    let class = ctx.allocate_class("SimpleNamespace".into(), scope, vec![object])?;
    // The static type has dictionary storage but introduces no weak-reference slot.
    if let Value::Scope(frame) = ctx.heap.get_mut(scope) {
        frame.class_namespace.as_mut().unwrap().slot_names = Some(["__dict__".into()].into());
    }
    ctx.classes.insert(CLASS.into(), class);
    ctx.builtins.insert(CLASS.into(), class);
    Ok(class)
}

/// The type's slot descriptors are immutable; ordinary subclass attributes are writable.
pub(super) fn check_class_write(ctx: &Context, obj: Id, name: &str) -> Result<()> {
    if matches!(ctx.heap.get(obj), Value::Class { .. }) && ctx.classes.get(CLASS) == Some(&obj) {
        return Err(Error::new(
            "TypeError",
            format!("cannot set '{name}' attribute of immutable type '{CLASS}'"),
        ));
    }
    Ok(())
}

/// A namespace descriptor requires real namespace storage, including on a subclass.
pub(super) fn instance_class(ctx: &Context, object: Id) -> Option<Id> {
    match *ctx.heap.get(object) {
        Value::Instance { class, .. } if ctx.direct_subclass(class, ctx.classes[CLASS]) => Some(class),
        _ => None,
    }
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&object, rest) = args.split_first().ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("descriptor '{name}' of '{CLASS}' object needs an argument"),
        )
    })?;
    let Some(class) = instance_class(ctx, object) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' requires a '{CLASS}' object but received a '{}'",
                ctx.type_name(object)
            ),
        ));
    };
    if name == "__init__" {
        return super::namespace_init::initialize(ctx, object, rest, kwargs);
    }
    if name == "__replace__" {
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                "__replace__() takes no positional arguments",
            ));
        }
        return super::namespace_init::replace(ctx, object, class, kwargs);
    }
    if name == "__reduce__" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "SimpleNamespace.__reduce__() takes no keyword arguments",
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("SimpleNamespace.__reduce__() takes no arguments ({} given)", rest.len()),
            ));
        }
        let args = ctx.alloc(Value::Tuple(Vec::new()));
        let state = ctx.instance_dictionary(object, class);
        return Ok(ctx.alloc(Value::Tuple(vec![class, args, state])));
    }
    if name == "readonly_dict" {
        return Err(Error::new("AttributeError", "readonly attribute"));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("wrapper {name}() takes no keyword arguments"),
        ));
    }
    match (name, rest) {
        ("__repr__", []) => super::namespace_repr::representation(ctx, object),
        ("__eq__" | "__ne__", [other]) => {
            super::namespace_init::equal(ctx, object, *other, name == "__ne__")
        }
        ("__dict__", []) => Ok(ctx.instance_dictionary(object, class)),
        _ => {
            let expected = usize::from(matches!(name, "__eq__" | "__ne__"));
            Err(Error::new(
                "TypeError",
                format!("expected {expected} arguments, got {}", rest.len()),
            ))
        }
    }
}
