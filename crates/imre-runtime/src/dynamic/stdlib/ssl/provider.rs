//! Blocking TLS connections using the optional system OpenSSL 3 library.
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

use crate::objects::socket;

pub(in crate::dynamic) mod bio;
pub(super) mod certificate;
pub(super) mod ciphers;
pub(in crate::dynamic) mod objects;
mod reasons;
mod trust;
pub(super) use trust::CaData;
pub(in crate::dynamic) mod versions;

type Handle = *mut c_void;
type PasswordCallback = unsafe extern "C" fn(*mut c_char, c_int, c_int, Handle) -> c_int;
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
// OpenSSL 3 ERR_PACK(ERR_LIB_SSL, 0, SSL_R_UNEXPECTED_EOF_WHILE_READING).
const UNEXPECTED_EOF: c_ulong = (20 << 23) | 294;

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
    OBJ_sn2nid(*const c_char) -> c_int;
    OBJ_nid2obj(c_int) -> Handle;
    OBJ_obj2nid(Handle) -> c_int;
    OBJ_nid2sn(c_int) -> *const c_char;
    OBJ_nid2ln(c_int) -> *const c_char;
    OBJ_obj2txt(*mut c_char, c_int, Handle, c_int) -> c_int;
    ASN1_OBJECT_free(Handle) -> ();
    BIO_s_mem() -> Handle;
    BIO_new(Handle) -> Handle;
    BIO_new_mem_buf(*const c_void, c_int) -> Handle;
    BIO_free(Handle) -> c_int;
    BIO_up_ref(Handle) -> c_int;
    BIO_ctrl_pending(Handle) -> usize;
    BIO_ctrl(Handle, c_int, c_long, Handle) -> c_long;
    BIO_set_flags(Handle, c_int) -> ();
    BIO_clear_flags(Handle, c_int) -> ();
    BIO_read(Handle, *mut c_void, c_int) -> c_int;
    BIO_write(Handle, *const c_void, c_int) -> c_int;
    TLS_method() -> Handle;
    TLS_client_method() -> Handle;
    TLS_server_method() -> Handle;
    SSL_CTX_set_default_passwd_cb(Handle, PasswordCallback) -> ();
    SSL_set_default_passwd_cb(Handle, PasswordCallback) -> ();
    SSL_set_default_passwd_cb_userdata(Handle, Handle) -> ();
    SSL_use_certificate_chain_file(Handle, *const c_char) -> c_int;
    SSL_use_PrivateKey_file(Handle, *const c_char, c_int) -> c_int;
    SSL_check_private_key(Handle) -> c_int;
    SSL_get_certificate(Handle) -> Handle;
    SSL_get_privatekey(Handle) -> Handle;
    SSL_CTX_use_cert_and_key(Handle, Handle, Handle, Handle, c_int) -> c_int;
    SSL_CTX_new(Handle) -> Handle;
    SSL_CTX_free(Handle) -> ();
    SSL_CTX_ctrl(Handle, c_int, c_long, Handle) -> c_long;
    SSL_CTX_get_options(Handle) -> u64;
    SSL_CTX_set_options(Handle, u64) -> u64;
    SSL_CTX_clear_options(Handle, u64) -> u64;
    SSL_CTX_get_verify_mode(Handle) -> c_int;
    SSL_CTX_set_verify(Handle, c_int, VerifyCallback) -> ();
    SSL_CTX_set_cipher_list(Handle, *const c_char) -> c_int;
    SSL_CTX_get_num_tickets(Handle) -> usize;
    SSL_CTX_set_num_tickets(Handle, usize) -> c_int;
    SSL_CTX_get_security_level(Handle) -> c_int;
    SSL_CTX_set_default_verify_paths(Handle) -> c_int;
    X509_get_default_cert_file_env() -> *const c_char;
    X509_get_default_cert_file() -> *const c_char;
    X509_get_default_cert_dir_env() -> *const c_char;
    X509_get_default_cert_dir() -> *const c_char;
    SSL_CTX_get_cert_store(Handle) -> Handle;
    d2i_X509_bio(Handle, *mut Handle) -> Handle;
    PEM_read_bio_X509(Handle, *mut Handle, PasswordCallback, Handle) -> Handle;
    X509_STORE_add_cert(Handle, Handle) -> c_int;
    X509_STORE_lock(Handle) -> c_int;
    X509_STORE_unlock(Handle) -> c_int;
    X509_STORE_get0_objects(Handle) -> Handle;
    X509_OBJECT_get_type(Handle) -> c_int;
    X509_OBJECT_get0_X509(Handle) -> Handle;
    X509_up_ref(Handle) -> c_int;
    X509_free(Handle) -> ();
    X509_check_ca(Handle) -> c_int;
    SSL_CTX_get0_param(Handle) -> Handle;
    X509_VERIFY_PARAM_set_flags(Handle, c_ulong) -> c_int;
    X509_VERIFY_PARAM_clear_flags(Handle, c_ulong) -> c_int;
    X509_VERIFY_PARAM_get_flags(Handle) -> c_ulong;
    SSL_CTX_set_alpn_protos(Handle, *const u8, c_uint) -> c_int;
    SSL_CTX_set_post_handshake_auth(Handle, c_int) -> ();
    SSL_CTX_load_verify_locations(Handle, *const c_char, *const c_char) -> c_int;
    SSL_new(Handle) -> Handle;
    SSL_free(Handle) -> ();
    SSL_set_fd(Handle, c_int) -> c_int;
    SSL_set0_rbio(Handle, Handle) -> ();
    SSL_set0_wbio(Handle, Handle) -> ();
    SSL_set_accept_state(Handle) -> ();
    SSL_set_connect_state(Handle) -> ();
    SSL_do_handshake(Handle) -> c_int;
    SSL_is_init_finished(Handle) -> c_int;
    SSL_get_SSL_CTX(Handle) -> Handle;
    SSL_shutdown(Handle) -> c_int;
    SSL_set_read_ahead(Handle, c_int) -> ();
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
    SSL_accept(Handle) -> c_int;
    SSL_connect(Handle) -> c_int;
    SSL_get_verify_mode(Handle) -> c_int;
    SSL_get_verify_result(Handle) -> c_long;
    SSL_get0_peer_certificate(Handle) -> Handle;
    X509_get_ext_d2i(Handle, c_int, *mut c_int, *mut c_int) -> Handle;
    GENERAL_NAMES_free(Handle) -> ();
    GENERAL_NAME_get0_value(Handle, *mut c_int) -> Handle;
    GENERAL_NAME_print(Handle, Handle) -> c_int;
    AUTHORITY_INFO_ACCESS_free(Handle) -> ();
    CRL_DIST_POINTS_free(Handle) -> ();
    SSL_get0_verified_chain(Handle) -> Handle;
    SSL_get_peer_cert_chain(Handle) -> Handle;
    OPENSSL_sk_num(Handle) -> c_int;
    OPENSSL_sk_value(Handle, c_int) -> Handle;
    i2d_X509(Handle, *mut *mut u8) -> c_int;
    X509_get_subject_name(Handle) -> Handle;
    X509_get_issuer_name(Handle) -> Handle;
    X509_get_version(Handle) -> c_long;
    X509_get_serialNumber(Handle) -> Handle;
    X509_get0_notBefore(Handle) -> Handle;
    X509_get0_notAfter(Handle) -> Handle;
    X509_NAME_entry_count(Handle) -> c_int;
    X509_NAME_get_entry(Handle, c_int) -> Handle;
    X509_NAME_ENTRY_set(Handle) -> c_int;
    X509_NAME_ENTRY_get_object(Handle) -> Handle;
    X509_NAME_ENTRY_get_data(Handle) -> Handle;
    ASN1_STRING_type(Handle) -> c_int;
    ASN1_STRING_to_UTF8(*mut *mut u8, Handle) -> c_int;
    CRYPTO_free(Handle, *const c_char, c_int) -> ();
    i2a_ASN1_INTEGER(Handle, Handle) -> c_int;
    ASN1_TIME_print(Handle, Handle) -> c_int;
    BIO_gets(Handle, *mut c_char, c_int) -> c_int;
    X509_verify_cert_error_string(c_long) -> *const c_char;
    SSL_get_shutdown(Handle) -> c_int;
    SSL_pending(Handle) -> c_int;
    SSL_read_ex(Handle, Handle, usize, *mut usize) -> c_int;
    SSL_write_ex(Handle, *const c_void, usize, *mut usize) -> c_int;
    SSL_get_ciphers(Handle) -> Handle;
    SSL_CIPHER_get_id(Handle) -> u32;
    SSL_CIPHER_get_name(Handle) -> *const c_char;
    SSL_CIPHER_get_version(Handle) -> *const c_char;
    SSL_CIPHER_description(Handle, *mut c_char, c_int) -> *mut c_char;
    SSL_CIPHER_get_bits(Handle, *mut c_int) -> c_int;
    SSL_CIPHER_is_aead(Handle) -> c_int;
    SSL_CIPHER_get_cipher_nid(Handle) -> c_int;
    SSL_CIPHER_get_digest_nid(Handle) -> c_int;
    SSL_CIPHER_get_kx_nid(Handle) -> c_int;
    SSL_CIPHER_get_auth_nid(Handle) -> c_int;
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
    Verify {
        code: i64,
        message: String,
    },
    Trust {
        code: i64,
        library: Option<&'static str>,
        reason: Option<String>,
        message: String,
    },
    Io(std::io::Error),
    Other(String),
}

