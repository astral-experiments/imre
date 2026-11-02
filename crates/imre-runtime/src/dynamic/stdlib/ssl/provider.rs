//! Blocking TLS clients using the optional system OpenSSL 3 library.
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::os::fd::{AsRawFd, OwnedFd};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

pub(in crate::dynamic) mod objects;

type Handle = *mut c_void;
type VerifyCallback = Option<unsafe extern "C" fn(c_int, Handle) -> c_int>;

// CPython 3.14.5 Modules/_ssl.c, PY_SSL_DEFAULT_CIPHER_STRING.
const CIPHERS: &CStr =
    c"@SECLEVEL=2:ECDH+AESGCM:ECDH+CHACHA20:ECDH+AES:DHE+AES:!aNULL:!eNULL:!aDSS:!SHA1:!AESCCM";
const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;
const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
const TLS1_2_VERSION: c_long = 0x0303;
const X509_V_FLAG_TRUSTED_FIRST: c_ulong = 0x8000;
const X509_V_FLAG_X509_STRICT: c_ulong = 0x20;
const X509_V_FLAG_PARTIAL_CHAIN: c_ulong = 0x80000;
const X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS: c_uint = 4;
pub(super) const HOSTFLAG_NEVER_CHECK_SUBJECT: c_uint = 0x20;
const SSL_ERROR_SSL: c_int = 1;
const SSL_ERROR_WANT_READ: c_int = 2;
const SSL_ERROR_WANT_WRITE: c_int = 3;
const SSL_ERROR_SYSCALL: c_int = 5;
const SSL_ERROR_ZERO_RETURN: c_int = 6;

macro_rules! functions {
    ($($name:ident($($arg:ty),*) -> $ret:ty;)+) => {
        #[allow(non_snake_case)]
        struct Api {
            has_psk: bool,
            $($name: unsafe extern "C" fn($($arg),*) -> $ret,)+
        }
        impl Api {
            fn symbols(handle: NonNull<c_void>) -> Option<Self> {
                Some(Self {
                    // OpenSSL omits this callback entry point in OPENSSL_NO_PSK builds.
                    // SAFETY: query the same live handle as the TLS entry points.
                    has_psk: !unsafe { libc::dlsym(handle.as_ptr(), c"SSL_CTX_set_psk_client_callback".as_ptr()) }.is_null(),
                    $($name: {
                        let name = concat!(stringify!($name), "\0");
                        // SAFETY: the live libssl handle includes its libcrypto dependency.
                        let addr = unsafe { libc::dlsym(handle.as_ptr(), name.as_ptr().cast()) };
                        // SAFETY: these are the OpenSSL 3 C signatures, and successful loads
                        // keep the library handle alive for the lifetime of the process.
                        unsafe { std::mem::transmute::<Handle, unsafe extern "C" fn($($arg),*) -> $ret>(
                            NonNull::new(addr)?.as_ptr()
                        ) }
                    },)+
                })
            }
        }
    };
}

