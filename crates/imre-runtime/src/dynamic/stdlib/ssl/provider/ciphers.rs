//! Snapshot a context's enabled ciphers in OpenSSL's preference order.
use std::ffi::{CStr, c_char};
use std::ptr::NonNull;

use super::super::{context, provider_error};
use super::{Api, Handle};
use crate::dynamic::{
    Context, builtins,
    value::{Id, NONE, Result, Value},
};

pub(in crate::dynamic) fn get(ctx: &mut Context, handle: Id) -> Result<Id> {
    let tls = context(ctx, handle)?.borrow();
    let api = tls.api;
    api.clear();
    // SAFETY: SSL_new takes a reference to the live context and inherits its cipher list.
    let ssl = unsafe { (api.SSL_new)(tls.handle.as_ptr()) };
    drop(tls);
    let ssl = NonNull::new(ssl).ok_or_else(|| provider_error(ctx, api.error("SSL_new")))?;
    let result = snapshot(ctx, api, ssl.as_ptr());
    // SAFETY: all strings are copied into the VM before releasing this sole SSL reference.
    unsafe { (api.SSL_free)(ssl.as_ptr()) };
    result
}

fn snapshot(ctx: &mut Context, api: &Api, ssl: Handle) -> Result<Id> {
    // SAFETY: the temporary SSL owns this stack and remains live through the traversal.
    let (stack, count) = unsafe {
        let stack = (api.SSL_get_ciphers)(ssl);
        (stack, (api.OPENSSL_sk_num)(stack))
    };
    if count < 0 {
        return Err(provider_error(ctx, api.error("SSL_get_ciphers")));
    }
    let mut list = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: index is within the borrowed stack.
        let cipher = unsafe { (api.OPENSSL_sk_value)(stack, index) };
        if cipher.is_null() {
            return Err(provider_error(ctx, api.error("missing SSL cipher")));
        }
        list.push(cipher_dict(ctx, api, cipher));
    }
    Ok(ctx.alloc(Value::List(list)))
}

impl super::OwnedSsl {
    pub(in crate::dynamic::stdlib::ssl) fn negotiated_cipher(&self, ctx: &mut Context) -> Id {
        // SAFETY: the SSL owns the borrowed cipher and OpenSSL owns its name strings.
        unsafe {
            let cipher = (self.api.SSL_get_current_cipher)(self.handle.as_ptr());
            if cipher.is_null() {
                return NONE;
            }
            let name = optional_text(ctx, (self.api.SSL_CIPHER_get_name)(cipher));
            let version = optional_text(ctx, (self.api.SSL_CIPHER_get_version)(cipher));
            let bits = (self.api.SSL_CIPHER_get_bits)(cipher, std::ptr::null_mut());
            let bits = ctx.int(i64::from(bits));
            ctx.alloc(Value::Tuple(vec![name, version, bits]))
        }
    }
}

unsafe fn optional_text(ctx: &mut Context, text: *const c_char) -> Id {
    if text.is_null() {
        return NONE;
    }
    // SAFETY: callers supply terminated names owned by the live OpenSSL provider.
    ctx.text(unsafe { CStr::from_ptr(text) }.to_string_lossy())
}

fn cipher_dict(ctx: &mut Context, api: &Api, cipher: Handle) -> Id {
    let mut description = [0u8; 512];
    let mut alg_bits = 0;
    // SAFETY: cipher is a borrowed, non-null entry in the live SSL's cipher stack.
    // description has room for the requested length plus one trailing NUL.
    let fields = unsafe {
        (api.SSL_CIPHER_description)(cipher, description.as_mut_ptr().cast(), 511);
        let description = CStr::from_bytes_until_nul(&description)
            .unwrap()
            .to_string_lossy();
        let description = description.strip_suffix('\n').unwrap_or(&description);
        let strength_bits = (api.SSL_CIPHER_get_bits)(cipher, &mut alg_bits);
        [
            ("id", ctx.int(i64::from((api.SSL_CIPHER_get_id)(cipher)))),
            (
                "name",
                optional_text(ctx, (api.SSL_CIPHER_get_name)(cipher)),
            ),
            (
                "protocol",
                optional_text(ctx, (api.SSL_CIPHER_get_version)(cipher)),
            ),
            ("description", ctx.text(description)),
            ("strength_bits", ctx.int(i64::from(strength_bits))),
            ("alg_bits", ctx.int(i64::from(alg_bits))),
            (
                "aead",
                builtins::boolean((api.SSL_CIPHER_is_aead)(cipher) != 0),
            ),
        ]
    };
    let mut entries: Vec<_> = fields
        .into_iter()
        .map(|(key, value)| (ctx.text(key), value))
        .collect();
    for (key, get_nid) in [
        ("symmetric", api.SSL_CIPHER_get_cipher_nid),
        ("digest", api.SSL_CIPHER_get_digest_nid),
        ("kea", api.SSL_CIPHER_get_kx_nid),
        ("auth", api.SSL_CIPHER_get_auth_nid),
    ] {
        // SAFETY: cipher belongs to the live SSL stack; OpenSSL owns the NID names.
        let name = unsafe {
            match get_nid(cipher) {
                0 => NONE, // OpenSSL's NID_undef
                nid => optional_text(ctx, (api.OBJ_nid2ln)(nid)),
            }
        };
        entries.push((ctx.text(key), name));
    }
    ctx.dict(entries)
}
