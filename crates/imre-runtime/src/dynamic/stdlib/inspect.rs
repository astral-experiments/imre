//! Native descriptor, frame and signature layout helpers for inspect.
use super::{Context, FALSE, Id, Result, TRUE, Value};

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if name == "_descriptor_get" {
        if let [descriptor, obj] = *args
            && kwargs.is_empty()
        {
            // As in inspect._descriptor_get, class objects supply their own signature.
            if ctx.is_type_object(descriptor) {
                return Ok(descriptor);
            }
            let owner = ctx.exact_type(obj)?;
            return ctx.bind_attribute(descriptor, obj, owner);
        }
        return Err(super::Error::new(
            "TypeError",
            "_descriptor_get expects two positional arguments",
        ));
    }
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
    let [obj] = super::required_arguments(name, ["obj"], args, kwargs)?;
    match name {
        "_abstract_flags" => {
            let target = ctx.weak_proxy_target(obj)?;
            let abstract_class = match *ctx.heap.get(target) {
                Value::Class {
                    scope, metaclass, ..
                } => {
                    // The default type descriptor takes precedence over a class-body __flags__.
                    if metaclass.is_some_and(|meta| {
                        ctx.class_attr(meta, "__flags__").is_some()
                            || ctx.class_attr(meta, "__getattribute__").is_some()
                    }) {
                        return ctx.lookup_attr(obj, "__flags__");
                    }
                    ctx.scope_data(scope).is_abstract
                }
                _ if ctx.is_type_object(target) => false,
                // isinstance(obj, type) can also succeed through a reported __class__.
                _ => return ctx.lookup_attr(obj, "__flags__"),
            };
            Ok(ctx.int(if abstract_class { 1 << 20 } else { 0 }))
        }
        "_native_data_descriptor" => Ok(if ctx.native_data_descriptor(obj) {
            TRUE
        } else {
            FALSE
        }),
        "_frame_position" => match *ctx.heap.get(obj) {
            Value::Traceback { .. } => Ok(super::traceback::tb_position(ctx, obj)),
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
                Ok(ctx.alloc(Value::Tuple(fields.to_vec())))
            }
            _ => Err(super::Error::new(
                "TypeError",
                "expected a frame or traceback",
            )),
        },
        _ => Err(super::Error::new(
            "SystemError",
            format!("unknown inspect helper {name}"),
        )),
    }
}

/// A user-supplied __dict__ prevents inspect from relying on that namespace.
fn shadowed_dict(ctx: &Context, class: Id) -> Option<Id> {
    ctx.find_in_mro(class, |entry| {
        let value = ctx.own_class_attr(entry, "__dict__")?;
        let owner = match *ctx.heap.get(value) {
            Value::LayoutDescriptor(owner, "__dict__") => Some(owner),
            Value::GetSetDescriptor {
                get,
                owner,
                name: "__dict__",
                ..
            } => match *ctx.heap.get(get) {
                Value::IoMethod { owner, .. } | Value::HeapTypeMethod { owner, .. } => Some(owner),
                _ => ctx.builtins.get(owner).copied(),
            },
            _ => None,
        };
        (owner != Some(entry)).then_some(value)
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
    let cleared = !is_class
        && ctx.layout_descriptors_cleared(class)
        && ctx.class_attr(class, "__dict__").is_none();
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
            Value::Instance { .. } if cleared => ctx.raw_instance_field(obj, "__dict__"),
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
            ref value => value.native_file_dict(),
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
        || (name == "__dict__" && (is_class || (!cleared && ctx.class_instances_have_dict(class))))
        || (is_class && name == "__qualname__")
    {
        return Err(super::Error::new(
            "NotImplementedError",
            format!("'{name}' has no boxed getset descriptor"),
        ));
    }
    Ok(instance.unwrap_or(missing))
}
