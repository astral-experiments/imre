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
    let state = (HashState::provider(name)?.create)(input);
    Ok(ctx.alloc(Value::Hash(Rc::new(RefCell::new(state)))))
}

pub(super) fn algorithms(ctx: &mut Context) -> Result<Id> {
    let set = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    for name in HashState::algorithms() {
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
    let provider = HashState::provider(name);
    let data = hash_input(ctx, data, flag, string)?;
    let state = (provider?.create)(data);
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
        _ => unreachable!("registered hash method"),
    };
    Ok(ctx.alloc(value))
}

pub(super) fn digest(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Value::Hash(hash) = ctx.heap.get(argument(args, 0)?) else {
        unreachable!("bound hash method");
    };
    let hash = Rc::clone(hash);
    let positional = &args[1..];
    let hex = method == "hexdigest";
    let value = if hash.borrow().is_xof() {
        let [length] = builtins::bind_builtin_arguments(method, positional, kwargs, ["length"], 1)?;
        let length = ctx.index_integer(length.expect("bound required length"))?;
        if hex {
            Value::Str(hash.borrow().xof_hexdigest(&length)?.to_string())
        } else {
            Value::Bytes(hash.borrow().xof_digest(&length)?)
        }
    } else {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("HASH.{method}() takes no keyword arguments"),
            ));
        }
        if !positional.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "HASH.{method}() takes no arguments ({} given)",
                    positional.len()
                ),
            ));
        }
        if hex {
            Value::Str(hash.borrow().hexdigest()?.to_string())
        } else {
            Value::Bytes(
                hash.borrow()
                    .fixed_digest(method, |data| Ok(data.to_vec()))?,
            )
        }
    };
    Ok(ctx.alloc(value))
}

/// Bind the positional-only file and factory before touching either object.
pub(super) fn file_digest(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let mut size = None;
    for (key, value) in kwargs {
        if key == "_bufsize" {
            size = Some(*value);
        } else {
            let positional: Vec<_> = ["fileobj", "digest"]
                .into_iter()
                .filter(|name| kwargs.iter().any(|(key, _)| key == name))
                .collect();
            let message = if positional.is_empty() {
                format!("file_digest() got an unexpected keyword argument '{key}'")
            } else {
                format!(
                    "file_digest() got some positional-only arguments passed as keyword arguments: '{}'",
                    positional.join(", ")
                )
            };
            return Err(Error::new("TypeError", message));
        }
    }
    let &[file, factory] = args else {
        let message = match args.len() {
            0 => "file_digest() missing 2 required positional arguments: 'fileobj' and 'digest'"
                .to_owned(),
            1 => "file_digest() missing 1 required positional argument: 'digest'".to_owned(),
            n if size.is_some() => format!(
                "file_digest() takes 2 positional arguments but {n} positional arguments (and 1 keyword-only argument) were given"
            ),
            n => format!("file_digest() takes 2 positional arguments but {n} were given"),
        };
        return Err(Error::new("TypeError", message));
    };
    let mark = ctx.heap.temporary_mark();
    let result = update_from_file(ctx, file, factory, size);
    ctx.heap.release_temporaries_since(mark);
    match &result {
        Ok(digest) => ctx.heap.keep(*digest),
        Err(error) => {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
        }
    }
    result
}

