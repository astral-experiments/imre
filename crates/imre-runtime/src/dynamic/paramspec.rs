//! Native parameter specifications and their positional and keyword markers.

use super::{Context, Error, Id, NONE, Result, Value, builtins, typing, value::NOT_IMPLEMENTED};

#[derive(Clone, Copy)]
pub(super) struct ParamSpec {
    pub name: Id,
    pub bound: Id,
    pub default: Id,
    pub namespace: Id,
    pub covariant: bool,
    pub contravariant: bool,
    pub infer_variance: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Marker {
    Args,
    Kwargs,
}

impl Marker {
    pub fn class(self) -> &'static str {
        match self {
            Self::Args => "typing.ParamSpecArgs",
            Self::Kwargs => "typing.ParamSpecKwargs",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Args => ".args",
            Self::Kwargs => ".kwargs",
        }
    }
}

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    const NAMES: [&str; 6] = [
        "name",
        "bound",
        "covariant",
        "contravariant",
        "infer_variance",
        "default",
    ];
    if args.len() + kwargs.len() <= NAMES.len() {
        if args.len() > 1 {
            return Err(Error::new(
                "TypeError",
                format!(
                    "paramspec() takes exactly 1 positional argument ({} given)",
                    args.len()
                ),
            ));
        }
        if args.is_empty() && !kwargs.iter().any(|(name, _)| name == "name") {
            return Err(Error::new(
                "TypeError",
                "paramspec() missing required argument 'name' (pos 1)",
            ));
        }
    }
    let [
        name,
        bound,
        covariant,
        contravariant,
        infer_variance,
        default,
    ] = builtins::bind_method_arguments("paramspec", args, kwargs, NAMES, 1)?;
    let name = name.ok_or_else(|| {
        Error::new(
            "TypeError",
            "paramspec() missing required argument 'name' (pos 1)",
        )
    })?;
    if ctx.string_storage(name).is_none() {
        return Err(Error::new(
            "TypeError",
            format!(
                "paramspec() argument 'name' must be str, not {}",
                if name == NONE {
                    "None"
                } else {
                    ctx.type_name(name)
                }
            ),
        ));
    }
    let covariant = ctx.optional_truth(covariant, false)?;
    let contravariant = ctx.optional_truth(contravariant, false)?;
    let infer_variance = ctx.optional_truth(infer_variance, false)?;
    if covariant && contravariant {
        return Err(Error::new(
            "ValueError",
            "Bivariant types are not supported.",
        ));
    }
    if infer_variance && (covariant || contravariant) {
        return Err(Error::new(
            "ValueError",
            "Variance cannot be specified with infer_variance.",
        ));
    }
    let bound = match bound {
        None | Some(NONE) => ctx.builtin("NoneType"),
        Some(value) => {
            let module = ctx.module("typing")?;
            let check = ctx.lookup_attr(module, "_type_check")?;
            let message = ctx.text("Bound must be a type.");
            ctx.call(check, vec![value, message], Vec::new())?
        }
    };
    let default = default.unwrap_or_else(|| typing::no_default(ctx));
    let module = ctx.heap.caller_module();
    let module_key = ctx.text("__module__");
    let namespace = ctx.dict(vec![(module_key, module)]);
    Ok(ctx.alloc(Value::ParamSpec(ParamSpec {
        name,
        bound,
        default,
        namespace,
        covariant,
        contravariant,
        infer_variance,
    })))
}

pub(super) fn construct_marker(
    ctx: &mut Context,
    kind: Marker,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let constructor = if kind == Marker::Args {
        "paramspecargs"
    } else {
        "paramspeckwargs"
    };
    let [origin] = builtins::bind_method_arguments(constructor, args, kwargs, ["origin"], 1)?;
    let origin = origin.ok_or_else(|| {
        Error::new(
            "TypeError",
            format!("{constructor}() missing required argument 'origin' (pos 1)"),
        )
    })?;
    Ok(ctx.alloc(Value::ParamSpecMarker(kind, origin)))
}

const READONLY: &[&str] = &[
    "__name__",
    "__bound__",
    "__covariant__",
    "__contravariant__",
    "__infer_variance__",
];
const PROPERTIES: &[&str] = &["args", "kwargs", "__default__", "evaluate_default"];

pub(super) fn names() -> impl Iterator<Item = &'static str> {
    READONLY.iter().chain(PROPERTIES).copied().chain([
        "__module__",
        "__reduce__",
        "__typing_subst__",
        "__typing_prepare_subst__",
        "has_default",
        "__mro_entries__",
    ])
}

fn method(ctx: &mut Context, value: Id, name: &str) -> Id {
    let callable = ctx.builtin(name);
    ctx.alloc(Value::Method {
        callable,
        receiver: value,
        explicit: false,
    })
}

