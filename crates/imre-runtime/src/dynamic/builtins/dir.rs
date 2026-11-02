use super::{Context, Error, Id, NONE, Result, Value, collections, type_error};
use crate::dynamic::typing::{Alias, AliasKind, forwards_to_origin, literal_origin};
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

/// PEP 585 aliases add their local attributes to the origin's directory.
fn generic_alias_names(ctx: &mut Context, origin: Id) -> Result<Id> {
    let dir = ctx.builtin("dir");
    let names = ctx.call(dir, vec![origin], Vec::new())?;
    ctx.heap.keep(names);
    for field in [
        "__class__",
        "__origin__",
        "__args__",
        "__unpacked__",
        "__parameters__",
        "__typing_unpacked_tuple_args__",
        "__mro_entries__",
        "__reduce_ex__",
        "__reduce__",
    ] {
        let field = ctx.heap.code_name(field);
        if !ctx.contains(field, names)? {
            let Value::List(items) = ctx.heap.get_mut(names) else {
                unreachable!("dir() returns a list")
            };
            items.try_reserve(1).map_err(|_| crate::Error::memory_error())?;
            items.push(field);
        }
    }
    Ok(names)
}

/// Merge the instance dictionary with the class dictionaries along the runtime's MRO.
fn object_names(ctx: &mut Context, object: Id) -> Result<Id> {
    let mut names = BTreeSet::new();
    let mut keys = Vec::new();
    let class = match ctx.heap.get(object) {
        Value::GenericAlias(alias) if alias.is_builtin_alias() => {
            return generic_alias_names(ctx, alias.origin);
        }
        Value::GenericAlias(alias) if matches!(alias.kind, AliasKind::Literal(_)) => {
            return literal_names(ctx, alias.clone());
        }
        Value::GenericAlias(alias) if alias.kind == AliasKind::Concatenate => {
            let origin = alias.origin;
            names.extend(alias.metadata().map(|(name, _)| name.to_owned()));
            names.extend(
                ["_inst", "_name", "copy_with", "__iter__", "__mro_entries__"].map(str::to_owned),
            );
            let origin_names = call(ctx, &[origin], &[])?;
            ctx.heap.keep(origin_names);
            let marker = ctx.text("__");
            ctx.heap.keep(marker);
            for name in ctx.items(origin_names)? {
                if visible_origin_name(ctx, name, marker)? {
                    keys.push(name);
                }
            }
            ctx.builtin("object")
        }
        Value::GenericAlias(alias) if alias.kind.is_typing_alias() => {
            return typing_alias_dir(ctx, &alias.clone());
        }
        Value::ParamSpec(_) => {
            names.extend(super::super::paramspec::names().map(str::to_owned));
            ctx.builtin("object")
        }
        Value::Native(_) | Value::Method { .. }
            if ctx.builtin_type_name(object) == Some("builtin_function_or_method") =>
        {
            ctx.builtin("builtin_function_or_method")
        }
        value if value.is_weak_proxy() => return namespace_directory(ctx, object, true),
        Value::Class { .. } if ctx.has_base(object, "tuple") => {
            return namespace_directory(ctx, object, false);
        }
        Value::Instance { class, .. } if ctx.has_base(*class, "tuple") => {
            return namespace_directory(ctx, object, true);
        }
        Value::Class { .. } | Value::Native(_) => object,
        Value::Count { .. }
        | Value::Repeat(_)
        | Value::DequeIterator { .. }
        | Value::Union(_)
        | Value::Dict(_)
        | Value::Tuple(_)
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

/// Look up an optional directory input without treating other hook exceptions as absence.
fn optional_attr(ctx: &mut Context, object: Id, name: &str) -> Result<Option<Id>> {
    match ctx.lookup_attr(object, name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if ctx.is_attribute_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Copy the instance dictionary before a reported __class__ can mutate it.
fn namespace_directory(ctx: &mut Context, object: Id, instance: bool) -> Result<Id> {
    let dictionary = if instance
        && let Some(source) = optional_attr(ctx, object, "__dict__")?
        && let Some(storage) = ctx.dict_storage(source)
    {
        ctx.heap.keep(source);
        collections::copy_dict(ctx, source, storage)?
    } else {
        ctx.dict(Vec::new())
    };
    ctx.heap.keep(dictionary);
    let class = if instance {
        optional_attr(ctx, object, "__class__")?
    } else {
        Some(object)
    };
    if let Some(class) = class {
        merge_class_names(ctx, class, dictionary)?;
    }
    Ok(dictionary)
}

/// A class namespace exposed through its ordinary proxy includes its own instance dict descriptor.
fn contributes_dict_descriptor(ctx: &mut Context, class: Id, mapping: Id) -> Result<bool> {
    let Value::Class { scope, .. } = *ctx.heap.get(class) else {
        return Ok(false);
    };
    let layout = ctx
        .scope_data(scope)
        .class_namespace
        .as_deref()
        .expect("class namespace");
    if layout.dictionary != Some(mapping) || !ctx.class_instances_have_dict(class) {
        return Ok(false);
    }
    let bases = ctx.items(layout.bases)?;
    if bases.is_empty() {
        return Ok(true);
    }
    let base = ctx.member_layout_base(&bases)?;
    Ok(!ctx.class_instances_have_dict(base))
}

/// Merge each reported namespace before reading its bases, as in object.__dir__.
fn merge_class_names(ctx: &mut Context, class: Id, dictionary: Id) -> Result<()> {
    if matches!(ctx.heap.get(class), Value::Native(name) if super::is_builtin_type(name)) {
        let mut names = BTreeSet::new();
        class_names(ctx, class, &mut names)?;
        for name in names {
            let key = ctx.text(name);
            ctx.put_item(dictionary, key, NONE)?;
        }
        return Ok(());
    }
    ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(class);
        if let Some(namespace) = optional_attr(ctx, class, "__dict__")? {
            ctx.heap.keep(namespace);
            let merged = ctx.for_each_mapping_item(
                namespace,
                dictionary,
                crate::dynamic::calls::MappingOperation::Copy,
                |ctx, key, value, hash| {
                    ctx.set_dictionary_key(dictionary, key, value, hash, true)
                        .map(|_| ())
                },
            )?;
            if !merged {
                return Err(ctx.missing_attribute(namespace, "keys"));
            }
            if let Value::MappingProxy(mapping) = *ctx.heap.get(namespace)
                && contributes_dict_descriptor(ctx, class, mapping)?
            {
                let key = ctx.text("__dict__");
                ctx.put_item(dictionary, key, NONE)?;
            }
        }
        if let Some(bases) = optional_attr(ctx, class, "__bases__")? {
            ctx.heap.keep(bases);
            if let Some(kind @ ("dict" | "collections.defaultdict" | "mappingproxy")) =
                ctx.builtin_type_name(bases)
            {
                return Err(type_error(format!("{kind} is not a sequence")));
            }
            let length = super::call(ctx, "len", &[bases], &[])?;
            let count = ctx.as_int(length)?.to_i64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                )
            })?;
            for i in 0..count {
                let index = ctx.int(i);
                let base = ctx.get_item(bases, index)?;
                merge_class_names(ctx, base, dictionary)?;
            }
        }
        ctx.heap.release_temporaries_since(mark);
        Ok(())
    })
}

