//! A live mapping over the bindings of an optimized Python frame.

use super::value::NOT_IMPLEMENTED;
use super::{Context, Error, Id, NONE, Result, Value, builtins, calls::MappingOperation};

pub(super) const CLASS: &str = "FrameLocalsProxy";

fn class(ctx: &mut Context) -> Result<Id> {
    if let Some(&class) = ctx.classes.get(CLASS) {
        return Ok(class);
    }
    let scope = ctx.scope("builtins", None);
    let module = ctx.text("builtins");
    ctx.bind(scope, "__module__", module);
    let slots = ctx.alloc(Value::Tuple(Vec::new()));
    ctx.bind(scope, "__slots__", slots);
    ctx.bind(scope, "__hash__", NONE);
    for name in [
        "__new__",
        "__repr__",
        "__eq__",
        "__ne__",
        "__iter__",
        "__len__",
        "__getitem__",
        "__setitem__",
        "__delitem__",
        "__contains__",
        "__or__",
        "__ior__",
        "__ror__",
        "__reversed__",
        "get",
        "keys",
        "values",
        "items",
        "copy",
        "pop",
        "setdefault",
        "update",
    ] {
        let method = ctx.builtin(&format!("{CLASS}.{name}"));
        ctx.bind(scope, name, method);
    }
    let object = ctx.builtin("object");
    let class = ctx.allocate_class(CLASS.into(), scope, vec![object])?;
    ctx.heap
        .set_registered_pattern_flags(class, super::collection_pattern::PatternFlags::MAPPING);
    if let Value::Scope(data) = ctx.heap.get_mut(scope) {
        data.class_namespace.as_mut().unwrap().immutable_type = true;
    }
    ctx.classes.insert(CLASS.into(), class);
    Ok(class)
}

/// Recreate empty lexical slots on first access after frame.clear(), without
/// retaining any of the values or closure cells released by the clear.
fn frame_scope(ctx: &mut Context, frame: Id) -> Result<(Id, Id, bool)> {
    let Value::PythonFrame {
        scope,
        code,
        unoptimized_locals,
        class_proxy_scope,
        ..
    } = *ctx.heap.get(frame)
    else {
        unreachable!("a proxy owns a Python frame")
    };
    if let Some(cells) = class_proxy_scope {
        return Ok((cells, NONE, true));
    }
    if scope != NONE {
        return Ok((scope, code, unoptimized_locals.is_none()));
    }
    let layout = ctx.code_object(code);
    let mut declared = if matches!(ctx.heap.get(layout.parameters), Value::Str(_)) {
        ctx.parsed_parameters(code)?.locals.clone()
    } else {
        Default::default()
    };
    let freevars = layout.definition.freevars();
    if !freevars.is_empty() {
        declared.extend(freevars.iter().cloned());
    }
    let empty = ctx.scope("", None);
    let Value::Scope(data) = ctx.heap.get_mut(empty) else {
        unreachable!()
    };
    data.closure.cleared = true;
    data.closure.declared = declared;
    let Value::PythonFrame { scope, .. } = ctx.heap.get_mut(frame) else {
        unreachable!()
    };
    *scope = empty;
    Ok((empty, code, unoptimized_locals.is_none()))
}

/// A class proxy exposes its captured slots, independently of the class namespace.
pub(super) fn class_cells(ctx: &mut Context, scope: Id, code: Id) -> Option<Id> {
    let data = ctx.scope_data(scope);
    if !data.in_class_body {
        return None;
    }
    let parent = data.parent?;
    #[derive(serde::Deserialize)]
    struct Bindings {
        #[serde(default)]
        scope_reads: Vec<String>,
        #[serde(default)]
        needs_class_cell: bool,
        #[serde(default)]
        needs_dict_cell: bool,
    }
    let Value::Code(code) = ctx.heap.get(code) else {
        return None;
    };
    let Value::Str(signature) = ctx.heap.get(code.parameters) else {
        return None;
    };
    let bindings: Bindings = serde_json::from_str(signature).ok()?;
    let mut names = Vec::new();
    let mut cells = Vec::new();
    if bindings.needs_class_cell
        && let Some(cell) = ctx.scope_data(scope).class_cell
    {
        names.push("__class__".to_owned());
        cells.push(cell);
    }
    if bindings.needs_dict_cell {
        names.push("__classdict__".to_owned());
        cells.push(ctx.class_annotation_dict_cell(scope));
    }
    for name in bindings.scope_reads {
        if ctx.is_enclosing_freevar(parent, &name) == Some(true) && !names.contains(&name) {
            cells.push(ctx.capture_free_cell(parent, &name));
            names.push(name);
        }
    }
    if cells.is_empty() {
        return None;
    }
    let globals = ctx.root_scope(parent);
    Some(ctx.closure_environment(globals, names, &cells))
}

