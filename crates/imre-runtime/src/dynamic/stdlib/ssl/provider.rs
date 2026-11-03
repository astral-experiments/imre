//! Blocking TLS connections using the optional system OpenSSL 3 library.
use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::os::fd::{AsRawFd, OwnedFd};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

pub(in crate::dynamic) mod bio;
pub(in crate::dynamic) mod objects;
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
    BIO_s_mem() -> Handle;
    BIO_new(Handle) -> Handle;
    BIO_free(Handle) -> c_int;
    BIO_ctrl_pending(Handle) -> usize;
    BIO_ctrl(Handle, c_int, c_long, Handle) -> c_long;
    BIO_set_flags(Handle, c_int) -> ();
    BIO_clear_flags(Handle, c_int) -> ();
    BIO_read(Handle, *mut c_void, c_int) -> c_int;
    BIO_write(Handle, *const c_void, c_int) -> c_int;
    TLS_client_method() -> Handle;
    TLS_server_method() -> Handle;
    SSL_CTX_set_default_passwd_cb(Handle, PasswordCallback) -> ();
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
    SSL_CTX_set_verify(Handle, c_int, VerifyCallback) -> ();
    SSL_CTX_set_cipher_list(Handle, *const c_char) -> c_int;
    SSL_CTX_get_security_level(Handle) -> c_int;
    SSL_CTX_set_default_verify_paths(Handle) -> c_int;
    SSL_CTX_get_cert_store(Handle) -> Handle;
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
    SSL_accept(Handle) -> c_int;
    SSL_connect(Handle) -> c_int;
    SSL_get_verify_result(Handle) -> c_long;
    SSL_get0_peer_certificate(Handle) -> Handle;
    SSL_get0_verified_chain(Handle) -> Handle;
    SSL_get_peer_cert_chain(Handle) -> Handle;
    OPENSSL_sk_num(Handle) -> c_int;
    OPENSSL_sk_value(Handle, c_int) -> Handle;
    i2d_X509(Handle, *mut *mut u8) -> c_int;
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

    fn file_error(&self, operation: &str) -> Failure {
        // OpenSSL 3 records a failed file open as the first error and may
        // append BIO and X509 errors. Decode its original errno from err.h.
        const SYSTEM_FLAG: c_ulong = 1 << 31;
        // SAFETY: the calling thread's queue was cleared before this operation.
        let first = unsafe { (self.ERR_peek_error)() };
        if first & SYSTEM_FLAG != 0 {
            let errno = (first & (SYSTEM_FLAG - 1)) as i32;
            return Failure::Io(std::io::Error::from_raw_os_error(errno));
        }
        self.error(operation)
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

// A password was required for this PEM. Never use OpenSSL's interactive UI.
unsafe extern "C" fn reject_password(_: *mut c_char, _: c_int, _: c_int, _: Handle) -> c_int {
    -1
}

pub(crate) struct Context {
    api: &'static Api,
    handle: NonNull<c_void>,
    server: bool,
}

impl Context {
    pub(crate) fn is_server(&self) -> bool {
        self.server
    }

    pub(crate) fn new(server: bool) -> Result<Self, Failure> {
        let api = api().ok_or_else(|| Failure::Other("OpenSSL 3 TLS is unavailable".into()))?;
        api.clear();
        // SAFETY: both methods are process-lifetime constants in this provider.
        let method = if server {
            api.TLS_server_method
        } else {
            api.TLS_client_method
        };
        let handle = NonNull::new(unsafe { (api.SSL_CTX_new)(method()) })
            .ok_or_else(|| api.error("SSL_CTX_new"))?;
        let mut ctx = Self {
            api,
            handle,
            server,
        };
        // SAFETY: the context is live and owns both settings. Clients verify the peer.
        // Servers have CPython's default policy of no client authentication.
        unsafe {
            (api.SSL_CTX_set_verify)(handle.as_ptr(), if server { 0 } else { 0x01 | 0x02 }, None);
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

    pub(crate) fn verify_flags(&self) -> u64 {
        // SAFETY: the borrowed parameters remain valid throughout the query.
        unsafe {
            (self.api.X509_VERIFY_PARAM_get_flags)((self.api.SSL_CTX_get0_param)(
                self.handle.as_ptr(),
            )) as u64
        }
    }

    pub(super) fn cert_store_stats(&self) -> Option<[c_int; 3]> {
        let api = self.api;
        let mut certificates = Vec::new();
        // SAFETY: the context owns this store. Retain each X509 while holding the store
        // lock so we can classify it after unlocking. This works on OpenSSL 3.0–3.2
        // as well, which do not export X509_STORE_get1_objects.
        unsafe {
            let store = (api.SSL_CTX_get_cert_store)(self.handle.as_ptr());
            if store.is_null() || (api.X509_STORE_lock)(store) != 1 {
                return None;
            }
            let snapshot = (|| {
                let objects = (api.X509_STORE_get0_objects)(store);
                let count = (api.OPENSSL_sk_num)(objects);
                certificates
                    .try_reserve_exact(usize::try_from(count).ok()?)
                    .ok()?;
                let mut crls = 0;
                for index in 0..count {
                    let object = NonNull::new((api.OPENSSL_sk_value)(objects, index))?;
                    match (api.X509_OBJECT_get_type)(object.as_ptr()) {
                        1 => {
                            // X509_LU_X509
                            let cert = (api.X509_OBJECT_get0_X509)(object.as_ptr());
                            if cert.is_null() || (api.X509_up_ref)(cert) != 1 {
                                return None;
                            }
                            certificates.push(cert);
                        }
                        2 => crls += 1, // X509_LU_CRL
                        _ => (),
                    }
                }
                Some(crls)
            })();
            let crls = if (api.X509_STORE_unlock)(store) == 1 {
                snapshot
            } else {
                None
            };
            let x509 = certificates.len() as c_int;
            let mut ca = 0;
            // Release every acquired reference, including those retained before a failure.
            for cert in certificates {
                if crls.is_some() && (api.X509_check_ca)(cert) != 0 {
                    ca += 1;
                }
                (api.X509_free)(cert);
            }
            crls.map(|crls| [x509, crls, ca])
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
            return Err(self.api.file_error("SSL_CTX_load_verify_locations"));
        }
        Ok(())
    }

    pub(crate) fn load_cert_chain(
        &mut self,
        certfile: &CStr,
        keyfile: &CStr,
    ) -> Result<(), Failure> {
        let api = self.api;
        api.clear();
        // SAFETY: SSL_new inherits this context's policy and noninteractive password callback.
        // Stage credentials on a separate SSL so parse errors leave the installed pair intact.
        let tmp = NonNull::new(unsafe { (api.SSL_new)(self.handle.as_ptr()) })
            .ok_or_else(|| api.error("SSL_new"))?;
        let result = self.install_credentials(tmp, certfile, keyfile);
        // SAFETY: SSL_CTX_use_cert_and_key retained its own references on success.
        unsafe { (api.SSL_free)(tmp.as_ptr()) };
        result
    }

    fn install_credentials(
        &mut self,
        tmp: NonNull<c_void>,
        certfile: &CStr,
        keyfile: &CStr,
    ) -> Result<(), Failure> {
        let api = self.api;
        let ssl = tmp.as_ptr();
        // SAFETY: both paths are terminated; the temporary SSL lives throughout the call.
        if unsafe { (api.SSL_use_certificate_chain_file)(ssl, certfile.as_ptr()) } != 1 {
            return Err(api.file_error("SSL_use_certificate_chain_file"));
        }
        // SAFETY: record the selected certificate, without taking ownership. Loading a key
        // of another algorithm can silently switch to an older credential slot.
        let loaded = unsafe { (api.SSL_get_certificate)(ssl) };
        api.clear();
        const SSL_FILETYPE_PEM: c_int = 1;
        // SAFETY: OpenSSL retains the decoded key in the temporary SSL.
        if unsafe { (api.SSL_use_PrivateKey_file)(ssl, keyfile.as_ptr(), SSL_FILETYPE_PEM) } != 1 {
            return Err(api.file_error("SSL_use_PrivateKey_file"));
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
                self.handle.as_ptr(),
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
    server: bool,
}

enum Operation<'a> {
    Accept,
    Connect,
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

impl Session {
    pub(crate) fn wrap(ctx: &Context, fd: OwnedFd, host: Option<&str>) -> Result<Self, Failure> {
        if ctx.server == host.is_some() {
            return Err(Failure::Other(
                "TLS role and server hostname disagree".into(),
            ));
        }
        let api = ctx.api;
        api.clear();
        // SAFETY: SSL_new takes its own reference to this live context.
        let handle = NonNull::new(unsafe { (api.SSL_new)(ctx.handle.as_ptr()) })
            .ok_or_else(|| api.error("SSL_new"))?;
        let session = Self {
            api,
            handle,
            socket: fd,
            server: ctx.server,
        };
        if let Some(host) = host {
            session.set_peer_name(host)?;
        }
        // SAFETY: SSL_set_fd creates a non-closing socket BIO. The owned descriptor
        // remains valid for the full lifetime of this SSL object.
        if unsafe { (api.SSL_set_fd)(handle.as_ptr(), session.socket.as_raw_fd()) } != 1 {
            return Err(api.error("SSL_set_fd"));
        }
        if ctx.server {
            session.blocking_io(Operation::Accept)?;
            return Ok(session);
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
            // SAFETY: only clear this thread's errno. All operations borrow our
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
            certificates.push(self.encode_certificate(leaf)?);
        }
        for index in 0..count {
            // SAFETY: this index is within the live stack returned above.
            let certificate = unsafe { (api.OPENSSL_sk_value)(chain, index) };
            certificates.push(self.encode_certificate(certificate)?);
        }
        Ok(certificates)
    }

    fn encode_certificate(&self, certificate: Handle) -> Result<Vec<u8>, Failure> {
        let api = self.api;
        if certificate.is_null() {
            return Err(Failure::Other("missing certificate in TLS chain".into()));
        }
        // SAFETY: the X509 remains live through both calls. A null output queries its DER size.
        let size = unsafe { (api.i2d_X509)(certificate, ptr::null_mut()) };
        if size <= 0 {
            return Err(api.error("i2d_X509"));
        }
        let mut der = vec![0u8; size as usize];
        let mut cursor = der.as_mut_ptr();
        // SAFETY: the unchanged certificate has exactly size encoded bytes. i2d_X509
        // advances cursor, while der retains ownership of the original allocation.
        if unsafe { (api.i2d_X509)(certificate, &mut cursor) } != size {
            return Err(api.error("i2d_X509"));
        }
        Ok(der)
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