impl Api {
    fn clear(&self) {
        // SAFETY: clears the calling thread's OpenSSL error queue.
        unsafe { (self.ERR_clear_error)() };
    }

    fn set_peer_name(&self, ssl: Handle, host: &str, verify_hostname: bool) -> Result<(), Failure> {
        if host.is_empty() || host.starts_with('.') || !host.is_ascii() {
            return Err(Failure::Other(
                "invalid or unsupported server hostname".into(),
            ));
        }
        let name =
            CString::new(host).map_err(|_| Failure::Other("embedded null character".into()))?;
        // SAFETY: CPython uses OpenSSL's own parser to select IP SAN matching
        // and suppress SNI for address literals. A successful parse is owned.
        let ip = unsafe { (self.a2i_IPADDRESS)(name.as_ptr()) };
        if !ip.is_null() {
            // SAFETY: the parsed ASN1_OCTET_STRING contains the address in network
            // byte order. OpenSSL copies its bytes before we release it.
            let accepted = unsafe {
                let accepted = !verify_hostname
                    || (self.X509_VERIFY_PARAM_set1_ip)(
                        (self.SSL_get0_param)(ssl),
                        (self.ASN1_STRING_get0_data)(ip),
                        (self.ASN1_STRING_length)(ip) as usize,
                    ) == 1;
                (self.ASN1_OCTET_STRING_free)(ip);
                accepted
            };
            if !accepted {
                return Err(self.error("X509_VERIFY_PARAM_set1_ip"));
            }
        } else {
            self.clear();
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
                if verify_hostname && (self.SSL_set1_host)(ssl, name.as_ptr()) != 1 {
                    return Err(self.error("SSL_set1_host"));
                }
                if (self.SSL_ctrl)(
                    ssl,
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    0,
                    name.as_ptr().cast_mut().cast(),
                ) != 1
                {
                    return Err(self.error("SSL_set_tlsext_host_name"));
                }
            }
        }
        Ok(())
    }

    fn encode_certificate(&self, certificate: Handle) -> Result<Vec<u8>, Failure> {
        if certificate.is_null() {
            return Err(Failure::Other("missing certificate in TLS chain".into()));
        }
        // SAFETY: the X509 remains live through both calls. A null output queries its DER size.
        let size = unsafe { (self.i2d_X509)(certificate, ptr::null_mut()) };
        if size <= 0 {
            return Err(self.error("i2d_X509"));
        }
        let mut der = vec![0u8; size as usize];
        let mut cursor = der.as_mut_ptr();
        // SAFETY: the unchanged certificate has exactly size encoded bytes. i2d_X509
        // advances cursor, while der retains ownership of the original allocation.
        if unsafe { (self.i2d_X509)(certificate, &mut cursor) } != size {
            return Err(self.error("i2d_X509"));
        }
        Ok(der)
    }

    fn verify_peer(&self, ssl: Handle, client_handshake_complete: bool) -> Result<(), Failure> {
        // SAFETY: read the mode copied into this SSL by SSL_new, even if the context
        // was reconfigured between retries on a memory BIO.
        if unsafe { (self.SSL_get_verify_mode)(ssl) } & SSL_VERIFY_PEER == 0 {
            return Ok(());
        }
        // SAFETY: this live SSL contains the verification result of that handshake.
        let code = unsafe { (self.SSL_get_verify_result)(ssl) };
        if code != 0 {
            // SAFETY: OpenSSL documents this as a static string for every result code.
            let msg = unsafe { CStr::from_ptr((self.X509_verify_cert_error_string)(code)) };
            #[allow(clippy::useless_conversion, reason = "OpenSSL returns a C long")]
            let code = i64::from(code);
            return Err(Failure::Verify {
                code,
                message: msg.to_string_lossy().into_owned(),
            });
        }
        // A user-selected anonymous TLS 1.2 cipher can omit the certificate and
        // still yield X509_V_OK. This client requires an authenticated peer.
        // SAFETY: only test the borrowed pointer for presence while SSL is live.
        if client_handshake_complete && unsafe { (self.SSL_get0_peer_certificate)(ssl) }.is_null() {
            return Err(Failure::Other("peer did not return a certificate".into()));
        }
        Ok(())
    }

    fn file_error(&self) -> Failure {
        // OpenSSL 3 records a failed file open as the first error and may
        // append BIO and X509 errors. Decode its original errno from err.h.
        const SYSTEM_FLAG: c_ulong = 1 << 31;
        // SAFETY: the calling thread's queue was cleared before this operation.
        let first = unsafe { (self.ERR_peek_error)() };
        if first & SYSTEM_FLAG != 0 {
            let errno = (first & (SYSTEM_FLAG - 1)) as i32;
            return Failure::Io(std::io::Error::from_raw_os_error(errno));
        }
        // SAFETY: keep the last provider reason for Python's SSLError.
        self.trust_error(unsafe { (self.ERR_peek_last_error)() }, None)
    }

    fn reason_text(&self, code: c_ulong) -> Option<std::borrow::Cow<'_, str>> {
        // SAFETY: the loaded library owns the optional static reason string.
        let reason = unsafe { (self.ERR_reason_error_string)(code) };
        NonNull::new(reason.cast_mut()).map(|p| {
            // SAFETY: a non-null result points to the provider's static, NUL-terminated string.
            unsafe { CStr::from_ptr(p.as_ptr()) }.to_string_lossy()
        })
    }

    fn error(&self, operation: &str) -> Failure {
        // SAFETY: inspect the calling thread's error queue.
        let code = unsafe { (self.ERR_peek_last_error)() };
        let message = self.reason_text(code).map_or_else(
            || operation.to_owned(),
            |detail| format!("{operation}: {detail}"),
        );
        Failure::Other(message)
    }
}

