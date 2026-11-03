//! CPython's _ssl version attributes and OpenSSL ASN.1 object database lookups.
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::{self, NonNull};

use super::Api;
use crate::dynamic::{
    Context, builtins, stdlib,
    value::{Error, Id, NONE, NativeModuleState, Result, SSL_ERROR_TYPE, Value},
};

#[repr(C)]
struct OptionEntry {
    name: *const c_char,
    value: u64,
}

unsafe extern "C" {
    fn imre_ssl_options(version: std::ffi::c_ulong, count: *mut usize) -> *const OptionEntry;
}

pub(in crate::dynamic) fn ssl_error(ctx: &Context, module: Id, message: &str) -> Error {
    let Value::Module {
        module_state: Some(NativeModuleState::SslError(class)),
        ..
    } = *ctx.heap.get(module)
    else {
        unreachable!("a MemoryBIO type retains its _ssl module")
    };
    let mut error = Error::new("SSLError", message);
    error.class = class;
    error
}

fn install_ssl_error(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("ssl", None);
    for (name, value) in [
        ("__module__", "ssl"),
        ("__doc__", "An error occurred in the SSL implementation."),
    ] {
        let value = ctx.text(value);
        ctx.bind(fields, name, value);
    }
    let base = ctx.builtin("OSError");
    let class = ctx.allocate_class("SSLError".into(), fields, vec![base])?;
    let Value::Class { heap_type, .. } = ctx.heap.get_mut(class) else {
        unreachable!()
    };
    *heap_type = Some(SSL_ERROR_TYPE);
    let Value::Scope(frame) = ctx.heap.get_mut(fields) else {
        unreachable!()
    };
    let namespace = frame.class_namespace.as_mut().unwrap();
    namespace.immutable_type = true;
    namespace.defining_module = Some(module);
    let slot = ctx.alloc(Value::HeapTypeMethod {
        owner: class,
        name: "__str__",
    });
    ctx.bind(fields, "__str__", slot);
    let Value::Module { module_state, .. } = ctx.heap.get_mut(module) else {
        unreachable!()
    };
    *module_state = Some(NativeModuleState::SslError(class));
    ctx.bind(scope, "SSLError", class);
    Ok(())
}

pub(in crate::dynamic) fn error_str(
    ctx: &mut Context,
    owner: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            "descriptor '__str__' of 'ssl.SSLError' object needs an argument",
        ));
    };
    if !ctx.instance_of(receiver, owner) {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '__str__' requires a 'ssl.SSLError' object but received a '{}'",
                ctx.type_name(receiver),
            ),
        ));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "wrapper __str__() takes no keyword arguments",
        ));
    }
    if !rest.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("expected 0 arguments, got {}", rest.len()),
        ));
    }
    if let Some(message) = ctx.os_attribute(receiver, "strerror")?
        && ctx.string_storage(message).is_some()
    {
        return Ok(message);
    }
    let Value::Raised(error) = ctx.heap.get(receiver) else {
        return Err(Error::new("TypeError", "expected an SSLError instance"));
    };
    let args = ctx.exception_argument_tuple(receiver, error.clone());
    let text = ctx.checked_format(args, false)?;
    Ok(ctx.text(text))
}

