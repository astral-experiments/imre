use super::{Context, Error, Id, NONE, Result, Value, argument, builtins};
use crate::objects::{HashState, SharedHash};

mod blake2;

/// Borrows the bytes of a boxed hash input, including mutable buffers.
pub(super) fn buffer(ctx: &Context, id: Id) -> Result<&[u8]> {
    match ctx.heap.get(ctx.scalar_value(id)) {
        Value::Bytes(data) | Value::ByteArray(data) => Ok(data),
        Value::Array(array) => Ok(array.as_bytes()),
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
    data_argument(data, string)?
        .map(|id| buffer(ctx, id))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn data_argument(data: Option<Id>, string: Option<Id>) -> Result<Option<Id>> {
    if data.is_some() && string.is_some() {
        return Err(Error::new(
            "TypeError",
            "'data' and 'string' are mutually exclusive and support for 'string' keyword \
             parameter is slated for removal in a future version.",
        ));
    }
    Ok(data.or(string))
}

pub(super) fn hash(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if matches!(name, "blake2b" | "blake2s") {
        return blake2::new(ctx, name, args, kwargs);
    }
    let [data, flag, string] = builtins::bind_method_arguments(
        name,
        args,
        kwargs,
        ["data", "usedforsecurity", "string"],
        1,
    )?;
    let input = hash_input(ctx, data, flag, string)?;
    let state = (HashState::provider(name)?.create)(input);
    Ok(ctx.alloc(Value::Hash(SharedHash::new(state))))
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
    let [name, data, flag, string] = super::builtins::bind_method_arguments(
        "new",
        args,
        kwargs,
        ["name", "data", "usedforsecurity", "string"],
        2,
    )?;
    let name =
        name.ok_or_else(|| Error::new("TypeError", "new() missing required argument 'name'"))?;
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
    Ok(ctx.alloc(Value::Hash(SharedHash::new(state))))
}

pub(super) fn method(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let Value::Hash(hash) = ctx.heap.get(argument(args, 0)?) else {
        unreachable!("bound hash method");
    };
    let value = match name {
        "hash.update" => {
            let data = buffer(ctx, argument(args, 1)?)?;
            hash.lock().update(data);
            return Ok(NONE);
        }
        "hash.copy" => Value::Hash(SharedHash::new(hash.lock().clone())),
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
    let hash = hash.clone();
    let positional = &args[1..];
    let hex = method == "hexdigest";
    let value = if hash.lock().is_xof() {
        let [length] = builtins::bind_builtin_arguments(method, positional, kwargs, ["length"], 1)?;
        let length = ctx.index_integer(length.expect("bound required length"))?;
        if hex {
            Value::Str(hash.lock().xof_hexdigest(&length)?.to_string())
        } else {
            Value::Bytes(hash.lock().xof_digest(&length)?)
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
            Value::Str(hash.lock().hexdigest()?.to_string())
        } else {
            Value::Bytes(
                hash.lock()
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
    if let Some(class) = ctx.heap.get(file).instance_class()
        && ctx.heap.get(class).io_kind() == Some("BytesIO")
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
            state.lock().update(&bytes[..end as usize]);
        }
        ctx.heap.release_temporaries_since(step);
        ctx.heap.reserve_slot_or_request_collection();
        ctx.collect_garbage();
    }
}

/// Unbound descriptors validate the exact recipient before operating on its storage.
pub(super) fn provider_method(
    ctx: &mut Context,
    class: &str,
    method_name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let short = class.rsplit_once('.').unwrap().1;
    if method_name == "__new__" {
        let Some((&receiver, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("{class}.__new__(): not enough arguments"),
            ));
        };
        let ty = ctx.builtin(class);
        if receiver != ty {
            if ctx.builtin_type_name(receiver) != Some("type") {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{class}.__new__(X): X is not a type object ({})",
                        ctx.type_name(receiver)
                    ),
                ));
            }
            let found = ctx.class_name(receiver);
            return Err(Error::new(
                "TypeError",
                format!("{class}.__new__({found}): {found} is not a subtype of {class}"),
            ));
        }
        return ctx.call(ty, rest.to_vec(), kwargs.to_vec());
    }
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method {short}.{method_name}() needs an argument"),
        ));
    };
    if ctx.type_name(receiver) != class {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{method_name}' for '{class}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    }
    if let Some(attr @ ("name" | "block_size" | "digest_size")) = method_name.strip_prefix("get_") {
        return ctx.lookup_attr(receiver, attr);
    }
    match method_name {
        "copy" | "update" | "digest" | "hexdigest" => {
            let xof = matches!(ctx.heap.get(receiver), Value::Hash(h) if h.lock().is_xof());
            if xof && matches!(method_name, "digest" | "hexdigest") {
                return digest(ctx, method_name, args, kwargs);
            }
            if !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!("{short}.{method_name}() takes no keyword arguments"),
                ));
            }
            if method_name == "update" && rest.len() != 1 {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{short}.update() takes exactly one argument ({} given)",
                        rest.len()
                    ),
                ));
            }
            if method_name != "update" && !rest.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{short}.{method_name}() takes no arguments ({} given)",
                        rest.len()
                    ),
                ));
            }
            match method_name {
                "copy" => method(ctx, "hash.copy", args),
                "update" => method(ctx, "hash.update", args),
                _ => digest(ctx, method_name, args, kwargs),
            }
        }
        _ => Err(Error::new("AttributeError", method_name)),
    }
}

pub(super) fn descriptor_get(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let (&descriptor, rest) = args
        .split_first()
        .ok_or_else(|| Error::new("TypeError", "expected a getset descriptor"))?;
    let Value::GetSetDescriptor {
        get, owner, name, ..
    } = *ctx.heap.get(descriptor)
    else {
        return Err(Error::new("TypeError", "expected a getset descriptor"));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "wrapper __get__() takes no keyword arguments",
        ));
    }
    if rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            "__get__ expected at least 1 argument, got 0",
        ));
    }
    if rest.len() > 2 {
        return Err(Error::new(
            "TypeError",
            format!("__get__ expected at most 2 arguments, got {}", rest.len()),
        ));
    }
    let receiver = rest[0];
    if receiver == NONE {
        if rest.get(1).copied().unwrap_or(NONE) == NONE {
            return Err(Error::new("TypeError", "__get__(None, None) is invalid"));
        }
        return Ok(descriptor);
    }
    if HashState::is_provider_class(owner) && ctx.type_name(receiver) != owner {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{name}' for '{owner}' objects doesn't apply to a '{}' object",
                ctx.type_name(receiver)
            ),
        ));
    }
    ctx.call(get, vec![receiver], Vec::new())
}