fn clear_errno() {
    // SAFETY: only clear this thread's errno for the following OpenSSL operation.
    unsafe {
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = 0;
        }
        #[cfg(target_os = "macos")]
        {
            *libc::__error() = 0;
        }
    }
}

// A password was required for this PEM. Never use OpenSSL's interactive UI.
unsafe extern "C" fn reject_password(_: *mut c_char, _: c_int, _: c_int, _: Handle) -> c_int {
    -1
}

// Method selectors exposed by CPython, distinct from TLS wire versions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Protocol {
    Tls = 2,
    Client = 16,
    Server = 17,
}

const SSL_VERIFY_PEER: c_int = 1;
const SSL_VERIFY_FAIL_IF_NO_PEER_CERT: c_int = 2;
const VERIFY_REQUIRED: c_int = SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT;

pub(crate) struct Context {
    api: &'static Api,
    handle: NonNull<c_void>,
    pub(super) protocol: Protocol,
    pub(super) check_hostname: bool,
}

/// References retained under the OpenSSL store lock, in provider order.
struct StoreSnapshot {
    api: &'static Api,
    certificates: Vec<Handle>,
    crls: c_int,
}

impl StoreSnapshot {
    fn authorities(&self) -> impl Iterator<Item = Handle> + '_ {
        self.certificates.iter().copied().filter(|&cert| {
            // SAFETY: this snapshot owns an X509 reference for every entry.
            unsafe { (self.api.X509_check_ca)(cert) != 0 }
        })
    }
}