fn api() -> Result<&'static Api> {
    super::api().ok_or_else(|| Error::new("ImportError", "system TLS provider unavailable"))
}

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let api = api()?;
    // Extension functions retain the module that first installed them.
    if matches!(
        ctx.heap.get(module),
        Value::Module {
            module_state: None,
            ..
        }
    ) {
        for name in ["txt2obj", "nid2obj", "get_default_verify_paths"] {
            let callable = ctx.builtin(&format!("_ssl.{name}"));
            let function = ctx.alloc(Value::Method {
                callable,
                receiver: module,
                explicit: false,
            });
            ctx.bind(scope, name, function);
        }
    }
    install_ssl_error(ctx, module, scope)?;
    super::bio::install(ctx, module, scope)?;
    // SAFETY: both functions read constants from the process-lifetime loaded provider.
    let (version, runtime_version) = unsafe {
        (
            CStr::from_ptr((api.OpenSSL_version)(0)),
            (api.OpenSSL_version_num)(),
        )
    };
    let version = ctx.text(version.to_string_lossy().into_owned());
    ctx.bind(scope, "OPENSSL_VERSION", version);
    let number = runtime_version as i64;
    let number_value = ctx.int(number);
    ctx.bind(scope, "OPENSSL_VERSION_NUMBER", number_value);
    // CPython keeps the historical major/minor/fix/patch/status bit layout for OpenSSL 3.
    let parts = [
        (number >> 28) & 255,
        (number >> 20) & 255,
        (number >> 12) & 255,
        (number >> 4) & 255,
        number & 15,
    ]
    .map(|n| ctx.int(n));
    let info = ctx.alloc(Value::Tuple(parts.into()));
    ctx.bind(scope, "OPENSSL_VERSION_INFO", info);
    // CPython protocol method selectors, distinct from TLS wire versions.
    for (name, code) in [
        ("PROTOCOL_SSLv23", 2),
        ("PROTOCOL_TLS", 2),
        ("PROTOCOL_TLSv1", 3),
        ("PROTOCOL_TLSv1_1", 4),
        ("PROTOCOL_TLSv1_2", 5),
        ("PROTOCOL_TLS_CLIENT", 16),
        ("PROTOCOL_TLS_SERVER", 17),
    ] {
        let value = ctx.int(code);
        ctx.bind(scope, name, value);
    }

    // These are library build capabilities, independent of runtime cipher policy.
    for (name, available) in [
        ("HAS_SNI", true),
        ("HAS_TLS_UNIQUE", true),
        ("HAS_ECDH", true),
        ("HAS_NPN", false),
        ("HAS_ALPN", true),
        ("HAS_PSK", api.has_psk),
        ("HAS_PHA", true),
    ] {
        ctx.bind(scope, name, builtins::boolean(available));
    }
    let flag = ctx.int(i64::from(super::HOSTFLAG_NEVER_CHECK_SUBJECT));
    ctx.bind(scope, "HOSTFLAG_NEVER_CHECK_SUBJECT", flag);
    let mut count = 0;
    // SAFETY: the header shim writes count and returns a static array of that many
    // initialized entries, including a nonempty fallback on a version mismatch.
    let options = unsafe {
        let table = imre_ssl_options(runtime_version, &mut count);
        std::slice::from_raw_parts(table, count)
    };
    for option in options {
        // SAFETY: each name is an ASCII string literal owned by the static table.
        let name = unsafe { CStr::from_ptr(option.name) }.to_str().unwrap();
        let value = ctx.alloc(Value::Int(crate::Int::new(option.value.into())));
        ctx.bind(scope, name, value);
    }
    super::versions::install(ctx, scope, runtime_version);
    Ok(())
}

// OBJ_txt2obj may allocate, while OBJ_nid2obj may return a static entry.
// ASN1_OBJECT_free handles both kinds of result.
struct Object(&'static Api, NonNull<c_void>);

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: release exactly this OpenSSL lookup result.
        unsafe { (self.0.ASN1_OBJECT_free)(self.1.as_ptr()) };
    }
}

impl Object {
    fn as_tuple(&self, ctx: &mut Context) -> Result<Id> {
        let api = self.0;
        let obj = self.1.as_ptr();
        // SAFETY: the ASN1_OBJECT belongs to this wrapper.
        let nid = unsafe { (api.OBJ_obj2nid)(obj) };
        if nid == 0 {
            return Err(Error::new("ValueError", "Unknown object"));
        }
        // SAFETY: OpenSSL owns the NID name strings. Copy any non-null names before returning.
        let (short, long) = unsafe { ((api.OBJ_nid2sn)(nid), (api.OBJ_nid2ln)(nid)) };
        let mut name = |s: *const c_char| {
            if s.is_null() {
                NONE
            } else {
                // SAFETY: OBJ_nid2sn/ln returned a static, terminated library name.
                ctx.text(unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned())
            }
        };
        let short = name(short);
        let long = name(long);
        // SAFETY: a null output buffer requests the exact length of the numeric OID.
        let len = unsafe { (api.OBJ_obj2txt)(ptr::null_mut(), 0, obj, 1) };
        let oid = match len {
            0 => NONE, // Some registered NIDs have names but no numeric OID.
            n if n > 0 => {
                let size = n
                    .checked_add(1)
                    .ok_or_else(|| Error::new("OverflowError", "OID too long"))?;
                let mut buf = vec![0; size as usize];
                // SAFETY: buf holds the queried length plus its terminating NUL.
                if unsafe { (api.OBJ_obj2txt)(buf.as_mut_ptr().cast(), size, obj, 1) } != len {
                    return Err(Error::new("SSLError", "could not format ASN.1 object"));
                }
                buf.truncate(len as usize);
                ctx.text(String::from_utf8(buf).expect("numeric OID is ASCII"))
            }
            _ => return Err(Error::new("SSLError", "could not format ASN.1 object")),
        };
        let nid = ctx.int(i64::from(nid));
        Ok(ctx.alloc(Value::Tuple(vec![nid, short, long, oid])))
    }
}

