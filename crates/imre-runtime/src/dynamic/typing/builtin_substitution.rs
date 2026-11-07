//! Parameter substitution for types.GenericAlias follows the operand protocols in
//! CPython's Objects/genericaliasobject.c independently of typing's alias classes.
mod prepare;

use super::{Alias, builtin_parameters};
use crate::dynamic::{Context, Error, Id, NONE, Result, Value};

/// Specialization creates a fresh alias and preserves the original's unpacking flag.
pub(super) fn specialize(ctx: &mut Context, owner: Id, alias: &Alias, item: Id) -> Result<Id> {
    let mark = ctx.heap.temporary_mark();
    ctx.heap.reserve_temporaries(2)?;
    ctx.heap.keep(owner);
    ctx.heap.keep(item);
    let result = (|| {
        let params = builtin_parameters(ctx, owner, alias.args, alias.parameters)?;
        let params = ctx.tuple_items(params)?.expect("generic alias parameters");
        if params.is_empty() {
            let name = ctx.checked_format(owner, true)?;
            return Err(Error::new(
                "TypeError",
                format!("{name} is not a generic class"),
            ));
        }
        let args = substitute(ctx, owner, alias.args, &params, item)?;
        Ok(ctx.alloc(Value::GenericAlias(Alias {
            args,
            parameters: NONE,
            ..alias.clone()
        })))
    })();
    ctx.heap.release_temporaries_since(mark);
    match &result {
        Ok(id) => ctx.heap.keep(*id),
        Err(error) => {
            for id in error.roots() {
                ctx.heap.keep(id);
            }
        }
    }
    result
}

/// Lists and tuples inside the alias run the same substitution against their own snapshot.
fn substitute(ctx: &mut Context, owner: Id, args: Id, params: &[Id], item: Id) -> Result<Id> {
    ctx.with_slot_depth("maximum recursion depth exceeded", |ctx| {
        let item = prepare::prepared(ctx, owner, params, item)?;
        ctx.heap.keep(item);
        let values = ctx.tuple_items(item)?.unwrap_or_else(|| vec![item]);
        if values.len() != params.len() {
            let which = if values.len() < params.len() {
                "few"
            } else {
                "many"
            };
            let name = ctx.checked_format(owner, true)?;
            return Err(Error::new(
                "TypeError",
                format!(
                    "Too {which} arguments for {name}; actual {}, expected {}",
                    values.len(),
                    params.len()
                ),
            ));
        }
        let list = ctx.list_storage(args).is_some();
        let args = if list && !matches!(ctx.heap.get(args), Value::List(_)) {
            let iter = ctx.iterator(args)?;
            ctx.items(iter)?
        } else if list {
            ctx.items(args)?
        } else {
            ctx.tuple_items(args)?.expect("alias argument tuple")
        };
        // A descriptor may clear the source list or collect an earlier substitution result.
        ctx.heap.reserve_temporaries(args.len())?;
        for &arg in &args {
            ctx.heap.keep(arg);
        }
        let mut result = Vec::new();
        for arg in args {
            if ctx.is_type_object(arg) {
                result.push(arg);
                continue;
            }
            if ctx.list_storage(arg).is_some() || ctx.builtin_slot_accepts(arg, "tuple") {
                let nested = substitute(ctx, owner, arg, params, item)?;
                ctx.heap.keep(nested);
                result.push(nested);
                continue;
            }
            let marker = ctx.optional_attr(arg, "__typing_is_unpacked_typevartuple__")?;
            let unpack = ctx.optional_truth(marker, false)?;
            let value = if let Some(hook) = ctx.optional_attr(arg, "__typing_subst__")? {
                let index = params.iter().position(|&p| p == arg).ok_or_else(|| {
                    Error::new("TypeError", "substitution parameter is not in the alias")
                })?;
                ctx.call(hook, vec![values[index]], Vec::new())?
            } else if matches!(ctx.heap.get(arg), Value::TypeVarTuple { .. }) {
                return Err(Error::new(
                    "TypeError",
                    "Substitution of bare TypeVarTuple is not supported",
                ));
            } else {
                substitute_nested(ctx, arg, params, &values)?
            };
            ctx.heap.keep(value);
            if unpack {
                let elements = ctx.tuple_items(value)?.ok_or_else(|| {
                    Error::new(
                        "TypeError",
                        format!(
                            "expected __typing_subst__ of {} objects to return a tuple, not {}",
                            type_name(ctx, arg),
                            type_name(ctx, value)
                        ),
                    )
                })?;
                result.extend(elements);
            } else {
                result.push(value);
            }
        }
        Ok(ctx.alloc(if list {
            Value::List(result)
        } else {
            Value::Tuple(result)
        }))
    })
}

/// Objects without a substitution hook can expose their parameters and handle their own subscription.
fn substitute_nested(ctx: &mut Context, arg: Id, params: &[Id], values: &[Id]) -> Result<Id> {
    let Some(metadata) = ctx.optional_attr(arg, "__parameters__")? else {
        return Ok(arg);
    };
    let Some(nested) = ctx.tuple_items(metadata)? else {
        return Ok(arg);
    };
    if nested.is_empty() {
        return Ok(arg);
    }
    let mut args = Vec::new();
    for p in nested {
        if let Some(i) = params.iter().position(|&param| param == p) {
            let value = values[i];
            if ctx.has_iter_slot(p)
                && let Some(items) = ctx.tuple_items(value)?
            {
                args.extend(items);
            } else {
                args.push(value);
            }
        } else {
            args.push(p);
        }
    }
    let key = ctx.alloc(Value::Tuple(args));
    ctx.get_item(arg, key)
}

fn type_name(ctx: &Context, value: Id) -> String {
    if let Some(class) = ctx.heap.get(value).instance_class()
        && let Value::Class { scope, .. } = ctx.heap.get(class)
    {
        let name = ctx.qualified_class_name(class, *scope);
        return name.strip_prefix("__main__.").unwrap_or(&name).to_owned();
    }
    ctx.builtin_type_name(value)
        .unwrap_or_else(|| ctx.type_name(value))
        .to_owned()
}