impl Drop for StoreSnapshot {
    fn drop(&mut self) {
        for &cert in &self.certificates {
            // SAFETY: release our reference, including on partial snapshot or decode failure.
            unsafe { (self.api.X509_free)(cert) };
        }
    }
}

impl Context {
    pub(super) fn new(protocol: Protocol) -> Result<Self, Failure> {
        let api = api().ok_or_else(|| Failure::Other("OpenSSL 3 TLS is unavailable".into()))?;
        api.clear();
        let method = match protocol {
            Protocol::Tls => api.TLS_method,
            Protocol::Client => api.TLS_client_method,
            Protocol::Server => api.TLS_server_method,
        };
        // SAFETY: each selector returns a process-lifetime SSL_METHOD from this same provider.
        let handle = NonNull::new(unsafe { (api.SSL_CTX_new)(method()) })
            .ok_or_else(|| api.error("SSL_CTX_new"))?;
        let mut ctx = Self {
            api,
            handle,
            protocol,
            check_hostname: protocol == Protocol::Client,
        };
        ctx.set_verify_bits(if ctx.check_hostname {
            VERIFY_REQUIRED
        } else {
            0
        });
        // SAFETY: the context owns the callback, which has static lifetime.
        unsafe {
            (api.SSL_CTX_set_default_passwd_cb)(handle.as_ptr(), reject_password);
        }
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
        // CPython's adjusted OP_ALL and the options set on every new client context.
        // OpenSSL 3 assigns zero to OP_NO_SSLv2 and both SINGLE_*_USE flags.
        const WORKAROUNDS: u64 = (1 << 31) | (1 << 6) | (1 << 4);
        const NO_COMPRESSION: u64 = 1 << 17;
        const CIPHER_SERVER_PREFERENCE: u64 = 1 << 22;
        let defaults =
            WORKAROUNDS | versions::SSL_OP_NO_SSLV3 | NO_COMPRESSION | CIPHER_SERVER_PREFERENCE;
        // SAFETY: options are set on a live, exclusively owned SSL_CTX.
        unsafe { (api.SSL_CTX_set_options)(handle.as_ptr(), defaults) };

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

    fn verify_bits(&self) -> c_int {
        // SAFETY: SSL_CTX_get_verify_mode reads this live context.
        unsafe { (self.api.SSL_CTX_get_verify_mode)(self.handle.as_ptr()) }
    }

    fn set_verify_bits(&mut self, bits: c_int) {
        // SAFETY: OpenSSL copies the mode. A null callback retains default verification.
        unsafe { (self.api.SSL_CTX_set_verify)(self.handle.as_ptr(), bits, None) };
    }

    pub(super) fn verify_mode(&self) -> Result<i64, Failure> {
        match self.verify_bits() & VERIFY_REQUIRED {
            0 => Ok(0),
            SSL_VERIFY_PEER => Ok(1),
            VERIFY_REQUIRED => Ok(2),
            _ => Err(Failure::Other(
                "invalid return value from SSL_CTX_get_verify_mode".into(),
            )),
        }
    }

    pub(super) fn set_verify_mode(&mut self, mode: c_int) -> Result<(), &'static str> {
        let bits = match mode {
            0 if self.check_hostname => {
                return Err("Cannot set verify_mode to CERT_NONE when check_hostname is enabled.");
            }
            0 => 0,
            1 => SSL_VERIFY_PEER,
            2 => VERIFY_REQUIRED,
            _ => return Err("invalid value for verify_mode"),
        };
        self.set_verify_bits(bits);
        Ok(())
    }

