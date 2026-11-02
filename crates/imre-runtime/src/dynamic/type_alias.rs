//! A PEP 695 declaration retains its evaluator and caches its first successful value.
use super::{Context, Error, Id, NONE, Result, Value, builtins, typing};

#[derive(Clone, Copy)]
pub(super) struct TypeAlias {
    pub name: Id,
    pub params: Id,
    pub module: Id,
    pub evaluator: Option<Id>,
    pub value: Option<Id>,
}

pub(super) fn declare(ctx: &mut Context, name: Id, evaluator: Id) -> Id {
    let params = ctx.heap.packed_tuple(Vec::new(), false);
    ctx.alloc(Value::TypeAlias(TypeAlias {
        name,
        params,
        module: NONE,
        evaluator: Some(evaluator),
        value: None,
    }))
}

pub(super) fn construct(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if args.len() + kwargs.len() <= 3 {
        if args.len() > 2 {
            return Err(Error::new(
                "TypeError",
                format!(
                    "typealias() takes exactly 2 positional arguments ({} given)",
                    args.len()
                ),
            ));
        }
        for (i, expected) in ["name", "value"].into_iter().enumerate() {
            if args.len() <= i && !kwargs.iter().any(|(key, _)| key == expected) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "typealias() missing required argument '{expected}' (pos {})",
                        i + 1
                    ),
                ));
            }
        }
    }
    let [name, value, params] = builtins::bind_method_arguments(
        "typealias",
        args,
        kwargs,
        ["name", "value", "type_params"],
        2,
    )?;
    let name = name.expect("required name");
    let value = value.expect("required value");
    if ctx.string_storage(name).is_none() {
        return Err(Error::new(
            "TypeError",
            format!(
                "typealias() argument 'name' must be str, not {}",
                if name == NONE {
                    "None"
                } else {
                    ctx.builtin_type_name(name)
                        .unwrap_or_else(|| ctx.type_name(name))
                }
            ),
        ));
    }
    let params = match params {
        None => ctx.heap.packed_tuple(Vec::new(), false),
        Some(tuple) => {
            let Value::Tuple(items) = ctx.heap.get(ctx.scalar_value(tuple)) else {
                return Err(Error::new("TypeError", "type_params must be a tuple"));
            };
            let empty = items.is_empty();
            let mut default_seen = false;
            for param in items.clone() {
                let has_default = match ctx.heap.get(param) {
                    Value::TypeVar(_) => false,
                    Value::ParamSpec(p) => !matches!(ctx.heap.get(p.default), Value::NoDefault),
                    Value::TypeVarTuple { default, .. } => {
                        !matches!(ctx.heap.get(*default), Value::NoDefault)
                    }
                    _ => {
                        return Err(Error::new(
                            "TypeError",
                            format!(
                                "Expected a type param, got {}",
                                ctx.checked_format(param, true)?
                            ),
                        ));
                    }
                };
                if default_seen && !has_default {
                    return Err(Error::new(
                        "TypeError",
                        format!(
                            "non-default type parameter '{}' follows default type parameter",
                            ctx.repr(param)
                        ),
                    ));
                }
                default_seen |= has_default;
            }
            if empty {
                ctx.heap.packed_tuple(Vec::new(), false)
            } else {
                tuple
            }
        }
    };
    let module = ctx.heap.caller_module();
    Ok(ctx.alloc(Value::TypeAlias(TypeAlias {
        name,
        params,
        module,
        evaluator: None,
        value: Some(value),
    })))
}

pub(super) fn attribute(
    ctx: &mut Context,
    owner: Id,
    alias: TypeAlias,
    name: &str,
) -> Option<Result<Id>> {
    Some(match name {
        "__name__" => Ok(alias.name),
        "__type_params__" => Ok(alias.params),
        "__parameters__" => parameters(ctx, alias.params),
        "__module__" => match alias.evaluator {
            Some(function) => ctx.lookup_attr(function, "__module__"),
            None => Ok(alias.module),
        },
        "evaluate_value" => Ok(alias
            .evaluator
            .unwrap_or_else(|| ctx.alloc(Value::ConstEvaluator(alias.value.unwrap())))),
        "__value__" => match alias.value {
            Some(value) => Ok(value),
            None => {
                // Evaluation may execute arbitrary Python and collect garbage.
                ctx.heap.keep(owner);
                ctx.call(alias.evaluator.unwrap(), Vec::new(), Vec::new())
                    .map(|value| {
                        let Value::TypeAlias(a) = ctx.heap.get_mut(owner) else {
                            unreachable!()
                        };
                        a.value = Some(value);
                        value
                    })
            }
        },
        "__reduce__" => {
            let callable = ctx.builtin("typing.TypeAliasType.__reduce__");
            Ok(ctx.alloc(Value::Method {
                callable,
                receiver: owner,
                explicit: false,
            }))
        }
        _ => return None,
    })
}

fn parameters(ctx: &mut Context, tuple: Id) -> Result<Id> {
    let Value::Tuple(params) = ctx.heap.get(ctx.scalar_value(tuple)) else {
        unreachable!()
    };
    if !params
        .iter()
        .any(|&p| matches!(ctx.heap.get(p), Value::TypeVarTuple { .. }))
    {
        return Ok(tuple);
    }
    let mut params = params.clone();
    for param in &mut params {
        if matches!(ctx.heap.get(*param), Value::TypeVarTuple { .. }) {
            *param = typing::unpack_parameter(ctx, *param)?;
        }
    }
    Ok(ctx.alloc(Value::Tuple(params)))
}