/// Collect a native typing alias's implemented attributes and its origin's non-dunder names.
pub(in crate::dynamic) fn typing_alias_dir(ctx: &mut Context, alias: &Alias) -> Result<Id> {
    let fields = alias.instance_fields().map(|(name, _)| name);
    let fields = fields.into_iter().chain([
        "__class__",
        "__dir__",
        "__iter__",
        "__mro_entries__",
        "copy_with",
    ]);
    let origin_names = ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
        call(ctx, &[alias.origin], &[])
    })?;
    let names = merge_origin_names(ctx, fields, origin_names)?;
    ctx.heap.keep(names);
    let class = ctx.builtin(alias.kind.class());
    if let Some(&dictionary) = ctx.native_type_dicts.get(&class) {
        if alias.kind == AliasKind::Generic {
            crate::dynamic::typing::include_generic_alias_methods(ctx, class, dictionary)?;
        }
        for (key, _) in ctx
            .dict_entries(dictionary)
            .expect("native alias class dictionary")
            .to_vec()
        {
            // An unnamed generic has no stored instance module; its class's module would be wrong.
            if alias.kind == AliasKind::Generic
                && alias.name.is_none()
                && ctx.str_value(key)? == "__module__"
            {
                continue;
            }
            ctx.put_item(names, key, NONE)?;
        }
    }
    if alias.kind == AliasKind::Callable {
        // The native Callable alias owns its reducer and its public module name.
        let reduce = ctx.text("__reduce__");
        ctx.put_item(names, reduce, NONE)?;
        if alias.name.is_some() {
            let module = ctx.text("__module__");
            ctx.put_item(names, module, NONE)?;
        }
    }
    let items = ctx.items(names)?;
    Ok(ctx.alloc(Value::List(items)))
}

