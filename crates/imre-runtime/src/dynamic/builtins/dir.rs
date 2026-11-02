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
    } else if let Value::Module { scope, .. } = *ctx.heap.get(object) {
        ctx.module_dir(object, scope)?
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
        Value::ParamSpec(_) => {
            names.extend(super::super::paramspec::names().map(str::to_owned));
            ctx.builtin("object")
        }
        Value::Native(_) | Value::Method { .. }
            if ctx.builtin_type_name(object) == Some("builtin_function_or_method") =>
        {
            ctx.builtin("builtin_function_or_method")
        }
        Value::Class { .. } | Value::Native(_) => object,
        Value::Count { .. }
        | Value::Repeat(_)
        | Value::Dict(_)
        | Value::List(_)
        | Value::Str(_)
        | Value::SurrogateStr(_) => ctx.exact_type(object)?,
        Value::NamedTuple {
            class: Some(class), ..
        } if crate::dynamic::stdlib::os::stat::layout(class).is_some()
            || crate::dynamic::stdlib::fixed_structseq::fixed_type(class).is_some() =>
        {
            ctx.builtin(class)
        }
        Value::Instance {
            class,
            fields,
            namespace,
            ..
        } => {
            let class = *class;
            if namespace.is_none()
                && !fields.contains_key("__dict__")
                && ctx.class_attr(class, "__dict__").is_none()
                && ctx.class_attr(class, "__getattribute__").is_none()
            {
                names.extend(fields.keys().cloned());
            } else {
                match ctx.lookup_attr(object, "__dict__") {
                    Ok(dict) => {
                        if let Some(entries) = ctx
                            .dict_storage(dict)
                            .and_then(|storage| ctx.dict_entries(storage))
                        {
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
        Value::Native(name)
            if matches!(
                name.as_str(),
                "builtin_function_or_method" | "dict" | "list" | "str"
            ) =>
        {
            class_names(ctx, ctx.builtins["object"], names)?;
            let (methods, slots): (&[&str], &[&str]) = match name.as_str() {
                "builtin_function_or_method" => (
                    &[],
                    &[
                        "__call__",
                        "__module__",
                        "__name__",
                        "__qualname__",
                        "__self__",
                        "__text_signature__",
                    ],
                ),
                "dict" => (
                    super::DICT_METHODS,
                    &["__class_getitem__", "__or__", "__reversed__", "__ror__"],
                ),
                "list" => (
                    super::LIST_METHODS,
                    &[
                        "__class_getitem__",
                        "__contains__",
                        "__delitem__",
                        "__getitem__",
                        "__imul__",
                        "__iter__",
                        "__len__",
                        "__mul__",
                        "__reversed__",
                        "__rmul__",
                        "__setitem__",
                    ],
                ),
                _ => {
                    // Passive attribute lookup in the compiler uses the same public str names.
                    names.extend(
                        include_str!("../../../native/str-method-names.txt")
                            .split_ascii_whitespace()
                            .map(str::to_owned),
                    );
                    (
                        &[],
                        &[
                            "__add__",
                            "__contains__",
                            "__getitem__",
                            "__getnewargs__",
                            "__iter__",
                            "__len__",
                            "__mod__",
                            "__mul__",
                            "__rmod__",
                            "__rmul__",
                        ],
                    )
                }
            };
            names.extend(methods.iter().chain(slots).map(|&name| name.to_owned()));
        }
        Value::Class {
            name: owner, scope, ..
        } => {
            let scope_id = *scope;
            let scope = ctx.scope_data(scope_id);
            names.extend(
                ctx.scope_bindings(scope_id)
                    .map(|(name, _)| name)
                    .filter(|name| !matches!(*name, "__qualname__" | "__annotations__"))
                    .map(str::to_owned),
            );
            // These are implicit entries in the dictionary a Python class body creates.
            if scope.class_cell.is_some() {
                names.extend(
                    ["__firstlineno__", "__static_attributes__", "__module__"].map(str::to_owned),
                );
            }
            if let Some(annotations) =
                ctx.namespace_binding(ctx.scope_data(scope_id), "__annotations__")
                && ctx
                    .dict_entries(annotations)
                    .is_none_or(|entries| !entries.is_empty())
            {
                names.insert("__annotations__".to_owned());
            }
            if scope
                .class_namespace
                .as_ref()
                .is_some_and(|ns| ns.annotation_sites.is_some())
            {
                names.insert("__annotate_func__".to_owned());
            }
            if let Some(slots) = ctx.namespace_binding(ctx.scope_data(scope_id), "__slots__") {
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
        Value::Native(_) if let Some(&dictionary) = ctx.native_type_dicts.get(&class) => {
            class_names(ctx, ctx.builtins["object"], names)?;
            for &(key, _) in ctx
                .dict_entries(dictionary)
                .expect("native class dictionary")
            {
                names.insert(ctx.str_value(key)?.to_owned());
            }
        }
        Value::Native(name)
            if let Some(layout) = crate::dynamic::stdlib::os::stat::layout(name) =>
        {
            struct_sequence_names(names, crate::dynamic::stdlib::os::stat::field_names(layout));
        }
        Value::Native(name)
            if let Some(class) = crate::dynamic::stdlib::fixed_structseq::fixed_type(name) =>
        {
            struct_sequence_names(
                names,
                crate::dynamic::stdlib::fixed_structseq::fields(class)
                    .iter()
                    .copied(),
            );
        }
        Value::Native(name) => return Err(missing_namespace(name)),
        _ => return Err(missing_namespace(ctx.type_name(class))),
    }
    Ok(())
}

/// Both the type and its instances expose the struct sequence and inherited tuple slots.
fn struct_sequence_names(names: &mut BTreeSet<String>, fields: impl Iterator<Item = &'static str>) {
    names.extend(
        crate::objects::OBJECT_METHODS
            .iter()
            .copied()
            .chain(fields)
            .chain([
                "__class__",
                "__doc__",
                "__module__",
                "__match_args__",
                "__replace__",
                "n_fields",
                "n_sequence_fields",
                "n_unnamed_fields",
                "__class_getitem__",
                "__add__",
                "count",
                "__rmul__",
                "__contains__",
                "__mul__",
                "__getitem__",
                "__len__",
                "__iter__",
                "__getnewargs__",
                "index",
            ])
            .map(str::to_owned),
    );
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
