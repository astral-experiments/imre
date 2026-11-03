//! Coroutine function flags and the identity marker from Lib/inspect.py.
use super::{Context, FALSE, Id, Result, TRUE, Value};

pub(super) fn marker(ctx: &mut Context) -> Id {
    let key = "inspect._is_coroutine_mark";
    if let Some(&value) = ctx.builtins.get(key) {
        return value;
    }
    let class = ctx.builtin("object");
    let value = ctx.instance(class, None);
    ctx.builtins.insert(key.to_owned(), value);
    value
}

fn attribute(ctx: &mut Context, obj: Id, name: &str) -> Result<Option<Id>> {
    match ctx.lookup_attr(obj, name) {
        Err(error) if ctx.is_attribute_error(&error) => Ok(None),
        result => result.map(Some),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "_static_member" {
        if let [obj, attr, missing] = *args
            && kwargs.is_empty()
        {
            return static_member(ctx, obj, attr, missing);
        }
        return Err(super::Error::new(
            "TypeError",
            "_static_member expects three positional arguments",
        ));
    }
    let param = if name == "markcoroutinefunction" {
        "func"
    } else {
        "obj"
    };
    let mut obj = super::single_argument(name, param, args, kwargs)?;
    match name {
        "_native_data_descriptor" => {
            return Ok(if ctx.native_data_descriptor(obj) {
                TRUE
            } else {
                FALSE
            });
        }
        "_frame_position" => match *ctx.heap.get(obj) {
            Value::Traceback { .. } => return Ok(super::traceback::tb_position(ctx, obj)),
            Value::PythonFrame { lineno, .. } => {
                let fields = match ctx.frame_position(obj) {
                    Some(range) => range.map(|v| ctx.int(i64::from(v))),
                    None => [
                        ctx.int(i64::from(lineno)),
                        super::NONE,
                        super::NONE,
                        super::NONE,
                    ],
                };
                return Ok(ctx.alloc(Value::Tuple(fields.to_vec())));
            }
            _ => {
                return Err(super::Error::new(
                    "TypeError",
                    "expected a frame or traceback",
                ));
            }
        },
        "_signature_layout" => return layout(ctx, obj),
        _ => {}
    }
    if name == "markcoroutinefunction" {
        obj = attribute(ctx, obj, "__func__")?.unwrap_or(obj);
        let mark = marker(ctx);
        ctx.set_attr(obj, "_is_coroutine_marker", mark)?;
        return Ok(obj);
    }
    loop {
        match ctx.heap.get(obj) {
            Value::Method { callable, .. }
                if matches!(ctx.heap.get(*callable), Value::Function { .. }) =>
            {
                obj = *callable
            }
            Value::Partial { function, .. } => obj = *function,
            Value::Function { code, .. } if ctx.code_object(*code).coroutine => return Ok(TRUE),
            _ => break,
        }
    }
    let marked =
        attribute(ctx, obj, "_is_coroutine_marker")?.is_some_and(|value| value == marker(ctx));
    Ok(if marked { TRUE } else { FALSE })
}

/// Read the same argument layout used to bind the boxed function's calls.
fn layout(ctx: &mut Context, obj: Id) -> Result<Id> {
    let code = match ctx.heap.get(obj) {
        Value::Function { code, .. } => ctx.code_object(*code),
        Value::Code(code) => code,
        _ => {
            return Err(super::Error::new(
                "TypeError",
                "expected a Python function or code object",
            ));
        }
    };
    if code.parameters == super::NONE {
        // Module suites and eval expressions are compiled without function parameters.
        if ctx.str_value(code.name)? == "<module>" {
            return Ok(ctx.alloc(Value::Tuple(Vec::new())));
        }
        return Err(super::Error::new(
            "NotImplementedError",
            "code lacks a parameter layout",
        ));
    }
    let parameters = code.parameters;
    let spec: serde_json::Value = serde_json::from_str(ctx.str_value(parameters)?)
        .map_err(|error| super::Error::new("SystemError", error.to_string()))?;
    let mut result = Vec::new();
    for (index, key) in ["posonly", "positional", "varargs", "kwonly", "kwargs"]
        .iter()
        .enumerate()
    {
        let kind = ctx.int(index as i64);
        let names = &spec[*key];
        let mut append = |name: &str| {
            let name = ctx.text(name);
            result.push(ctx.alloc(Value::Tuple(vec![kind, name])));
        };
        if let Some(names) = names.as_array() {
            for name in names.iter().filter_map(serde_json::Value::as_str) {
                append(name);
            }
        } else if let Some(name) = names.as_str() {
            append(name);
        }
    }
    Ok(ctx.alloc(Value::Tuple(result)))
}

/// A user-supplied __dict__ prevents inspect from relying on that namespace.
fn shadowed_dict(ctx: &Context, class: Id) -> Option<Id> {
    ctx.find_in_mro(class, |entry| {
        let value = ctx.own_class_attr(entry, "__dict__")?;
        if matches!(ctx.heap.get(value), Value::LayoutDescriptor(owner, "__dict__") if *owner == entry)
            || matches!(ctx.heap.get(value), Value::GetSetDescriptor { owner, name: "__dict__", .. }
            if ctx.builtins.get(*owner) == Some(&entry))
        {
            return None;
        }
        Some(value)
    })
}

/// Read the stored class namespaces without invoking metaclass hooks or binding a member.
fn class_member(ctx: &mut Context, class: Id, name: &str) -> Option<Id> {
    for entry in ctx.base_lineage(class, true) {
        if let Value::Class {
            metaclass: Some(meta),
            ..
        } = ctx.heap.get(entry)
            && shadowed_dict(ctx, *meta).is_some()
        {
            continue;
        }
        // Python class frames retain __qualname__ for type's descriptor, outside
        // the published class dictionary (see Context::class_dictionary).
        if matches!(ctx.heap.get(entry), Value::Class { .. }) && name == "__qualname__" {
            continue;
        }
        if let Some(value) = ctx.own_class_attr(entry, name) {
            return Some(value);
        }
        // Some native wrapper and method descriptors are interned on first access.
        if matches!(ctx.heap.get(entry), Value::Native(owner) if owner == "object") {
            if let Some(member) = ctx.raw_object_member(name) {
                return Some(member);
            }
            continue;
        }
        if let Value::Native(owner) = ctx.heap.get(entry) {
            let symbol = format!("{owner}.{name}");
            if crate::dynamic::wrapper_descriptor::BuiltinDescriptor::lookup(&symbol).is_some() {
                return Some(ctx.builtin(&symbol));
            }
        }
    }
    None
}

fn static_member(ctx: &mut Context, obj: Id, attr: Id, missing: Id) -> Result<Id> {
    let objtype = ctx.exact_type(obj)?;
    let is_class = matches!(ctx.heap.get(obj), Value::Class { .. })
        || ctx.builtin_type_name(obj) == Some("type");
    let class = if is_class { obj } else { objtype };
    let instance = if is_class
        || shadowed_dict(ctx, class).is_some_and(|shadow| {
            !matches!(
                ctx.heap.get(shadow),
                Value::SlotMember { .. } | Value::ReadOnlyMember(_, _)
            )
        }) {
        None
    } else {
        let dict = match *ctx.heap.get(obj) {
            Value::Instance { .. } if ctx.class_instances_have_dict(class) => {
                Some(ctx.instance_dictionary(obj, class))
            }
            Value::Module { scope, .. } => Some(ctx.module_globals(scope)?),
            Value::Function { fields, .. } => Some(fields.dict),
            Value::Raised(ref error) => error.trace.as_ref().and_then(|trace| trace.dictionary),
            Value::Partial { dictionary, .. } => dictionary,
            Value::Cached { dictionary, .. }
            | Value::ClassMethod { dictionary, .. }
            | Value::StaticMethod { dictionary, .. } => Some(dictionary),
            _ => None,
        };
        if let Some(storage) = dict.and_then(|dict| ctx.dict_storage(dict)) {
            let (_, index) = ctx.find_dictionary_key(storage, attr, None)?;
            index.map(|i| ctx.dict_entries(storage).unwrap()[i].1)
        } else {
            None
        }
    };
    let Ok(name) = ctx.str_value(attr).map(str::to_owned) else {
        // Class namespaces have string keys; an unhashable key still raises.
        ctx.dictionary_hash(attr)?;
        return Ok(instance.unwrap_or(missing));
    };
    let instance = instance.filter(|value| *value != missing);
    let member = class_member(ctx, class, &name).filter(|value| *value != missing);
    if ctx.is_inline_instance_slot(class, &name) {
        return Err(super::Error::new(
            "NotImplementedError",
            "inline slot has no boxed member descriptor",
        ));
    }
    if let (Some(value), Some(member)) = (instance, member) {
        if ctx.native_data_descriptor(member) {
            return Ok(member);
        }
        if let Some(typ) = ctx.heap.get(member).instance_class()
            && class_member(ctx, typ, "__get__").is_some()
            && (class_member(ctx, typ, "__set__").is_some()
                || class_member(ctx, typ, "__delete__").is_some())
        {
            return Ok(member);
        }
        return Ok(value);
    }
    if let Some(value) = member.or_else(|| {
        is_class
            .then(|| class_member(ctx, objtype, &name))
            .flatten()
    }) {
        return Ok(value);
    }
    if name == "__class__"
        || (name == "__dict__" && (is_class || ctx.class_instances_have_dict(class)))
        || (is_class && name == "__qualname__")
    {
        return Err(super::Error::new(
            "NotImplementedError",
            format!("'{name}' has no boxed getset descriptor"),
        ));
    }
    Ok(instance.unwrap_or(missing))
}