    pub(super) fn set_check_hostname(&mut self, enabled: bool) {
        if enabled && self.verify_bits() == 0 {
            self.set_verify_bits(VERIFY_REQUIRED);
        }
        self.check_hostname = enabled;
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

    pub(super) fn options(&self) -> u64 {
        // SAFETY: SSL_CTX_get_options reads the live context.
        unsafe { (self.api.SSL_CTX_get_options)(self.handle.as_ptr()) }
    }

    pub(super) fn change_options(&mut self, clear: u64, set: u64) {
        // SAFETY: both masks apply to the same live SSL_CTX. OpenSSL preserves unrelated bits.
        unsafe {
            if clear != 0 {
                (self.api.SSL_CTX_clear_options)(self.handle.as_ptr(), clear);
            }
            if set != 0 {
                (self.api.SSL_CTX_set_options)(self.handle.as_ptr(), set);
            }
        }
    }

    pub(crate) fn verify_flags(&self) -> c_ulong {
        // SAFETY: the borrowed parameters remain valid throughout the query.
        unsafe {
            (self.api.X509_VERIFY_PARAM_get_flags)((self.api.SSL_CTX_get0_param)(
                self.handle.as_ptr(),
            ))
        }
    }

    pub(super) fn set_verify_flags(&mut self, flags: c_ulong) -> Result<(), Failure> {
        let old = self.verify_flags();
        let clear = old & !flags;
        if clear != 0 {
            self.api.clear();
            // SAFETY: the verification parameters are borrowed from this live SSL_CTX.
            if unsafe {
                (self.api.X509_VERIFY_PARAM_clear_flags)(
                    (self.api.SSL_CTX_get0_param)(self.handle.as_ptr()),
                    clear,
                )
            } != 1
            {
                return Err(self.api.error("X509_VERIFY_PARAM_clear_flags"));
            }
        }
        let set = flags & !old;
        if set != 0 {
            self.add_verify_flags(set)?;
        }
        Ok(())
    }

    pub(super) fn session_stats(&self) -> [(&'static str, i64); 11] {
        // SSL_CTX_sess_* are macros over SSL_CTX_ctrl. CPython exposes these
        // SSL_CTRL_SESS_* counters in this order, omitting CB_HIT (28).
        [
            ("number", 20),
            ("connect", 21),
            ("connect_good", 22),
            ("connect_renegotiate", 23),
            ("accept", 24),
            ("accept_good", 25),
            ("accept_renegotiate", 26),
            ("hits", 27),
            ("misses", 29),
            ("timeouts", 30),
            ("cache_full", 31),
        ]
        .map(|(name, command)| {
            // SAFETY: these read-only commands take no pointer or integer argument.
            let count = unsafe {
                (self.api.SSL_CTX_ctrl)(self.handle.as_ptr(), command, 0, ptr::null_mut())
            };
            (name, count as i64)
        })
    }

    pub(super) fn cert_store_stats(&self) -> Option<[c_int; 3]> {
        let snapshot = self.store_snapshot()?;
        Some([
            snapshot.certificates.len() as c_int,
            snapshot.crls,
            snapshot.authorities().count() as c_int,
        ])
    }

    fn store_snapshot(&self) -> Option<StoreSnapshot> {
        let api = self.api;
        let mut snapshot = StoreSnapshot {
            api,
            certificates: Vec::new(),
            crls: 0,
        };
        // SAFETY: the context owns this store. Retain each X509 while holding the store
        // lock so we can classify it after unlocking. This works on OpenSSL 3.0–3.2
        // as well, which do not export X509_STORE_get1_objects.
        unsafe {
            let store = (api.SSL_CTX_get_cert_store)(self.handle.as_ptr());
            if store.is_null() || (api.X509_STORE_lock)(store) != 1 {
                return None;
            }
            let captured = (|| {
                let objects = (api.X509_STORE_get0_objects)(store);
                let count = (api.OPENSSL_sk_num)(objects);
                snapshot
                    .certificates
                    .try_reserve_exact(usize::try_from(count).ok()?)
                    .ok()?;
                for index in 0..count {
                    let object = NonNull::new((api.OPENSSL_sk_value)(objects, index))?;
                    match (api.X509_OBJECT_get_type)(object.as_ptr()) {
                        1 => {
                            // X509_LU_X509
                            let cert = (api.X509_OBJECT_get0_X509)(object.as_ptr());
                            if cert.is_null() || (api.X509_up_ref)(cert) != 1 {
                                return None;
                            }
                            snapshot.certificates.push(cert);
                        }
                        2 => snapshot.crls += 1, // X509_LU_CRL
                        _ => (),
                    }
                }
                Some(())
            })();
            if (api.X509_STORE_unlock)(store) != 1 {
                return None;
            }
            captured.map(|()| snapshot)
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

    /// Release the Python-visible context borrow before invoking any password callback.
    pub(super) fn credentials(&self) -> Result<Credentials, Failure> {
        self.api.clear();
        // SAFETY: the new SSL holds its own reference to this context.
        let handle = NonNull::new(unsafe { (self.api.SSL_new)(self.handle.as_ptr()) })
            .ok_or_else(|| self.api.error("SSL_new"))?;
        Ok(Credentials {
            api: self.api,
            handle,
        })
    }
}

/// A private SSL stages the key, certificate and chain until all three can be installed.
pub(super) struct Credentials {
    api: &'static Api,
    handle: NonNull<c_void>,
}

impl Drop for Credentials {
    fn drop(&mut self) {
        // SAFETY: release the SSL_new reference and all staged credentials.
        unsafe { (self.api.SSL_free)(self.handle.as_ptr()) };
    }
}

struct Password<'a> {
    fill: &'a mut dyn FnMut(&mut [u8]) -> Option<usize>,
    panic: Option<Box<dyn std::any::Any + Send>>,
}

unsafe extern "C" fn copy_password(
    buf: *mut c_char,
    size: c_int,
    _: c_int,
    userdata: Handle,
) -> c_int {
    // SAFETY: OpenSSL provides a writable buffer. Initialize it before turning it
    // into a Rust slice. The userdata is scoped to this private SSL's synchronous reads.
    let (password, out) = unsafe {
        let buf = buf.cast::<u8>();
        buf.write_bytes(0, size as usize);
        (
            &mut *userdata.cast::<Password<'_>>(),
            std::slice::from_raw_parts_mut(buf, size as usize),
        )
    };
    if password.panic.is_some() {
        return -1;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (password.fill)(out))) {
        Ok(Some(n)) if n <= out.len() => n as c_int,
        Ok(_) => -1,
        Err(payload) => {
            password.panic = Some(payload);
            -1
        }
    }
}

impl Credentials {
    pub(super) fn load_cert_chain(
        self,
        certfile: &CStr,
        keyfile: &CStr,
        fill: &mut dyn FnMut(&mut [u8]) -> Option<usize>,
    ) -> Result<(), Failure> {
        let mut password = Password { fill, panic: None };
        // SAFETY: only the staged SSL can invoke this callback. Caller reentry uses
        // a different SSL and cannot replace or invalidate this stack-local userdata.
        unsafe {
            (self.api.SSL_set_default_passwd_cb)(self.handle.as_ptr(), copy_password);
            (self.api.SSL_set_default_passwd_cb_userdata)(
                self.handle.as_ptr(),
                (&raw mut password).cast(),
            );
        }
        let api = self.api;
        let result = self.install_credentials(certfile, keyfile);
        drop(self);
        // Capture any provider error before clearing the thread's queue, even
        // when a Python callback supplies the exception that will be raised.
        api.clear();
        if let Some(payload) = password.panic {
            std::panic::resume_unwind(payload);
        }
        result
    }

    fn install_credentials(&self, certfile: &CStr, keyfile: &CStr) -> Result<(), Failure> {
        let api = self.api;
        let ssl = self.handle.as_ptr();
        // SAFETY: both paths are terminated; the temporary SSL lives throughout the call.
        if unsafe { (api.SSL_use_certificate_chain_file)(ssl, certfile.as_ptr()) } != 1 {
            return Err(api.file_error());
        }
        // SAFETY: record the selected certificate, without taking ownership. Loading a key
        // of another algorithm can silently switch to an older credential slot.
        let loaded = unsafe { (api.SSL_get_certificate)(ssl) };
        api.clear();
        const SSL_FILETYPE_PEM: c_int = 1;
        // SAFETY: OpenSSL retains the decoded key in the temporary SSL.
        if unsafe { (api.SSL_use_PrivateKey_file)(ssl, keyfile.as_ptr(), SSL_FILETYPE_PEM) } != 1 {
            return Err(api.file_error());
        }
        // SAFETY: borrow the live selection after loading the key. The earlier pointer is
        // used only for identity comparison. The certificate passed on below comes from here.
        let selected = unsafe { (api.SSL_get_certificate)(ssl) };
        if selected.is_null() || selected != loaded {
            return Err(Failure::Other(
                "private key type does not match certificate".into(),
            ));
        }
        // SAFETY: check exactly the pair we just loaded.
        if unsafe { (api.SSL_check_private_key)(ssl) } != 1 {
            return Err(api.error("SSL_check_private_key"));
        }
        let mut chain: Handle = ptr::null_mut();
        const SSL_CTRL_GET_CHAIN_CERTS: c_int = 115;
        // SAFETY: the SSL_get0_chain_certs macro returns a borrowed chain, possibly null.
        if unsafe { (api.SSL_ctrl)(ssl, SSL_CTRL_GET_CHAIN_CERTS, 0, (&raw mut chain).cast()) } != 1
        {
            return Err(api.error("SSL_get0_chain_certs"));
        }
        // SAFETY: install the matched key, certificate and chain together. The context retains
        // references independently of the temporary SSL that owns the borrowed arguments.
        if unsafe {
            (api.SSL_CTX_use_cert_and_key)(
                (api.SSL_get_SSL_CTX)(ssl),
                selected,
                (api.SSL_get_privatekey)(ssl),
                chain,
                1,
            )
        } != 1
        {
            return Err(api.error("SSL_CTX_use_cert_and_key"));
        }
        Ok(())
    }
}

impl Context {
    /// Set one TLS group by its OpenSSL short name. Return false for an unknown name.
    pub(super) fn set_ecdh_curve(&mut self, name: &CStr) -> Result<bool, Failure> {
        self.api.clear();
        // SAFETY: the name is NUL-terminated and borrowed only during lookup.
        let mut nid = unsafe { (self.api.OBJ_sn2nid)(name.as_ptr()) };
        if nid == 0 {
            return Ok(false);
        }
        // SSL_CTX_set1_groups copies the supplied group list to the context.
        const SSL_CTRL_SET_GROUPS: c_int = 91;
        // SAFETY: the live context and one-element NID array outlive SSL_CTX_ctrl.
        let ok = unsafe {
            (self.api.SSL_CTX_ctrl)(
                self.handle.as_ptr(),
                SSL_CTRL_SET_GROUPS,
                1,
                (&raw mut nid).cast(),
            )
        };
        if ok == 0 {
            // SAFETY: this thread's last OpenSSL error belongs to the failed call.
            let code = unsafe { (self.api.ERR_peek_last_error)() };
            let error = self.api.trust_error(code, None);
            self.api.clear();
            return Err(error);
        }
        Ok(true)
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

    pub(super) fn num_tickets(&self) -> usize {
        // SAFETY: the context owns the SSL_CTX handle throughout this query.
        unsafe { (self.api.SSL_CTX_get_num_tickets)(self.handle.as_ptr()) }
    }

    pub(super) fn set_num_tickets(&mut self, count: usize) -> Result<(), &'static str> {
        if self.protocol != Protocol::Server {
            return Err("SSLContext is not a server context.");
        }
        // SAFETY: the context owns the SSL_CTX handle and OpenSSL copies the count.
        if unsafe { (self.api.SSL_CTX_set_num_tickets)(self.handle.as_ptr(), count) } != 1 {
            return Err("failed to set num tickets.");
        }
        Ok(())
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
    socket: Option<crate::ObjectOwner>,
    server: bool,
}

enum Operation<'a> {
    Accept,
    Connect,
    Read(&'a mut [u8]),
    Write(&'a [u8]),
    Shutdown,
}

impl Session {
    pub(super) fn socket(&self) -> &crate::ObjectOwner {
        self.socket.as_ref().expect("live TLS transport")
    }

    /// Free the non-closing TLS BIO and transfer its original socket back to the caller.
    pub(super) fn into_socket(mut self) -> crate::ObjectOwner {
        self.socket.take().expect("live TLS transport")
    }

    pub(crate) fn wrap(
        ctx: &Context,
        socket: crate::ObjectOwner,
        host: Option<&str>,
        server: bool,
    ) -> Result<Self, bio::BioFailure> {
        let api = ctx.api;
        api.clear();
        // SAFETY: SSL_new takes its own reference to this live context.
        let handle = NonNull::new(unsafe { (api.SSL_new)(ctx.handle.as_ptr()) })
            .ok_or_else(|| api.error("SSL_new"))?;
        let session = Self {
            api,
            handle,
            socket: Some(socket),
            server,
        };
        if let Some(host) = host {
            api.set_peer_name(handle.as_ptr(), host, ctx.check_hostname)?;
        }
        // SAFETY: SSL_set_fd creates a non-closing socket BIO. The owned descriptor
        // remains valid for the full lifetime of this SSL object.
        if unsafe { (api.SSL_set_fd)(handle.as_ptr(), socket::fd(session.socket())) } != 1 {
            return Err(api.error("SSL_set_fd").into());
        }
        let handshake = session.blocking_io(
            if server {
                Operation::Accept
            } else {
                Operation::Connect
            },
            Api::ssl_failure_kind,
        );
        api.verify_peer(handle.as_ptr(), !server && handshake.is_ok())?;
        handshake?;
        Ok(session)
    }

    /// Retry a blocking socket BIO operation with the same buffer and length.
    fn blocking_io<E: From<Failure>>(
        &self,
        mut operation: Operation<'_>,
        failure: impl FnOnce(&Api, i32, &str) -> E,
    ) -> Result<usize, E> {
        let api = self.api;
        let ssl = self.handle.as_ptr();
        let mut sent_shutdown = false;
        loop {
            api.clear();
            let mut n = 0;
            clear_errno();
            // SAFETY: all operations borrow our live SSL, and their buffers and
            // output counter outlive each attempt.
            let (rc, label) = unsafe {
                match &mut operation {
                    Operation::Shutdown => ((api.SSL_shutdown)(ssl), "SSL_shutdown"),
                    Operation::Accept => ((api.SSL_accept)(ssl), "SSL_accept"),
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
            if rc == 0 && matches!(operation, Operation::Shutdown) {
                // Retry for the peer's close_notify without consuming subsequent
                // cleartext. CPython accepts a second zero from SSL_shutdown.
                if sent_shutdown {
                    return Ok(0);
                }
                sent_shutdown = true;
                // SAFETY: read-ahead is a setting on this live SSL.
                unsafe { (api.SSL_set_read_ahead)(ssl, 0) };
                continue;
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
                    socket::wait_blocking(socket::fd(self.socket()), events)
                        .map_err(Failure::Io)?;
                }
                SSL_ERROR_SYSCALL if os_error.kind() == std::io::ErrorKind::Interrupted => continue,
                SSL_ERROR_SYSCALL if os_error.raw_os_error() != Some(0) => {
                    return Err(Failure::Io(os_error).into());
                }
                _ => return Err(failure(api, kind, label)),
            }
        }
    }

    pub(crate) fn channel_binding(&self) -> Option<Vec<u8>> {
        let ssl = self.handle.as_ptr();
        // SAFETY: this SSL completed its handshake. On a full handshake the client
        // sends the first Finished; on resumption the server sends it first.
        let resumed = unsafe { (self.api.SSL_session_reused)(ssl) } != 0;
        let finished = if resumed == self.server {
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

    pub(crate) fn certificate_chain(&self, verified: bool) -> Result<Vec<Vec<u8>>, Failure> {
        let api = self.api;
        api.clear();
        // SAFETY: both stacks and their X509 entries are borrowed from this live SSL.
        let chain = unsafe {
            if verified {
                (api.SSL_get0_verified_chain)(self.handle.as_ptr())
            } else {
                (api.SSL_get_peer_cert_chain)(self.handle.as_ptr())
            }
        };
        if chain.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: OpenSSL returned a live stack above.
        let count = unsafe { (api.OPENSSL_sk_num)(chain) };
        let mut certificates = Vec::new();
        // OpenSSL omits the peer leaf from the wire stack on server-side connections.
        if self.server && !verified {
            // SAFETY: a non-null peer chain belongs to the current handshake.
            let leaf = unsafe { (api.SSL_get0_peer_certificate)(self.handle.as_ptr()) };
            certificates.push(api.encode_certificate(leaf)?);
        }
        for index in 0..count {
            // SAFETY: this index is within the live stack returned above.
            let certificate = unsafe { (api.OPENSSL_sk_value)(chain, index) };
            certificates.push(api.encode_certificate(certificate)?);
        }
        Ok(certificates)
    }

    pub(crate) fn send(&mut self, bytes: &[u8]) -> Result<usize, Failure> {
        let n = self.blocking_io(Operation::Write(bytes), |api, _, label| api.error(label))?;
        if (n == 0 && !bytes.is_empty()) || n > bytes.len() {
            return Err(Failure::Other("invalid SSL_write byte count".into()));
        }
        Ok(n)
    }

    pub(crate) fn sendall(&mut self, mut bytes: &[u8]) -> Result<(), Failure> {
        while !bytes.is_empty() {
            let n = self.send(bytes)?;
            bytes = &bytes[n..];
        }
        Ok(())
    }

    pub(in crate::dynamic) fn read(&mut self, out: &mut [u8]) -> Result<usize, bio::BioFailure> {
        if out.is_empty() {
            return Ok(0);
        }
        let result = self.blocking_io(Operation::Read(out), Api::ssl_failure_kind);
        self.api.finish_read(self.handle, result)
    }

    pub(in crate::dynamic) fn pending(&self) -> Result<usize, bio::BioFailure> {
        self.api.ssl_pending(self.handle)
    }

    /// Exchange close notifications while the session still owns the socket.
    pub(in crate::dynamic) fn shutdown(&mut self) -> Result<(), bio::BioFailure> {
        self.blocking_io(Operation::Shutdown, Api::ssl_failure_kind)?;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: free the SSL and its non-closing BIO while the socket is still valid.
        unsafe { (self.api.SSL_free)(self.handle.as_ptr()) };
        // An endpoint operation may still hold a reference across a Python callback.
        if let Some(socket) = &self.socket {
            let _ = socket::close(socket);
        }
    }
}
