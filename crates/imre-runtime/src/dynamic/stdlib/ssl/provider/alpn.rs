//! OpenSSL's ALPN offer, server selection and negotiated connection result.
use std::cell::RefCell;
use std::ffi::{c_int, c_uint, c_void};
use std::ptr::{self, NonNull};
use std::rc::Rc;

use super::{Api, Failure, Handle};

pub(super) type SelectCallback =
    unsafe extern "C" fn(Handle, *mut *const u8, *mut u8, *const u8, c_uint, Handle) -> c_int;

/// Shared by the context and each SSL that can invoke its selection callback.
pub(super) struct Alpn {
    api: &'static Api,
    protocols: RefCell<Vec<u8>>,
}

impl Alpn {
    pub(super) fn new(api: &'static Api) -> Self {
        Self {
            api,
            protocols: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn configure(
        self: &Rc<Self>,
        ctx: NonNull<c_void>,
        protocols: &[u8],
    ) -> Result<(), Failure> {
        let len = c_uint::try_from(protocols.len())
            .map_err(|_| Failure::Other("ALPN protocol list is too long".into()))?;
        self.api.clear();
        // SAFETY: OpenSSL validates and copies the length-prefixed client offer.
        if unsafe { (self.api.SSL_CTX_set_alpn_protos)(ctx.as_ptr(), protocols.as_ptr(), len) } != 0
        {
            return Err(self.api.error("SSL_CTX_set_alpn_protos"));
        }
        self.protocols.replace(protocols.to_vec());
        // SAFETY: the callback argument lives in an Rc allocation retained by the
        // context and both kinds of connection, including across context reconfiguration.
        unsafe {
            (self.api.SSL_CTX_set_alpn_select_cb)(
                ctx.as_ptr(),
                select,
                Rc::as_ptr(self).cast_mut().cast(),
            );
        }
        Ok(())
    }

    pub(super) fn selected(&self, ssl: NonNull<c_void>) -> Option<Vec<u8>> {
        let mut data = ptr::null();
        let mut len = 0;
        // SAFETY: OpenSSL borrows the selection from the live SSL. Copy it before
        // the connection can be advanced, unwrapped or freed.
        unsafe {
            (self.api.SSL_get0_alpn_selected)(ssl.as_ptr(), &mut data, &mut len);
            (!data.is_null()).then(|| std::slice::from_raw_parts(data, len as usize).to_vec())
        }
    }
}

unsafe extern "C" fn select(
    _ssl: Handle,
    out: *mut *const u8,
    out_len: *mut u8,
    client: *const u8,
    client_len: c_uint,
    arg: Handle,
) -> c_int {
    const OPENSSL_NPN_NEGOTIATED: c_int = 1;
    const SSL_TLSEXT_ERR_OK: c_int = 0;
    const SSL_TLSEXT_ERR_NOACK: c_int = 3;
    // SAFETY: configure installed this argument; the connection retains its owner.
    let alpn = unsafe { &*arg.cast::<Alpn>() };
    let server = alpn.protocols.borrow();
    if server.is_empty() {
        return SSL_TLSEXT_ERR_NOACK;
    }
    let mut selected = ptr::null_mut();
    // SAFETY: both wire lists remain valid through selection. OpenSSL supplies
    // the client list and writable outputs. The server list was validated above.
    let result = unsafe {
        (alpn.api.SSL_select_next_proto)(
            &mut selected,
            out_len,
            server.as_ptr(),
            server.len() as c_uint,
            client,
            client_len,
        )
    };
    if result == OPENSSL_NPN_NEGOTIATED {
        // SAFETY: OpenSSL consumes this protocol before returning to the caller.
        unsafe { *out = selected };
        SSL_TLSEXT_ERR_OK
    } else {
        SSL_TLSEXT_ERR_NOACK
    }
}
