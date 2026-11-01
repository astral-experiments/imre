use std::cell::RefCell;
use std::rc::Rc;

use super::{Context, Error, Id, NONE, Result, Value, argument, builtins};
use crate::objects::HashState;

/// Borrows the bytes of a boxed hash input, including mutable buffers.
pub(super) fn buffer(ctx: &Context, id: Id) -> Result<&[u8]> {
    match ctx.heap.get(ctx.scalar_value(id)) {
        Value::Bytes(data) | Value::ByteArray(data) => Ok(data),
        Value::Str(_) => Err(Error::new(
            "TypeError",
            "Strings must be encoded before hashing",
        )),
        _ => Err(Error::new(
            "TypeError",
            "object supporting the buffer API required",
        )),
    }
}

fn hash_input(
    ctx: &mut Context,
    data: Option<Id>,
    flag: Option<Id>,
    string: Option<Id>,
) -> Result<&[u8]> {
    ctx.optional_truth(flag, true)?;
    if data.is_some() && string.is_some() {
        return Err(Error::new(
            "TypeError",
            "'data' and 'string' are mutually exclusive and support for 'string' keyword \
             parameter is slated for removal in a future version.",
        ));
    }
    data.or(string)
        .map(|id| buffer(ctx, id))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn hash(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [data, flag, string] = builtins::bind_method_arguments(
        name,
        args,
        kwargs,
        ["data", "usedforsecurity", "string"],
        1,
    )?;
    let input = hash_input(ctx, data, flag, string)?;
    let state = match name {
        "sha1" => HashState::sha1(input),
        "sha256" => HashState::sha256(input),
        _ => unreachable!("registered hash constructor"),
    };
    Ok(ctx.alloc(Value::Hash(Rc::new(RefCell::new(state)))))
}

pub(super) fn algorithms(ctx: &mut Context) -> Result<Id> {
    let set = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    for name in HashState::ALGORITHMS {
        let key = ctx.text(name);
        super::builtins::set_add(ctx, set, key)?;
    }
    Ok(set)
}

pub(super) fn new(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    // The public Python wrapper binds name before the provider binds the remaining arguments.
    let name = super::option(args, 0, kwargs, "name")
        .map_err(|_| {
            Error::new(
                "TypeError",
                "__hash_new() got multiple values for argument 'name'",
            )
        })?
        .ok_or_else(|| {
            Error::new(
                "TypeError",
                "__hash_new() missing 1 required positional argument: 'name'",
            )
        })?;
    let [_, data, flag, string] = super::builtins::bind_method_arguments(
        "new",
        args,
        kwargs,
        ["name", "data", "usedforsecurity", "string"],
        2,
    )?;
    let name = ctx.str_value(name).map_err(|_| {
        let ty = if name == NONE {
            "None"
        } else {
            ctx.type_name(name)
        };
        Error::new(
            "TypeError",
            format!("new() argument 'name' must be str, not {ty}"),
        )
    })?;
    let constructor = HashState::constructor(name);
    let data = hash_input(ctx, data, flag, string)?;
    let state = constructor?(data);
    Ok(ctx.alloc(Value::Hash(Rc::new(RefCell::new(state)))))
}

pub(super) fn method(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let Value::Hash(hash) = ctx.heap.get(argument(args, 0)?) else {
        unreachable!("bound hash method");
    };
    let value = match name {
        "hash.update" => {
            hash.borrow_mut().update(buffer(ctx, argument(args, 1)?)?);
            return Ok(NONE);
        }
        "hash.copy" => Value::Hash(Rc::new(RefCell::new(hash.borrow().clone()))),
        "hash.digest" => Value::Bytes(hash.borrow().digest(<[u8]>::to_vec)),
        "hash.hexdigest" => Value::Str(
            hash.borrow()
                .hexdigest()
                .map_err(|_| Error::new("MemoryError", "could not allocate hexadecimal digest"))?
                .to_string(),
        ),
        _ => unreachable!("registered hash method"),
    };
    Ok(ctx.alloc(value))
}