fn has_attr(ctx: &mut Context, obj: Id, attr: &str) -> Result<bool> {
    match ctx.lookup_attr(obj, attr) {
        Ok(_) => Ok(true),
        Err(error) if ctx.is_attribute_error(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn update_from_file(ctx: &mut Context, file: Id, factory: Id, size: Option<Id>) -> Result<Id> {
    let digest = if ctx.string_storage(factory).is_some() {
        let module = ctx.module("hashlib")?;
        let new = ctx.lookup_attr(module, "new")?;
        ctx.call(new, vec![factory], Vec::new())?
    } else {
        ctx.call(factory, Vec::new(), Vec::new())?
    };
    ctx.heap.keep(digest);
    if has_attr(ctx, file, "getbuffer")? {
        let update = ctx.lookup_attr(digest, "update")?;
        let getbuffer = ctx.lookup_attr(file, "getbuffer")?;
        let data = ctx.call(getbuffer, Vec::new(), Vec::new())?;
        ctx.call(update, vec![data], Vec::new())?;
        return Ok(digest);
    }
    let native_hash = matches!(ctx.heap.get(digest), Value::Hash(_));
    if let Some(&class) = ctx.builtins.get("_io.BytesIO")
        && ctx.heap.get(file).instance_class() == Some(class)
    {
        // A native digest consumes this snapshot synchronously. A Python update
        // callback can observe the exported view and must take the getbuffer path.
        if !native_hash {
            return Err(Error::new(
                "NotImplementedError",
                "BytesIO.getbuffer requires a writable memoryview",
            ));
        }
        let data = super::io::call(ctx, class, "getvalue", &[file], &[])?;
        method(ctx, "hash.update", &[digest, data])?;
        return Ok(digest);
    }
    let readable = if has_attr(ctx, file, "readinto")? && has_attr(ctx, file, "readable")? {
        let read = ctx.lookup_attr(file, "readable")?;
        let value = ctx.call(read, Vec::new(), Vec::new())?;
        ctx.truth(value)?
    } else {
        false
    };
    if !readable {
        let repr = ctx.builtin("repr");
        let value = ctx.call(repr, vec![file], Vec::new())?;
        let text = ctx.str_value(value)?;
        return Err(Error::new(
            "ValueError",
            format!("'{text}' is not a file-like object in binary reading mode."),
        ));
    }

    let size = size.unwrap_or_else(|| ctx.int(1 << 18));
    let bytearray = ctx.builtin("bytearray");
    let buf = ctx.call(bytearray, vec![size], Vec::new())?;
    ctx.heap.keep(buf);
    if native_hash {
        // Python sees the actual reusable bytearray. A native hash consumes
        // each prefix synchronously while resizing the exported buffer is blocked.
        return ctx.with_buffer_export(buf, |ctx| read_chunks(ctx, file, digest, buf, None));
    }
    let memoryview = ctx.builtin("memoryview");
    let view = ctx.call(memoryview, vec![buf], Vec::new())?;
    ctx.heap.keep(view);
    read_chunks(ctx, file, digest, buf, Some(view))
}

fn read_chunks(ctx: &mut Context, file: Id, digest: Id, buf: Id, view: Option<Id>) -> Result<Id> {
    let zero = ctx.int(0);
    let step = ctx.heap.temporary_mark();
    loop {
        let readinto = ctx.lookup_attr(file, "readinto")?;
        let count = ctx.call(readinto, vec![buf], Vec::new())?;
        if count == NONE {
            return Err(Error::new("BlockingIOError", "I/O operation would block."));
        }
        if ctx.checked_equal(count, zero)? {
            return Ok(digest);
        }
        if let Some(view) = view {
            let update = ctx.lookup_attr(digest, "update")?;
            let slice = ctx.alloc(Value::Slice([NONE, count, NONE]));
            let chunk = ctx.get_item(view, slice)?;
            ctx.call(update, vec![chunk], Vec::new())?;
        } else {
            let bounds = super::super::slice::sequence_components(ctx, [NONE, count, NONE])?;
            // __eq__ and __index__ above can run Python and change the contents.
            let bytes = buffer(ctx, buf)?;
            let [_, end, _] = super::super::slice::normalize(bounds, bytes.len() as i64);
            let Value::Hash(state) = ctx.heap.get(digest) else {
                unreachable!()
            };
            state.borrow_mut().update(&bytes[..end as usize]);
        }
        ctx.heap.release_temporaries_since(step);
        ctx.heap.reserve_slot_or_request_collection();
        ctx.collect_garbage();
    }
}
