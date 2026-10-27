//! Runtime-created tuple subclasses use the same positional layout as `typing.NamedTuple`.

use std::collections::HashSet;

use super::{Context, Error, Id, NONE, Result, Value, builtins};
use ruff_python_stdlib::{identifiers::is_identifier, keyword::is_keyword};

/// Consume the complete iterable before inspecting its length, including errors from its iterator.
fn collect(ctx: &mut Context, source: Id) -> Result<Vec<Id>> {
    let iterator = ctx.iterator(source)?;
    ctx.heap.keep(iterator);
    let mut items = Vec::new();
    while let Some(item) = ctx.next_item(iterator)? {
        ctx.heap.keep(item);
        items.push(item);
    }
    Ok(items)
}

fn string(ctx: &mut Context, object: Id) -> Result<String> {
    let constructor = ctx.builtin("str");
    let value = ctx.call(constructor, vec![object], Vec::new())?;
    Ok(ctx.str_value(value)?.to_owned())
}

fn validate(name: &str) -> Result<()> {
    if is_identifier(name) {
        return Ok(());
    }
    let reason = if is_keyword(name) {
        "cannot be a keyword"
    } else {
        "must be valid identifiers"
    };
    let repr = crate::objects::exceptions::string_repr(name);
    Err(Error::new(
        "ValueError",
        format!("Type names and field names {reason}: {repr}"),
    ))
}

/// Allocate a fresh class for each call, even when its typename and fields match an earlier declaration.
pub(super) fn factory(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [typename, fields, rename, defaults, module] = builtins::bind_method_arguments(
        "namedtuple",
        args,
        kwargs,
        ["typename", "field_names", "rename", "defaults", "module"],
        2,
    )?;
    let (Some(typename), Some(fields)) = (typename, fields) else {
        return Err(Error::new(
            "TypeError",
            "namedtuple() requires a typename and field_names",
        ));
    };
    let mut names: Vec<String> = if let Ok(text) = ctx.str_value(ctx.scalar_value(fields)) {
        text.split(|c| c == ',' || crate::objects::python_whitespace(c))
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        collect(ctx, fields)?
            .into_iter()
            .map(|v| string(ctx, v))
            .collect::<Result<_>>()?
    };
    let typename = string(ctx, typename)?;
    let rename = ctx.optional_truth(rename, false)?;
    if rename {
        let mut seen = HashSet::new();
        for (i, name) in names.iter_mut().enumerate() {
            let duplicate = !seen.insert(name.clone());
            if duplicate || name.starts_with('_') || !is_identifier(name) {
                *name = format!("_{i}");
            }
        }
    }
    validate(&typename)?;
    for name in &names {
        validate(name)?;
    }
    let mut unique = HashSet::new();
    for name in &names {
        let message = if name.starts_with('_') && !rename {
            "Field names cannot start with an underscore"
        } else if !unique.insert(name) {
            "Encountered duplicate field name"
        } else {
            continue;
        };
        return Err(Error::new(
            "ValueError",
            format!(
                "{message}: {}",
                crate::objects::exceptions::string_repr(name)
            ),
        ));
    }
    let defaults = match defaults {
        Some(id) if id != NONE => collect(ctx, id)?,
        _ => Vec::new(),
    };
    if defaults.len() > names.len() {
        return Err(Error::new(
            "TypeError",
            "Got more default values than field names",
        ));
    }
    let module = module
        .filter(|&id| id != NONE)
        .unwrap_or_else(|| ctx.heap.caller_module());
    let module_name = ctx.str_value(module).unwrap_or("__main__").to_owned();
    let scope = ctx.scope(&module_name, None);
    ctx.heap.keep(scope);
    let name = ctx.text(&typename);
    ctx.bind(scope, "__name__", name);
    ctx.bind(scope, "__qualname__", name);
    ctx.bind(scope, "__module__", module);
    let field_ids: Vec<_> = names.iter().map(|n| ctx.text(n)).collect();
    let offset = field_ids.len() - defaults.len();
    let field_defaults = ctx.dict(
        field_ids[offset..]
            .iter()
            .zip(&defaults)
            .map(|(&k, &v)| (k, v))
            .collect(),
    );
    ctx.bind(scope, "_field_defaults", field_defaults);
    let fields = ctx.alloc(Value::Tuple(field_ids));
    ctx.bind(scope, "_fields", fields);
    ctx.bind(scope, "__match_args__", fields);
    let collections = ctx.module("collections")?;
    let repr_factory = ctx.lookup_attr(collections, "_namedtuple_repr")?;
    let repr = ctx.call(repr_factory, vec![name, fields], Vec::new())?;
    ctx.bind(scope, "__repr__", repr);
    let slots = ctx.alloc(Value::Tuple(Vec::new()));
    ctx.bind(scope, "__slots__", slots);
    let doc = ctx.text(format!("{typename}({})", names.join(", ")));
    ctx.bind(scope, "__doc__", doc);
    let make = ctx.builtin("_collections._make");
    let make = ctx.alloc(Value::ClassMethod(make));
    ctx.bind(scope, "_make", make);
    let tuple = ctx.builtin("tuple");
    let class = ctx.allocate_class(typename, scope, vec![tuple])?;
    let Value::Class {
        namedtuple_fields, ..
    } = ctx.heap.get_mut(class)
    else {
        unreachable!()
    };
    *namedtuple_fields = Some(names.into());
    if !defaults.is_empty() {
        let defaults = ctx.alloc(Value::Tuple(defaults));
        let Value::Scope(scope) = ctx.heap.get_mut(scope) else {
            unreachable!()
        };
        scope.class_namespace.as_mut().unwrap().namedtuple_defaults = Some(defaults);
    }
    Ok(class)
}

/// Build the descriptor's actual receiver class from the iterable's positional values.
pub(super) fn make(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [Some(class), Some(iterable)] =
        builtins::bind_method_arguments("_make", args, kwargs, ["cls", "iterable"], 2)?
    else {
        return Err(Error::new("TypeError", "_make() requires an iterable"));
    };
    let names = ctx
        .namedtuple_fields(class)
        .ok_or_else(|| Error::new("TypeError", "expected a named tuple class"))?;
    let values = collect(ctx, iterable)?;
    if names.len() != values.len() {
        return Err(Error::new(
            "TypeError",
            format!("Expected {} arguments, got {}", names.len(), values.len()),
        ));
    }
    ctx.call(class, values, Vec::new())
}