functions! {
    OpenSSL_version(c_int) -> *const c_char;
    OpenSSL_version_num() -> c_ulong;
    OBJ_txt2obj(*const c_char, c_int) -> Handle;
    OBJ_nid2obj(c_int) -> Handle;
    OBJ_obj2nid(Handle) -> c_int;
    OBJ_nid2sn(c_int) -> *const c_char;
    OBJ_nid2ln(c_int) -> *const c_char;
    OBJ_obj2txt(*mut c_char, c_int, Handle, c_int) -> c_int;
    ASN1_OBJECT_free(Handle) -> ();
    TLS_client_method() -> Handle;
    SSL_CTX_new(Handle) -> Handle;
    SSL_CTX_free(Handle) -> ();
    SSL_CTX_ctrl(Handle, c_int, c_long, Handle) -> c_long;
    SSL_CTX_set_verify(Handle, c_int, VerifyCallback) -> ();
    SSL_CTX_set_cipher_list(Handle, *const c_char) -> c_int;
    SSL_CTX_get_security_level(Handle) -> c_int;
    SSL_CTX_set_default_verify_paths(Handle) -> c_int;
    SSL_CTX_get0_param(Handle) -> Handle;
    X509_VERIFY_PARAM_set_flags(Handle, c_ulong) -> c_int;
    X509_VERIFY_PARAM_get_flags(Handle) -> c_ulong;
    SSL_CTX_set_alpn_protos(Handle, *const u8, c_uint) -> c_int;
    SSL_CTX_set_post_handshake_auth(Handle, c_int) -> ();
    SSL_CTX_load_verify_locations(Handle, *const c_char, *const c_char) -> c_int;
    SSL_new(Handle) -> Handle;
    SSL_free(Handle) -> ();
    SSL_set_fd(Handle, c_int) -> c_int;
    X509_VERIFY_PARAM_get_hostflags(Handle) -> c_uint;
    X509_VERIFY_PARAM_set_hostflags(Handle, c_uint) -> ();
    SSL_session_reused(Handle) -> c_int;
    SSL_get_finished(Handle, Handle, usize) -> usize;
    SSL_get_peer_finished(Handle, Handle, usize) -> usize;
    SSL_get0_param(Handle) -> Handle;
    a2i_IPADDRESS(*const c_char) -> Handle;
    ASN1_STRING_length(Handle) -> c_int;
    ASN1_STRING_get0_data(Handle) -> *const u8;
    ASN1_OCTET_STRING_free(Handle) -> ();
    X509_VERIFY_PARAM_set1_ip(Handle, *const u8, usize) -> c_int;
    SSL_set1_host(Handle, *const c_char) -> c_int;
    SSL_ctrl(Handle, c_int, c_long, Handle) -> c_long;
    SSL_connect(Handle) -> c_int;
    SSL_get_verify_result(Handle) -> c_long;
    SSL_get0_peer_certificate(Handle) -> Handle;
    X509_verify_cert_error_string(c_long) -> *const c_char;
    SSL_read_ex(Handle, Handle, usize, *mut usize) -> c_int;
    SSL_write_ex(Handle, *const c_void, usize, *mut usize) -> c_int;
    SSL_get_error(Handle, c_int) -> c_int;
    ERR_clear_error() -> ();
    ERR_peek_error() -> c_ulong;
    ERR_peek_last_error() -> c_ulong;
    ERR_reason_error_string(c_ulong) -> *const c_char;
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let name = if cfg!(target_os = "linux") {
            c"libssl.so.3"
        } else if cfg!(target_os = "macos") {
            c"libssl.3.dylib"
        } else {
            return None;
        };
        // SAFETY: name is a NUL-terminated soname and the flags are valid.
        let handle = NonNull::new(unsafe {
            libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
        })?;
        let symbols = Api::symbols(handle);
        if symbols.is_none() {
            // SAFETY: failed lookup publishes no function pointers.
            unsafe { libc::dlclose(handle.as_ptr()) };
        }
        symbols
    })
    .as_ref()
}

pub(crate) fn available() -> bool {
    api().is_some()
}

