use super::{Context, Error, Id, NONE, Result, Value, collections, type_error};
use std::collections::BTreeSet;

/// Apply the type's special method and sort a fresh list, preserving duplicates and comparison errors.
pub(super) fn call(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(type_error("dir() takes no keyword arguments"));
    }
    let object = match args {
        [object] => *object,
        [] => {
            return Err(Error::new(
                "NotImplementedError",
                "dir() requires caller locals",
            ));
        }
        _ => {
            return Err(type_error(format!(
                "dir expected at most 1 argument, got {}",
                args.len()
            )));
        }
    };
    let names = if let Some(method) = ctx.bound_slot(object, "__dir__")? {
        ctx.call(method, vec![], vec![])?
    } else if let Value::Module {
        scope,
        name,
        constructor,
        ..
    } = ctx.heap.get(object)
    {
        let scope = *scope;
        if let Some(method) = ctx.scope_name(scope, "__dir__") {
            ctx.call(method, vec![], vec![])?
        } else if constructor.is_none() && super::super::stdlib::is_module(name) {
            return Err(Error::new(
                "NotImplementedError",
                "dir() cannot enumerate lazy native module exports",
            ));
        } else {
            ctx.module_globals(scope)
        }
    } else {
        object_names(ctx, object)?
    };
    collections::sorted(ctx, &[names], &[])
}

/// Merge the instance dictionary with the class dictionaries along the runtime's MRO.
fn object_names(ctx: &mut Context, object: Id) -> Result<Id> {
    let mut names = BTreeSet::new();
    let mut keys = Vec::new();
    let class = match ctx.heap.get(object) {
        Value::Class { .. } | Value::Native(_) => object,
        Value::Instance { class, fields, .. } => {
            let class = *class;
            if !fields.contains_key("__dict__")
                && ctx.class_attr(class, "__dict__").is_none()
                && ctx.class_attr(class, "__getattribute__").is_none()
            {
                names.extend(fields.keys().cloned());
            } else {
                match ctx.lookup_attr(object, "__dict__") {
                    Ok(dict) => {
                        if let Some(entries) = ctx.dict_entries(dict) {
                            keys.extend(entries.iter().map(|&(key, _)| key));
                        }
                    }
                    Err(error) if ctx.is_attribute_error(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            // A Python hook can hide or replace the type that object.__dir__ inspects.
            if ctx.class_attr(class, "__class__").is_some()
                || ctx.class_attr(class, "__getattribute__").is_some()
            {
                for &key in &keys {
                    ctx.heap.keep(key);
                }
                match ctx.lookup_attr(object, "__class__") {
                    Ok(class) => class,
                    Err(error) if ctx.is_attribute_error(&error) => NONE,
                    Err(error) => return Err(error),
                }
            } else {
                class
            }
        }
        _ => return Err(missing_namespace(ctx.type_name(object))),
    };
    if let Value::Class { ancestors, .. } = ctx.heap.get(class) {
        for &base in std::iter::once(&class).chain(ancestors) {
            class_names(ctx, base, &mut names)?;
        }
    } else if class != NONE {
        class_names(ctx, class, &mut names)?;
    }
    let dictionary = ctx.dict(keys.into_iter().map(|key| (key, NONE)).collect());
    for name in names {
        let key = ctx.text(name);
        ctx.put_item(dictionary, key, NONE)?;
    }
    Ok(dictionary)
}

/// Collect each Python class's own names without evaluating class values or descriptors.
fn class_names(ctx: &Context, class: Id, names: &mut BTreeSet<String>) -> Result<()> {
    match ctx.heap.get(class) {
        Value::Native(name) if name == "object" => {
            names.extend(
                crate::objects::OBJECT_METHODS
                    .iter()
                    .map(|&name| name.to_owned()),
            );
            names.extend(["__class__".to_owned(), "__doc__".to_owned()]);
        }
        Value::Class {
            name: owner, scope, ..
        } => {
            let scope = ctx.scope_data(*scope);
            names.extend(
                scope
                    .names
                    .keys()
                    .filter(|name| !matches!(name.as_str(), "__qualname__" | "__annotations__"))
                    .cloned(),
            );
            // These are implicit entries in the dictionary a Python class body creates.
            if scope.class_cell.is_some() {
                names.extend(
                    ["__firstlineno__", "__static_attributes__", "__module__"].map(str::to_owned),
                );
            }
            if let Some(&annotations) = scope.names.get("__annotations__")
                && ctx
                    .dict_entries(annotations)
                    .is_none_or(|entries| !entries.is_empty())
            {
                names.insert("__annotations__".to_owned());
            }
            if scope
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.annotation_thunks.is_some())
            {
                names.insert("__annotate_func__".to_owned());
            }
            if let Some(&slots) = scope.names.get("__slots__") {
                match ctx.heap.get(slots) {
                    Value::Str(name) => {
                        names.insert(slot_name(owner, name));
                    }
                    Value::Tuple(slots) | Value::List(slots) => {
                        for &slot in slots {
                            names.insert(slot_name(owner, ctx.str_value(slot)?));
                        }
                    }
                    _ => {
                        return Err(Error::new(
                            "NotImplementedError",
                            "dir() requires materialized class slots",
                        ));
                    }
                }
            } else {
                names.extend(["__dict__".to_owned(), "__weakref__".to_owned()]);
            }
        }
        Value::Native(name) => return Err(missing_namespace(name)),
        _ => return Err(missing_namespace(ctx.type_name(class))),
    }
    Ok(())
}

/// Python installs a private slot under its declaring class's mangled name.
fn slot_name(owner: &str, slot: &str) -> String {
    let owner = owner.trim_start_matches('_');
    if !owner.is_empty() && slot.starts_with("__") && !slot.ends_with("__") {
        format!("_{owner}{slot}")
    } else {
        slot.to_owned()
    }
}

fn missing_namespace(name: &str) -> Error {
    Error::new(
        "NotImplementedError",
        format!("dir() cannot enumerate the native namespace of '{name}'"),
    )
}