pub(super) fn attribute(ctx: &mut Context, value: Id, name: &str) -> Option<Result<Id>> {
    let result = match *ctx.heap.get(value) {
        Value::ParamSpec(p) => match name {
            "__name__" => p.name,
            "__bound__" => p.bound,
            "__default__" => p.default,
            "__covariant__" => builtins::boolean(p.covariant),
            "__contravariant__" => builtins::boolean(p.contravariant),
            "__infer_variance__" => builtins::boolean(p.infer_variance),
            "args" => ctx.alloc(Value::ParamSpecMarker(Marker::Args, value)),
            "kwargs" => ctx.alloc(Value::ParamSpecMarker(Marker::Kwargs, value)),
            "evaluate_default" => ctx.alloc(Value::ConstEvaluator(p.default)),
            _ if let Some(item) = ctx.dict_named_item(p.namespace, name) => item,
            "__module__" => ctx.text("typing"),
            "has_default" | "__reduce__" | "__typing_subst__" | "__typing_prepare_subst__" => {
                method(ctx, value, &format!("typing.ParamSpec.{name}"))
            }
            _ => return None,
        },
        Value::Native(ref class)
            if class == "typing.ParamSpec"
                && matches!(name, "__typing_subst__" | "__typing_prepare_subst__") =>
        {
            ctx.builtin(&format!("typing.ParamSpec.{name}"))
        }
        Value::ParamSpecMarker(kind, origin) => match name {
            "__origin__" => origin,
            "__eq__" | "__ne__" => method(ctx, value, &format!("{}.{name}", kind.class())),
            "__hash__" => NONE,
            _ => return None,
        },
        Value::Native(ref class)
            if matches!(
                class.as_str(),
                "typing.ParamSpecArgs" | "typing.ParamSpecKwargs"
            ) && matches!(name, "__eq__" | "__ne__" | "__hash__") =>
        {
            if name == "__hash__" {
                NONE
            } else {
                ctx.builtin(&format!("{class}.{name}"))
            }
        }
        _ => return None,
    };
    Some(Ok(result))
}

pub(super) fn update(
    ctx: &mut Context,
    obj: Id,
    name: &str,
    value: Option<Id>,
) -> Option<Result<()>> {
    let namespace = match *ctx.heap.get(obj) {
        Value::ParamSpecMarker(_, _) if name == "__origin__" => {
            return Some(Err(Error::new("AttributeError", "readonly attribute")));
        }
        Value::ParamSpec(p) => p.namespace,
        _ => return None,
    };
    if READONLY.contains(&name) {
        return Some(Err(Error::new("AttributeError", "readonly attribute")));
    }
    if PROPERTIES.contains(&name) {
        return Some(Err(Error::new(
            "AttributeError",
            format!("attribute '{name}' of 'typing.ParamSpec' objects is not writable"),
        )));
    }
    if name == "__class__" {
        return None;
    }
    let key = ctx.text(name);
    Some(if let Some(value) = value {
        ctx.put_item(namespace, key, value)
    } else if ctx.dict_named_item(namespace, name).is_some() {
        ctx.delete_item(namespace, key)
    } else {
        Err(ctx.missing_attribute(obj, name))
    })
}

pub(super) fn compare(ctx: &mut Context, op: &str, a: Id, b: Id) -> Result<Id> {
    match (ctx.heap.get(a), ctx.heap.get(b)) {
        (Value::ParamSpecMarker(left, x), Value::ParamSpecMarker(right, y)) if left == right => {
            ctx.binary(op, *x, *y)
        }
        _ => Ok(NOT_IMPLEMENTED),
    }
}

pub(super) fn repr(ctx: &mut Context, obj: Id) -> Option<Result<Id>> {
    match *ctx.heap.get(obj) {
        Value::ParamSpec(p) => Some((|| {
            let name = ctx.string_storage(p.name).unwrap();
            if p.infer_variance {
                return Ok(p.name);
            }
            let prefix = ctx.text(if p.covariant {
                "+"
            } else if p.contravariant {
                "-"
            } else {
                "~"
            });
            ctx.binary("+", prefix, name)
        })()),
        Value::ParamSpecMarker(kind, origin) => Some((|| {
            let text = if let Value::ParamSpec(p) = *ctx.heap.get(origin) {
                ctx.string_storage(p.name).unwrap()
            } else {
                let repr = ctx.builtin("repr");
                ctx.call(repr, vec![origin], Vec::new())?
            };
            let suffix = ctx.text(kind.suffix());
            ctx.binary("+", text, suffix)
        })()),
        _ => None,
    }
}

pub(super) fn eval(
    ctx: &mut Context,
    value: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "constevaluator.__call__() takes no keyword arguments",
        ));
    }
    let [format] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "constevaluator.__call__() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    };
    if ctx.index_c_int(*format)? != 4 {
        return Ok(value);
    }
    let format_one = |ctx: &mut Context, value| {
        let mut text = String::new();
        if !typing::append_type_label(ctx, value, &mut text)? {
            text = ctx.checked_format(value, true)?;
        }
        Ok::<_, Error>(text)
    };
    let text = if let Value::Tuple(items) = ctx.heap.get(value) {
        let mut texts = Vec::new();
        for item in items.clone() {
            texts.push(format_one(ctx, item)?);
        }
        format!("({})", texts.join(", "))
    } else {
        format_one(ctx, value)?
    };
    Ok(ctx.text(text))
}