pub(crate) fn version() -> Option<String> {
    let api = api()?;
    // SAFETY: OPENSSL_VERSION (0) returns a process-lifetime terminated version string.
    Some(
        unsafe { CStr::from_ptr((api.OpenSSL_version)(0)) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[derive(Debug)]
pub(crate) enum Failure {
    Verify { code: i64, message: String },
    Io(std::io::Error),
    Other(String),
}

impl Api {
    fn clear(&self) {
        // SAFETY: clears the calling thread's OpenSSL error queue.
        unsafe { (self.ERR_clear_error)() };
    }

    fn error(&self, operation: &str) -> Failure {
        // SAFETY: OpenSSL returns a static NUL-terminated reason, or null.
        let reason = unsafe { (self.ERR_reason_error_string)((self.ERR_peek_last_error)()) };
        if reason.is_null() {
            Failure::Other(operation.to_owned())
        } else {
            // SAFETY: reason is the non-null library string checked above.
            let detail = unsafe { CStr::from_ptr(reason) }.to_string_lossy();
            Failure::Other(format!("{operation}: {detail}"))
        }
    }
}

pub(crate) struct Context {
    api: &'static Api,
    handle: NonNull<c_void>,
}

impl Context {
    pub(crate) fn client() -> Result<Self, Failure> {
        let api = api().ok_or_else(|| Failure::Other("OpenSSL 3 TLS is unavailable".into()))?;
        api.clear();
        // SAFETY: TLS_client_method returns OpenSSL's process-lifetime client method.
        let handle = NonNull::new(unsafe { (api.SSL_CTX_new)((api.TLS_client_method)()) })
            .ok_or_else(|| api.error("SSL_CTX_new"))?;
        let mut ctx = Self { api, handle };
        // SAFETY: this fresh SSL_CTX is exclusively owned. Null selects OpenSSL's
        // standard verify callback, which aborts a handshake on failed validation.
        unsafe { (api.SSL_CTX_set_verify)(handle.as_ptr(), 0x01 | 0x02, None) };
        // SAFETY: arguments implement the SSL_CTX_set_min_proto_version macro.
        if unsafe {
            (api.SSL_CTX_ctrl)(
                handle.as_ptr(),
                SSL_CTRL_SET_MIN_PROTO_VERSION,
                TLS1_2_VERSION,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(api.error("SSL_CTX_set_min_proto_version"));
        }
        // SAFETY: SSL_new inherits these flags from the context's owned parameters.
        unsafe {
            (api.X509_VERIFY_PARAM_set_hostflags)(
                (api.SSL_CTX_get0_param)(handle.as_ptr()),
                X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS,
            )
        };
        ctx.add_verify_flags(X509_V_FLAG_TRUSTED_FIRST)?;
        ctx.set_ciphers(CIPHERS)?;
        Ok(ctx)
    }

    fn add_verify_flags(&mut self, flags: c_ulong) -> Result<(), Failure> {
        self.api.clear();
        // SAFETY: the parameters are borrowed from this live SSL_CTX; set_flags
        // adds verification requirements without clearing the other flags.
        if unsafe {
            (self.api.X509_VERIFY_PARAM_set_flags)(
                (self.api.SSL_CTX_get0_param)(self.handle.as_ptr()),
                flags,
            )
        } != 1
        {
            return Err(self.api.error("X509_VERIFY_PARAM_set_flags"));
        }
        Ok(())
    }

    pub(crate) fn apply_default_verify_flags(&mut self) -> Result<(), Failure> {
        self.add_verify_flags(X509_V_FLAG_PARTIAL_CHAIN | X509_V_FLAG_X509_STRICT)
    }

    pub(crate) fn verify_flags(&self) -> u64 {
        // SAFETY: the borrowed parameters remain valid throughout the query.
        unsafe {
            (self.api.X509_VERIFY_PARAM_get_flags)((self.api.SSL_CTX_get0_param)(
                self.handle.as_ptr(),
            )) as u64
        }
    }

    pub(crate) fn set_default_verify_paths(&mut self) -> Result<(), Failure> {
        self.api.clear();
        // SAFETY: OpenSSL populates its owned trust store from its configured
        // locations, including the provider's SSL_CERT_FILE and SSL_CERT_DIR.
        if unsafe { (self.api.SSL_CTX_set_default_verify_paths)(self.handle.as_ptr()) } != 1 {
            return Err(self.api.error("SSL_CTX_set_default_verify_paths"));
        }
        Ok(())
    }

    pub(crate) fn set_alpn_protocols(&mut self, protocols: &[u8]) -> Result<(), Failure> {
        let len = c_uint::try_from(protocols.len())
            .map_err(|_| Failure::Other("ALPN protocol list is too long".into()))?;
        self.api.clear();
        // SAFETY: OpenSSL validates and copies the length-prefixed protocol
        // list, and this API returns zero on success (including an empty list).
        if unsafe {
            (self.api.SSL_CTX_set_alpn_protos)(self.handle.as_ptr(), protocols.as_ptr(), len)
        } != 0
        {
            return Err(self.api.error("SSL_CTX_set_alpn_protos"));
        }
        Ok(())
    }

    pub(crate) fn set_post_handshake_auth(&mut self, enabled: bool) {
        // SAFETY: configure the PHA ClientHello extension on future SSL objects.
        unsafe {
            (self.api.SSL_CTX_set_post_handshake_auth)(self.handle.as_ptr(), i32::from(enabled))
        };
    }

    pub(crate) fn load_verify_locations(&mut self, cafile: &CStr) -> Result<(), Failure> {
        self.api.clear();
        // SAFETY: the context is live and the terminated path outlives the call.
        if unsafe {
            (self.api.SSL_CTX_load_verify_locations)(
                self.handle.as_ptr(),
                cafile.as_ptr(),
                ptr::null(),
            )
        } != 1
        {
            // OpenSSL 3 records a failed file open as the first error and may
            // append BIO and X509 errors. Decode its original errno from err.h.
            const SYSTEM_FLAG: c_ulong = 1 << 31;
            // SAFETY: the calling thread's queue was cleared before this operation.
            let first = unsafe { (self.api.ERR_peek_error)() };
            if first & SYSTEM_FLAG != 0 {
                let errno = (first & (SYSTEM_FLAG - 1)) as i32;
                return Err(Failure::Io(std::io::Error::from_raw_os_error(errno)));
            }
            return Err(self.api.error("SSL_CTX_load_verify_locations"));
        }
        Ok(())
    }

    pub(crate) fn set_ciphers(&mut self, ciphers: &CStr) -> Result<(), Failure> {
        self.api.clear();
        // SAFETY: the context is live and OpenSSL copies the supplied cipher list.
        if unsafe { (self.api.SSL_CTX_set_cipher_list)(self.handle.as_ptr(), ciphers.as_ptr()) }
            != 1
        {
            self.api.clear();
            return Err(Failure::Other("No cipher can be selected.".into()));
        }
        Ok(())
    }

    pub(crate) fn hostname_checks_common_name(&self) -> bool {
        // SAFETY: OpenSSL owns the verification parameters for this live SSL_CTX.
        let flags = unsafe {
            (self.api.X509_VERIFY_PARAM_get_hostflags)((self.api.SSL_CTX_get0_param)(
                self.handle.as_ptr(),
            ))
        };
        flags & HOSTFLAG_NEVER_CHECK_SUBJECT == 0
    }

    pub(crate) fn set_hostname_checks_common_name(&mut self, enabled: bool) {
        // SAFETY: update only the common-name bit on this live SSL_CTX.
        unsafe {
            let param = (self.api.SSL_CTX_get0_param)(self.handle.as_ptr());
            let flags = (self.api.X509_VERIFY_PARAM_get_hostflags)(param);
            (self.api.X509_VERIFY_PARAM_set_hostflags)(
                param,
                if enabled {
                    flags & !HOSTFLAG_NEVER_CHECK_SUBJECT
                } else {
                    flags | HOSTFLAG_NEVER_CHECK_SUBJECT
                },
            );
        }
    }

    pub(crate) fn security_level(&self) -> i32 {
        // SAFETY: self owns a live context throughout this query.
        unsafe { (self.api.SSL_CTX_get_security_level)(self.handle.as_ptr()) }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: release the reference acquired by SSL_CTX_new exactly once.
        unsafe { (self.api.SSL_CTX_free)(self.handle.as_ptr()) };
    }
}

pub(crate) struct Session {
    api: &'static Api,
    handle: NonNull<c_void>,
    socket: OwnedFd,
}

enum Operation<'a> {
    Connect,
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

impl Session {
    pub(crate) fn connect(ctx: &Context, fd: OwnedFd, host: &str) -> Result<Self, Failure> {
        let api = ctx.api;
        api.clear();
        // SAFETY: SSL_new takes its own reference to this live context.
        let handle = NonNull::new(unsafe { (api.SSL_new)(ctx.handle.as_ptr()) })
            .ok_or_else(|| api.error("SSL_new"))?;
        let session = Self {
            api,
            handle,
            socket: fd,
        };
        session.set_peer_name(host)?;
        // SAFETY: SSL_set_fd creates a non-closing socket BIO. The owned descriptor
        // remains valid for the full lifetime of this SSL object.
        if unsafe { (api.SSL_set_fd)(handle.as_ptr(), session.socket.as_raw_fd()) } != 1 {
            return Err(api.error("SSL_set_fd"));
        }
        let connected = session.blocking_io(Operation::Connect);
        // SAFETY: this live SSL contains the verification result of that handshake.
        let code = unsafe { (api.SSL_get_verify_result)(handle.as_ptr()) };
        if code != 0 {
            // SAFETY: OpenSSL documents this as a static string for every result code.
            let msg = unsafe { CStr::from_ptr((api.X509_verify_cert_error_string)(code)) };
            return Err(Failure::Verify {
                code: code as i64,
                message: msg.to_string_lossy().into_owned(),
            });
        }
        connected?;
        // A user-selected anonymous TLS 1.2 cipher can omit the certificate and
        // still yield X509_V_OK. This client requires an authenticated peer.
        // SAFETY: only test the borrowed pointer for presence while SSL is live.
        if unsafe { (api.SSL_get0_peer_certificate)(handle.as_ptr()) }.is_null() {
            return Err(Failure::Other("peer did not return a certificate".into()));
        }
        Ok(session)
    }

    fn set_peer_name(&self, host: &str) -> Result<(), Failure> {
        let api = self.api;
        let ssl = self.handle.as_ptr();
        if host.is_empty() || host.starts_with('.') || !host.is_ascii() {
            return Err(Failure::Other(
                "invalid or unsupported server hostname".into(),
            ));
        }
        let name =
            CString::new(host).map_err(|_| Failure::Other("embedded null character".into()))?;
        // SAFETY: CPython uses OpenSSL's own parser to select IP SAN matching
        // and suppress SNI for address literals. A successful parse is owned.
        let ip = unsafe { (api.a2i_IPADDRESS)(name.as_ptr()) };
        if !ip.is_null() {
            // SAFETY: the parsed ASN1_OCTET_STRING contains the address in network
            // byte order. OpenSSL copies its bytes before we release it.
            let rc = unsafe {
                let param = (api.SSL_get0_param)(ssl);
                let rc = (api.X509_VERIFY_PARAM_set1_ip)(
                    param,
                    (api.ASN1_STRING_get0_data)(ip),
                    (api.ASN1_STRING_length)(ip) as usize,
                );
                (api.ASN1_OCTET_STRING_free)(ip);
                rc
            };
            if rc != 1 {
                return Err(api.error("X509_VERIFY_PARAM_set1_ip"));
            }
        } else {
            api.clear();
            // Reject ambiguous numeric inputs instead of authenticating them as DNS.
            if host.bytes().all(|c| c.is_ascii_digit() || c == b'.')
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_'))
            {
                return Err(Failure::Other(
                    "invalid or unsupported server hostname".into(),
                ));
            }
            // SAFETY: OpenSSL copies the terminated name in both calls. Host flags
            // are inherited from the context. IP literals took the separate verification path above.
            unsafe {
                if (api.SSL_set1_host)(ssl, name.as_ptr()) != 1 {
                    return Err(api.error("SSL_set1_host"));
                }
                if (api.SSL_ctrl)(
                    ssl,
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    0,
                    name.as_ptr().cast_mut().cast(),
                ) != 1
                {
                    return Err(api.error("SSL_set_tlsext_host_name"));
                }
            }
        }
        Ok(())
    }

    /// Retry a blocking socket BIO operation with the same buffer and length.
    fn blocking_io(&self, mut operation: Operation<'_>) -> Result<usize, Failure> {
        let api = self.api;
        let ssl = self.handle.as_ptr();
        loop {
            api.clear();
            let mut n = 0;
            // SAFETY: only clear this thread's errno. All three operations borrow our
            // live SSL, and their buffers and output counter outlive each attempt.
            let (rc, label) = unsafe {
                #[cfg(target_os = "linux")]
                {
                    *libc::__errno_location() = 0;
                }
                #[cfg(target_os = "macos")]
                {
                    *libc::__error() = 0;
                }
                match &mut operation {
                    Operation::Connect => ((api.SSL_connect)(ssl), "SSL_connect"),
                    Operation::Read(buf) => (
                        (api.SSL_read_ex)(ssl, buf.as_mut_ptr().cast(), buf.len(), &mut n),
                        "SSL_read",
                    ),
                    Operation::Write(buf) => (
                        (api.SSL_write_ex)(ssl, buf.as_ptr().cast(), buf.len(), &mut n),
                        "SSL_write",
                    ),
                }
            };
            if rc == 1 {
                return Ok(n);
            }
            // Snapshot errno before any other OpenSSL call can change it.
            let os_error = std::io::Error::last_os_error();
            // SAFETY: classify this exact return value immediately on the same thread.
            let kind = unsafe { (api.SSL_get_error)(ssl, rc) };
            match kind {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => {
                    let events = if kind == SSL_ERROR_WANT_READ {
                        libc::POLLIN
                    } else {
                        libc::POLLOUT
                    };
                    crate::objects::socket::wait_blocking(self.socket.as_raw_fd(), events)
                        .map_err(Failure::Io)?;
                }
                SSL_ERROR_SYSCALL if os_error.kind() == std::io::ErrorKind::Interrupted => continue,
                SSL_ERROR_SYSCALL if os_error.raw_os_error() != Some(0) => {
                    return Err(Failure::Io(os_error));
                }
                _ => {
                    if matches!(operation, Operation::Read(_)) {
                        // OpenSSL 3 ERR_PACK(ERR_LIB_SSL, 0, SSL_R_UNEXPECTED_EOF_WHILE_READING).
                        const UNEXPECTED_EOF: c_ulong = (20 << 23) | 294;
                        // SAFETY: peek at the error from this operation before clearing the queue.
                        let detail = unsafe { (api.ERR_peek_last_error)() };
                        // Our public wrapper accepts only suppress_ragged_eofs=True.
                        if kind == SSL_ERROR_ZERO_RETURN
                            || (kind == SSL_ERROR_SSL && detail == UNEXPECTED_EOF)
                        {
                            return Ok(0);
                        }
                    }
                    return Err(api.error(label));
                }
            }
        }
    }

    pub(crate) fn channel_binding(&self) -> Option<Vec<u8>> {
        let ssl = self.handle.as_ptr();
        // SAFETY: this SSL is a connected client. Like CPython, select our Finished
        // for a full handshake and the peer's Finished for a resumed handshake.
        let finished = if unsafe { (self.api.SSL_session_reused)(ssl) } == 0 {
            self.api.SSL_get_finished
        } else {
            self.api.SSL_get_peer_finished
        };
        // SAFETY: a zero-length output requests the full size. The SSL is exclusively
        // borrowed across both calls, so no handshake can change the Finished message.
        let len = unsafe { finished(ssl, ptr::null_mut(), 0) };
        if len == 0 {
            return None;
        }
        let mut bytes = vec![0; len];
        // SAFETY: OpenSSL copies at most the supplied buffer length.
        unsafe { finished(ssl, bytes.as_mut_ptr().cast(), len) };
        Some(bytes)
    }

    pub(crate) fn sendall(&mut self, mut bytes: &[u8]) -> Result<(), Failure> {
        while !bytes.is_empty() {
            let n = self.blocking_io(Operation::Write(bytes))?;
            if n == 0 || n > bytes.len() {
                return Err(Failure::Other("invalid SSL_write byte count".into()));
            }
            bytes = &bytes[n..];
        }
        Ok(())
    }

    pub(crate) fn recv(&mut self, size: usize) -> Result<Vec<u8>, Failure> {
        // A TLS application-data read returns at most one plaintext record.
        let mut bytes = vec![0; size.min(16 * 1024)];
        if !bytes.is_empty() {
            let n = self.blocking_io(Operation::Read(&mut bytes))?;
            bytes.truncate(n);
        }
        Ok(bytes)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: SSL_free releases the sole SSL reference before socket is dropped.
        unsafe { (self.api.SSL_free)(self.handle.as_ptr()) };
    }
}
