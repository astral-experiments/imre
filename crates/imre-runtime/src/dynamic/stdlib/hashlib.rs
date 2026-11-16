use super::{Context, Error, Id, NONE, Result, Value, argument, builtins};
use crate::objects::HashState;
use digest::SharedHash;

mod blake2;
pub(crate) mod digest;
mod kdf;
pub(in crate::dynamic) mod md5;
pub(crate) mod openssl;

const UNSUPPORTED_DIGEST: &str = "_hashlib.UnsupportedDigestmodError";

pub(in crate::dynamic) fn install_openssl(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let provider = openssl::required()?;
    let class = if let Some(&class) = ctx.builtins.get(UNSUPPORTED_DIGEST) {
        class
    } else {
        let fields = ctx.scope("_hashlib", None);
        let module = ctx.text("_hashlib");
        ctx.bind(fields, "__module__", module);
        let name = ctx.text("UnsupportedDigestmodError");
        ctx.bind(fields, "__qualname__", name);
        ctx.bind(fields, "__doc__", NONE);
        let base = ctx.builtin("ValueError");
        let class = ctx.allocate_class("UnsupportedDigestmodError".into(), fields, vec![base])?;
        ctx.builtins.insert(UNSUPPORTED_DIGEST.into(), class);
        class
    };
    ctx.bind(scope, "UnsupportedDigestmodError", class);
    super::hmac::bind_compare(ctx, module, scope, "_hashlib.compare_digest");
    for name in ["HASH", "new", "get_fips_mode", "pbkdf2_hmac", "scrypt"] {
        if name != "scrypt" || provider.has_scrypt() {
            let function = ctx.builtin(&format!("_hashlib.{name}"));
            ctx.bind(scope, name, function);
        }
    }
    let names = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: true,
    });
    let mut constructors = Vec::new();
    for &name in digest::NAMES {
        let attr = format!("openssl_{name}");
        let constructor = ctx.builtin(&format!("_hashlib.{attr}"));
        ctx.bind(scope, &attr, constructor);
        let key = ctx.text(name);
        constructors.push((constructor, key));
        if provider.digest(name).is_ok() {
            builtins::set_add(ctx, names, key)?;
        }
    }
    let constructors = ctx.dict(constructors);
    let constructors = ctx.alloc(Value::MappingProxy(constructors));
    ctx.bind(scope, "_constructors", constructors);
    ctx.bind(scope, "openssl_md_meth_names", names);
    Ok(())
}

pub(super) fn call_openssl(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match name {
        "pbkdf2_hmac" => kdf::pbkdf2_hmac(ctx, args, kwargs),
        "scrypt" => kdf::scrypt(ctx, args, kwargs),
        "get_fips_mode" => {
            if !args.is_empty() || !kwargs.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    "get_fips_mode() takes no arguments",
                ));
            }
            let provider = openssl::required()?;
            Ok(ctx.int(i64::from(provider.fips_mode())))
        }
        _ => digest::construct(ctx, name, args, kwargs),
    }
    .map_err(|mut error| {
        if error.kind == UNSUPPORTED_DIGEST && error.class == NONE {
            error.class = ctx.builtins[UNSUPPORTED_DIGEST];
        }
        error
    })
}

/// Borrows the bytes of a boxed hash input, including mutable buffers.
pub(super) fn buffer(ctx: &Context, id: Id) -> Result<&[u8]> {
    match ctx.heap.get(ctx.scalar_value(id)) {
        Value::Bytes(_)
        | Value::ByteArray(_)
        | Value::MemoryView { .. }
        | Value::BytesIoBuffer { .. }
        | Value::Array(_) => ctx.native_buffer_bytes(id),
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
    fixed_hash(ctx, name, HashState::public_constructor(name), args, kwargs)
}

fn fixed_hash(
    ctx: &mut Context,
    name: &str,
    class: Option<&'static str>,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    // The six SHA-3 types share CPython's sha3_224 Argument Clinic entry point.
    let constructor = if class.is_some_and(|c| c.starts_with("_sha3.")) {
        "sha3_224"
    } else {
        name
    };
    let [data, flag, string] = builtins::bind_method_arguments(
        constructor,
        args,
        kwargs,
        ["data", "usedforsecurity", "string"],
        1,
    )?;
    let input = hash_input(ctx, data, flag, string)?;
    let state = HashState::provider(name)?.create(class, input);
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
    let class = HashState::public_constructor(name);
    let provider = HashState::provider(name);
    let data = hash_input(ctx, data, flag, string)?;
    let state = provider?.create(class, data);
    Ok(ctx.alloc(Value::Hash(SharedHash::new(state))))
}

pub(super) fn method(ctx: &mut Context, name: &str, args: &[Id]) -> Result<Id> {
    let Value::Hash(hash) = ctx.heap.get(argument(args, 0)?) else {
        unreachable!("bound hash method");
    };
    let value = match name {
        "hash.update" => {
            let data = buffer(ctx, argument(args, 1)?)?;
            hash.lock().update(data)?;
            return Ok(NONE);
        }
        "hash.copy" => Value::Hash(hash.lock().copy()?),
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
    let length = if hash.lock().is_xof() {
        let [length] = builtins::bind_builtin_arguments(method, positional, kwargs, ["length"], 1)?;
        Some(ctx.index_integer(length.expect("bound required length"))?)
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
        None
    };
    let value = hash.lock().digest(method, length.as_ref())?;
    Ok(ctx.alloc(value))
}

/// Hash successive prefixes of a reusable, exported readinto buffer.
pub(super) fn readinto(ctx: &mut Context, file: Id, digest: Id, size: Id) -> Result<Id> {
    let bytearray = ctx.builtin("bytearray");
    let buf = ctx.call(bytearray, vec![size], Vec::new())?;
    ctx.heap.keep(buf);
    if matches!(ctx.heap.get(ctx.scalar_value(digest)), Value::Hash(_)) {
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
            let Value::Hash(state) = ctx.heap.get(ctx.scalar_value(digest)) else {
                unreachable!()
            };
            state.lock().update(&bytes[..end as usize])?;
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
    if let Some(
        attr
        @ ("name" | "block_size" | "digest_size" | "_rate_bits" | "_capacity_bits" | "_suffix"),
    ) = method_name.strip_prefix("get_")
    {
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
    let (descriptor, rest) = ctx.getset_receiver("__get__", args)?;
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
    if let Value::LayoutDescriptor(owner, field) = *ctx.heap.get(descriptor) {
        return ctx.read_layout_descriptor(owner, field, receiver);
    }
    let Value::GetSetDescriptor {
        get, owner, name, ..
    } = *ctx.heap.get(descriptor)
    else {
        unreachable!("validated getset descriptor")
    };
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