/// Capture the actual namespace while a class body or other unoptimized scope is still active.
pub(super) fn unoptimized_namespace(ctx: &mut Context, scope: Id, code: Id) -> Result<Option<Id>> {
    let data = ctx.scope_data(scope);
    if data.parent.is_none() || data.exec_locals.is_some() || data.in_class_body {
        return ctx.locals_for_code(scope, code).map(Some);
    }
    Ok(None)
}

/// Return the actual namespace in unoptimized frames and a fresh view in optimized ones.
pub(super) fn get(ctx: &mut Context, frame: Id) -> Result<Id> {
    if let Value::PythonFrame {
        unoptimized_locals: Some(namespace),
        ..
    } = *ctx.heap.get(frame)
    {
        return Ok(namespace);
    }
    let class = class(ctx)?;
    Ok(ctx.alloc(Value::FrameLocalsProxy { class, frame }))
}

fn snapshot(ctx: &mut Context, frame: Id) -> Result<Id> {
    let (scope, code, optimized) = frame_scope(ctx, frame)?;
    if optimized {
        return ctx.locals_for_code(scope, code);
    }
    // A proxy explicitly constructed from an unoptimized frame exposes only extra keys.
    match ctx.scope_data(scope).closure.extra_locals {
        Some(extras) => builtins::call(ctx, "dict", &[extras], &[]),
        None => Ok(ctx.dict(Vec::new())),
    }
}

#[derive(Clone, Copy)]
enum Access {
    Read,
    Write(Id),
    Remove,
}

/// Resolve keys against lexical slots, including unbound names. An object
/// comparing equal to a local's name addresses the same slot.
fn access(ctx: &mut Context, frame: Id, key: Id, op: Access) -> Result<Option<Id>> {
    let (original, code, optimized) = frame_scope(ctx, frame)?;
    let mut names = indexmap::IndexSet::new();
    if optimized {
        names = ctx.frame_binding_names(original, code)?;
        names.extend(ctx.scope_data(original).closure.declared.iter().cloned());
    }
    let slots = names
        .iter()
        .map(|name| (ctx.heap.code_name(name), NONE))
        .collect();
    let slots = ctx.dict(slots);
    ctx.heap.keep(slots);
    let (hash, index) = ctx.find_dictionary_key(slots, key, None)?;
    // Hashing or comparing a Python key can clear this frame.
    let (scope, code, optimized) = frame_scope(ctx, frame)?;
    if let Some(index) = index {
        let name = &names[index];
        let mut owner = scope;
        loop {
            let data = ctx.scope_data(owner);
            let cell = if name == "__class__" {
                data.class_cell
            } else {
                None
            };
            if cell.is_some()
                || data.closure.declared.contains(name)
                || data.closure.cells.contains_key(name)
                || data.names.contains_key(name.as_str())
            {
                return match op {
                    Access::Read => Ok(match cell {
                        Some(cell) => {
                            let Value::Cell(value) = *ctx.heap.get(cell) else {
                                unreachable!()
                            };
                            value
                        }
                        None => ctx.scope_name(owner, name),
                    }),
                    Access::Write(value) => {
                        if let Some(cell) = cell {
                            ctx.heap.replace_cell(cell, Some(value));
                        } else {
                            if owner == scope
                                && optimized
                                && code != NONE
                                && !data.closure.cells.contains_key(name)
                                && let Some(&old) = data.names.get(name.as_str())
                                && old != value
                                && ctx
                                    .parsed_parameters(code)?
                                    .cell_layout
                                    .retains_displaced(name, data.closure.cleared)
                            {
                                ctx.heap.retain_overwritten_local(frame, old)?;
                            }
                            ctx.bind_local(owner, name, value);
                        }
                        Ok(None)
                    }
                    Access::Remove => Err(Error::new(
                        "ValueError",
                        "cannot remove local variables from FrameLocalsProxy",
                    )),
                };
            }
            let Some(parent) = data.parent else { break };
            owner = parent;
        }
    }
    let extras = match (ctx.scope_data(scope).closure.extra_locals, op) {
        (Some(extras), _) => extras,
        (None, Access::Read | Access::Remove) => return Ok(None),
        (None, Access::Write(_)) => {
            let extras = ctx.dict(Vec::new());
            let Value::Scope(data) = ctx.heap.get_mut(scope) else {
                unreachable!()
            };
            data.closure.extra_locals = Some(extras);
            extras
        }
    };
    ctx.heap.keep(extras);
    match op {
        Access::Read => {
            let (_, found) = ctx.find_dictionary_key(extras, key, Some(hash))?;
            Ok(found.map(|index| ctx.stored_entries(extras)[index].1))
        }
        Access::Write(value) => ctx
            .set_dictionary_key(extras, key, value, Some(hash), true)
            .map(|_| None),
        Access::Remove => builtins::remove_dict_item_hashed(ctx, extras, key, Some(hash)),
    }
}

