//! CPython's _ssl version attributes and OpenSSL ASN.1 object database lookups.
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::{self, NonNull};

use super::Api;
use crate::dynamic::{
    Context, builtins, stdlib,
    value::{Error, Id, NONE, Result, Value},
};

fn api() -> Result<&'static Api> {
    super::api().ok_or_else(|| Error::new("ImportError", "system TLS provider unavailable"))
}

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
    let api = api()?;
    // SAFETY: both functions read constants from the process-lifetime loaded provider.
    let (version, number) = unsafe {
        (
            CStr::from_ptr((api.OpenSSL_version)(0)),
            (api.OpenSSL_version_num)() as i64,
        )
    };
    let version = ctx.text(version.to_string_lossy().into_owned());
    ctx.bind(scope, "OPENSSL_VERSION", version);
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
    for name in ["txt2obj", "nid2obj"] {
        let function = ctx.builtin(&format!("_ssl.{name}"));
        ctx.bind(scope, name, function);
    }
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

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let api = api()?;
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
