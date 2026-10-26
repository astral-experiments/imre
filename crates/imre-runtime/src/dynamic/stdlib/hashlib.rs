use std::cell::RefCell;
use std::rc::Rc;

use super::{Context, Error, Id, NONE, Result, Value, argument, option};
use crate::objects::HashState;

fn buffer(ctx: &Context, id: Id) -> Result<&[u8]> {
    match ctx.heap.get(id) {
        Value::Bytes(data) => Ok(data),
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

pub(super) fn sha1(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let data = option(args, 0, kwargs, "data")?;
    ctx.optional_truth(
        kwargs
            .iter()
            .find_map(|(key, id)| (key == "usedforsecurity").then_some(*id)),
        true,
    )?;
    let data = data
        .map(|id| buffer(ctx, id))
        .transpose()?
        .unwrap_or_default();
    let state = HashState::sha1(data);
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