// PyUnicode_FromFormat("%.100s") replaces invalid UTF-8, but omits a partial
// final code point when the byte precision is reached.
fn unknown_text(text: &CStr) -> Error {
    let bytes = text.to_bytes();
    let mut prefix = &bytes[..bytes.len().min(100)];
    if prefix.len() == 100
        && let Some(start) = prefix.iter().rposition(|b| b & 0xc0 != 0x80)
        && std::str::from_utf8(&prefix[start..]).is_err_and(|e| e.error_len().is_none())
    {
        prefix = &prefix[..start];
    }
    Error::new(
        "ValueError",
        format!("unknown object '{}'", String::from_utf8_lossy(prefix)),
    )
}

pub(in crate::dynamic) fn function_metadata(symbol: &str, attribute: &str) -> Option<&'static str> {
    Some(match (symbol, attribute) {
        ("_ssl.get_default_verify_paths", "__text_signature__") => "($module, /)",
        ("_ssl.nid2obj", "__text_signature__") => "($module, nid, /)",
        ("_ssl.txt2obj", "__text_signature__") => "($module, /, txt, name=False)",
        ("_ssl.get_default_verify_paths", "__doc__") => concat!(
            "Return search paths and environment vars that are used by ",
            "SSLContext's set_default_verify_paths() to load default CAs.\n\n",
            "The values are 'cert_file_env', 'cert_file', 'cert_dir_env', 'cert_dir'."
        ),
        ("_ssl.nid2obj", "__doc__") => {
            "Lookup NID, short name, long name and OID of an ASN1_OBJECT by NID."
        }
        ("_ssl.txt2obj", "__doc__") => concat!(
            "Lookup NID, short name, long name and OID of an ASN1_OBJECT.\n\n",
            "By default objects are looked up by OID. With name=True short and\n",
            "long name are also matched."
        ),
        _ => return None,
    })
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&module, args)) = args.split_first() else {
        return Err(Error::new("TypeError", "expected an _ssl module receiver"));
    };
    if !matches!(
        ctx.heap.get(module),
        Value::Module {
            module_state: Some(NativeModuleState::SslError(_)),
            ..
        }
    ) {
        return Err(Error::new("TypeError", "expected an _ssl module receiver"));
    }
    let api = api()?;
    if name == "get_default_verify_paths" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "_ssl.get_default_verify_paths() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "_ssl.get_default_verify_paths() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let parts = [
            api.X509_get_default_cert_file_env,
            api.X509_get_default_cert_file,
            api.X509_get_default_cert_dir_env,
            api.X509_get_default_cert_dir,
        ]
        .map(|get| {
            // SAFETY: each getter returns a borrowed, terminated provider string or NULL.
            let path = unsafe { get() };
            if path.is_null() {
                NONE
            } else {
                // SAFETY: the non-null provider string lives for the lifetime of the module.
                let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
                ctx.text_codepoints(crate::objects::interpreter_stdlib::fsdecode(bytes).collect())
            }
        });
        return Ok(ctx.alloc(Value::Tuple(parts.into())));
    }
    let obj = match name {
        "txt2obj" => {
            let [text, named] =
                builtins::bind_builtin_arguments("txt2obj", args, kwargs, ["txt", "name"], 1)?;
            let text = text.expect("required txt");
            if ctx.string_storage(text).is_none() {
                let ty = if text == NONE {
                    "None"
                } else {
                    ctx.type_name(text)
                };
                let end = ty.floor_char_boundary(ty.len().min(50));
                return Err(Error::new(
                    "TypeError",
                    format!("txt2obj() argument 'txt' must be str, not {}", &ty[..end]),
                ));
            }
            let encoded = stdlib::codecs::encode_text(ctx, text, None, None)?;
            let txt = CString::new(encoded)
                .map_err(|_| Error::new("ValueError", "embedded null character"))?;
            let named = named
                .map(|name| ctx.truth(name))
                .transpose()?
                .unwrap_or(false);
            // SAFETY: OpenSSL reads a terminated buffer for this lookup.
            NonNull::new(unsafe { (api.OBJ_txt2obj)(txt.as_ptr(), i32::from(!named)) })
                .ok_or_else(|| unknown_text(&txt))?
        }
        "nid2obj" => {
            let arg = builtins::one_positional_arg("_ssl.nid2obj", args, kwargs)?;
            let nid = stdlib::index_c_int(ctx, arg)?;
            if nid < 0 {
                return Err(Error::new("ValueError", "NID must be positive."));
            }
            // SAFETY: OpenSSL validates the integer against its object database.
            NonNull::new(unsafe { (api.OBJ_nid2obj)(nid) })
                .ok_or_else(|| Error::new("ValueError", format!("unknown NID {nid}")))?
        }
        _ => return Err(Error::new("AttributeError", name.to_owned())),
    };
    Object(api, obj).as_tuple(ctx)
}