pub(super) fn method(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{CLASS}.{name}() takes no keyword arguments"),
        ));
    }
    if name == "__new__" {
        if let &[class, frame] = args
            && Some(&class) == ctx.classes.get(CLASS)
            && matches!(ctx.heap.get(frame), Value::PythonFrame { .. })
        {
            return Ok(ctx.alloc(Value::FrameLocalsProxy { class, frame }));
        }
        return Err(Error::new(
            "TypeError",
            "FrameLocalsProxy() requires a frame",
        ));
    }
    let Some((&proxy, rest)) = args.split_first() else {
        return Err(Error::new("TypeError", "unbound frame locals proxy method"));
    };
    let Value::FrameLocalsProxy { frame, .. } = *ctx.heap.get(proxy) else {
        return Err(Error::new("TypeError", "expected a FrameLocalsProxy"));
    };
    match (name, rest) {
        ("__setitem__", [key, value]) => {
            return access(ctx, frame, *key, Access::Write(*value)).map(|_| NONE);
        }
        ("__delitem__" | "pop", [key]) | ("pop", [key, _]) => {
            let removed = access(ctx, frame, *key, Access::Remove)?
                .or_else(|| rest.get(1).copied())
                .ok_or_else(|| ctx.lookup_error("KeyError", *key))?;
            return Ok(if name == "pop" { removed } else { NONE });
        }
        ("__getitem__", [key]) => {
            if let Some(value) = access(ctx, frame, *key, Access::Read)? {
                return Ok(value);
            }
            let repr = builtins::call(ctx, "repr", &[*key], &[])?;
            return Err(Error::new(
                "KeyError",
                format!("local variable '{}' is not defined", ctx.str_value(repr)?),
            ));
        }
        ("__contains__", [key]) => {
            return Ok(builtins::boolean(
                access(ctx, frame, *key, Access::Read)?.is_some(),
            ));
        }
        ("get" | "setdefault", [key] | [key, _]) => {
            match access(ctx, frame, *key, Access::Read) {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(error) if ctx.is_builtin_error(&error, "KeyError") => {}
                Err(error) => return Err(error),
            }
            let default = rest.get(1).copied().unwrap_or(NONE);
            if name == "setdefault" {
                access(ctx, frame, *key, Access::Write(default))?;
            }
            return Ok(default);
        }
        ("__eq__" | "__ne__", [other]) => {
            if let Value::FrameLocalsProxy {
                frame: other_frame, ..
            } = *ctx.heap.get(*other)
            {
                return Ok(builtins::boolean(
                    (frame == other_frame) == (name == "__eq__"),
                ));
            }
            if ctx.dict_storage(*other).is_none() {
                return Ok(NOT_IMPLEMENTED);
            }
        }
        _ => {}
    }
    let dict = snapshot(ctx, frame)?;
    ctx.heap.keep(dict);
    match (name, rest) {
        ("copy", []) => Ok(dict),
        ("__len__", []) => builtins::call(ctx, "len", &[dict], &[]),
        ("__iter__", []) => ctx.iterator(dict),
        ("__repr__", []) => {
            if ctx.repr_ancestors.contains(&proxy) {
                return Ok(ctx.text("{...}"));
            }
            ctx.repr_ancestors.push(proxy);
            let result = builtins::call(ctx, "repr", &[dict], &[]);
            ctx.repr_ancestors.pop();
            result
        }
        ("__reversed__", []) => {
            let it = builtins::call(ctx, "reversed", &[dict], &[])?;
            builtins::call(ctx, "list", &[it], &[])
        }
        ("__eq__" | "__ne__", [other]) => {
            ctx.binary(if name == "__eq__" { "==" } else { "!=" }, dict, *other)
        }
        ("keys" | "items" | "values", []) => {
            let method = ctx.lookup_attr(dict, name)?;
            let view = ctx.call(method, Vec::new(), Vec::new())?;
            builtins::call(ctx, "list", &[view], &[])
        }
        ("update" | "__ior__" | "__or__" | "__ror__", [other]) => {
            let other = if let Value::FrameLocalsProxy { frame, .. } = *ctx.heap.get(*other) {
                snapshot(ctx, frame)?
            } else if ctx.dict_storage(*other).is_some() {
                *other
            } else if name == "update" {
                return Err(Error::new(
                    "TypeError",
                    "update() argument must be dict or another FrameLocalsProxy",
                ));
            } else {
                return Ok(NOT_IMPLEMENTED);
            };
            if name == "__or__" {
                return ctx.binary("|", dict, other);
            }
            if name == "__ror__" {
                return ctx.binary("|", other, dict);
            }
            ctx.for_each_mapping_item(
                other,
                dict,
                MappingOperation::Update,
                |ctx, key, value, _hash| access(ctx, frame, key, Access::Write(value)).map(|_| ()),
            )?;
            Ok(if name == "update" { NONE } else { proxy })
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {CLASS}.{name}"),
        )),
    }
}