/// Literal delegates non-dunder names to its origin, including custom Python directory hooks.
fn literal_names(ctx: &mut Context, alias: Alias) -> Result<Id> {
    let (origin, nested) = literal_origin(ctx, alias.origin)?;
    let mut names = call(ctx, &[origin], &[])?;
    // Each enclosing alias filters the sorted dir() of the preceding alias.
    for _ in 0..nested {
        let merged = merge_literal_names(ctx, &alias, names)?;
        names = collections::sorted(ctx, &[merged], &[])?;
    }
    merge_literal_names(ctx, &alias, names)
}

fn merge_literal_names(ctx: &mut Context, alias: &Alias, origin_names: Id) -> Result<Id> {
    let fields = alias.instance_fields().map(|(name, _)| name);
    let fields = fields.into_iter().chain([
        "__class__",
        "__format__",
        "__getstate__",
        "__init_subclass__",
        "__iter__",
        "__mro_entries__",
    ]);
    merge_origin_names(ctx, fields, origin_names)
}

fn merge_origin_names(
    ctx: &mut Context,
    fields: impl IntoIterator<Item = &'static str>,
    origin_names: Id,
) -> Result<Id> {
    ctx.heap.keep(origin_names);
    let entries = fields
        .into_iter()
        .map(|name| (ctx.text(name), NONE))
        .collect();
    let names = ctx.dict(entries);
    ctx.heap.keep(names);
    let marker = ctx.text("__");
    ctx.heap.keep(marker);
    for name in ctx.items(origin_names)? {
        if visible_origin_name(ctx, name, marker)? {
            ctx.put_item(names, name, NONE)?;
        }
    }
    Ok(names)
}

/// Keep exact Unicode string names on their native storage path, including lone surrogates.
fn visible_origin_name(ctx: &mut Context, name: Id, marker: Id) -> Result<bool> {
    match ctx.heap.get(name) {
        Value::Str(text) => return Ok(forwards_to_origin(text)),
        Value::SurrogateStr(points) => {
            let underscores = [u32::from('_'); 2];
            return Ok(!(points.starts_with(&underscores) && points.ends_with(&underscores)));
        }
        _ => {}
    }
    for method in ["startswith", "endswith"] {
        let check = ctx.lookup_attr(name, method)?;
        let matches = ctx.call(check, vec![marker], Vec::new())?;
        if !ctx.truth(matches)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Collect each Python class's own names without evaluating class values or descriptors.
fn class_names(ctx: &Context, class: Id, names: &mut BTreeSet<String>) -> Result<()> {
    match ctx.heap.get(class) {
        Value::Native(name) if name == "tuple" => {
            class_names(ctx, ctx.builtins["object"], names)?;
            names.extend(TUPLE_NAMES.map(str::to_owned));
        }
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

const TUPLE_NAMES: [&str; 11] = [
    "__add__",
    "__class_getitem__",
    "__contains__",
    "__getitem__",
    "__getnewargs__",
    "__iter__",
    "__len__",
    "__mul__",
    "__rmul__",
    "count",
    "index",
];

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
            ])
            .chain(TUPLE_NAMES)
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
