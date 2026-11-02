//! The optional system OpenSSL 3 provider. Executables without libcrypto can
//! still use the bundled digest implementations.
use std::ffi::{CStr, CString, c_char, c_int, c_ulong, c_void};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

use super::{Error, Result};

type Fetch = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *mut c_void;
type Free = unsafe extern "C" fn(*mut c_void);
type Size = unsafe extern "C" fn(*const c_void) -> c_int;
type Fips = unsafe extern "C" fn(*mut c_void) -> c_int;
type Pbkdf2 = unsafe extern "C" fn(
    *const c_char,
    c_int,
    *const u8,
    c_int,
    c_int,
    *const c_void,
    c_int,
    *mut u8,
) -> c_int;
type Scrypt = unsafe extern "C" fn(
    *const c_char,
    usize,
    *const u8,
    usize,
    u64,
    u64,
    u64,
    u64,
    *mut u8,
    usize,
) -> c_int;
type PeekError = unsafe extern "C" fn() -> c_ulong;
type ClearErrors = unsafe extern "C" fn();
type ErrorString = unsafe extern "C" fn(c_ulong) -> *const c_char;

#[cfg(unix)]
macro_rules! symbol {
    ($handle:expr, $name:expr, $signature:ty) => {{
        // SAFETY: the candidate library is live and the symbol name is terminated.
        let address = unsafe { libc::dlsym($handle.as_ptr(), $name.as_ptr()) };
        NonNull::new(address).map(|p| {
            // SAFETY: each name is paired with its declared OpenSSL 3 function signature.
            unsafe { std::mem::transmute::<*mut c_void, $signature>(p.as_ptr()) }
        })
    }};
}

mod stream;
pub(crate) use stream::Stream;

pub(crate) struct Provider {
    fetch: Fetch,
    free: Free,
    size: Size,
    fips: Fips,
    pbkdf2: Pbkdf2,
    scrypt: Option<Scrypt>,
    peek: PeekError,
    clear: ClearErrors,
    library: ErrorString,
    reason: ErrorString,
    stream: stream::Functions,
}

pub(crate) fn provider() -> Option<&'static Provider> {
    static PROVIDER: OnceLock<Option<Provider>> = OnceLock::new();
    PROVIDER.get_or_init(Provider::load).as_ref()
}

pub(super) fn required() -> Result<&'static Provider> {
    provider().ok_or_else(|| Error::new("ImportError", "OpenSSL is unavailable"))
}

impl Provider {
    fn load() -> Option<Self> {
        #[cfg(target_os = "linux")]
        let name = c"libcrypto.so.3";
        #[cfg(target_os = "macos")]
        let name = c"libcrypto.3.dylib";
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return None;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // SAFETY: dlopen receives a terminated soname and valid loader flags.
            let handle = NonNull::new(unsafe {
                libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
            })?;
            let result = Self::functions(handle);
            // A successful process-wide provider retains its loader reference for
            // the lifetime of all cached function pointers and fetched digests.
            if result.is_none() {
                // SAFETY: no function pointers from this handle were published.
                unsafe { libc::dlclose(handle.as_ptr()) };
            }
            result
        }
    }

    #[cfg(unix)]
    fn functions(handle: NonNull<c_void>) -> Option<Self> {
        Some(Self {
            fetch: symbol!(handle, c"EVP_MD_fetch", Fetch)?,
            free: symbol!(handle, c"EVP_MD_free", Free)?,
            size: symbol!(handle, c"EVP_MD_get_size", Size)?,
            fips: symbol!(handle, c"EVP_default_properties_is_fips_enabled", Fips)?,
            pbkdf2: symbol!(handle, c"PKCS5_PBKDF2_HMAC", Pbkdf2)?,
            scrypt: symbol!(handle, c"EVP_PBE_scrypt", Scrypt),
            peek: symbol!(handle, c"ERR_peek_last_error", PeekError)?,
            clear: symbol!(handle, c"ERR_clear_error", ClearErrors)?,
            library: symbol!(handle, c"ERR_lib_error_string", ErrorString)?,
            reason: symbol!(handle, c"ERR_reason_error_string", ErrorString)?,
            stream: stream::Functions::load(handle)?,
        })
    }

    pub(crate) fn has_scrypt(&self) -> bool {
        self.scrypt.is_some()
    }

    pub(crate) fn fips_mode(&self) -> i32 {
        // SAFETY: null selects the default OpenSSL library context.
        unsafe { (self.fips)(ptr::null_mut()) }
    }

    pub(crate) fn digest(&'static self, name: &str) -> Result<Digest> {
        self.digest_with_security(name, true)
    }

    pub(crate) fn digest_with_security(&'static self, name: &str, used: bool) -> Result<Digest> {
        // Underscored Python algorithm names differ from their OpenSSL aliases.
        let native_name = match name {
            "sha3_224" => "SHA3-224",
            "sha3_256" => "SHA3-256",
            "sha3_384" => "SHA3-384",
            "sha3_512" => "SHA3-512",
            "sha512_224" | "SHA512_224" => "SHA512-224",
            "sha512_256" | "SHA512_256" => "SHA512-256",
            "shake_128" => "SHAKE128",
            "shake_256" => "SHAKE256",
            "blake2b" => "BLAKE2B-512",
            "blake2s" => "BLAKE2S-256",
            other => other,
        };
        let native_name = CString::new(native_name)
            .map_err(|_| Error::new("ValueError", "embedded null character"))?;
        let properties = if used { ptr::null() } else { c"-fips".as_ptr() };
        // SAFETY: the default context is requested and the strings remain valid
        // throughout the call. A nonnull return owns one EVP_MD reference.
        let md = NonNull::new(unsafe {
            (self.clear)();
            (self.fetch)(ptr::null_mut(), native_name.as_ptr(), properties)
        })
        .ok_or_else(|| {
            let mut error = self.error(&format!("unsupported hash type {name}"));
            error.kind = super::UNSUPPORTED_DIGEST.into();
            error
        })?;
        Ok(Digest { provider: self, md })
    }

    fn error(&self, fallback: &str) -> Error {
        // SAFETY: these functions read the calling thread's OpenSSL error queue;
        // library and reason strings are owned by the loaded library.
        unsafe {
            let code = (self.peek)();
            (self.clear)();
            if code != 0 {
                let reason = (self.reason)(code);
                if !reason.is_null() {
                    let reason = CStr::from_ptr(reason).to_string_lossy();
                    let library = (self.library)(code);
                    if !library.is_null() {
                        let library = CStr::from_ptr(library).to_string_lossy();
                        return Error::new("ValueError", format!("[{library}] {reason}"));
                    }
                    return Error::new("ValueError", reason);
                }
            }
        }
        Error::new("ValueError", fallback)
    }

    fn derive(&self, size: usize, write: impl FnOnce(*mut u8) -> c_int) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        result
            .try_reserve_exact(size)
            .map_err(|_| Error::new("MemoryError", ""))?;
        if write(result.as_mut_ptr()) != 1 {
            return Err(self.error("no reason supplied"));
        }
        // SAFETY: either KDF initializes exactly size output bytes on success.
        unsafe { result.set_len(size) };
        Ok(result)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "matches the native scrypt parameters"
    )]
    pub(crate) fn scrypt(
        &self,
        password: &[u8],
        salt: &[u8],
        n: u64,
        r: u64,
        p: u64,
        maxmem: i64,
        dklen: i64,
    ) -> Result<Vec<u8>> {
        let scrypt = self
            .scrypt
            .ok_or_else(|| Error::new("AttributeError", "scrypt"))?;
        let maxmem =
            u64::try_from(maxmem).map_err(|_| Error::new("ValueError", "invalid maxmem"))?;
        let size = usize::try_from(dklen).map_err(|_| Error::new("ValueError", "invalid dklen"))?;
        // CPython lets OpenSSL check memory limits and cost combinations before
        // allocating the user's output. A null output only validates the costs.
        // SAFETY: all three null buffers have length zero.
        if unsafe {
            (self.clear)();
            scrypt(
                ptr::null(),
                0,
                ptr::null(),
                0,
                n,
                r,
                p,
                maxmem,
                ptr::null_mut(),
                0,
            )
        } != 1
        {
            return Err(self.error("Invalid parameter combination for n, r, p, maxmem."));
        }
        self.derive(size, |out| {
            // SAFETY: both slices remain live for the call and out holds size bytes.
            unsafe {
                scrypt(
                    password.as_ptr().cast(),
                    password.len(),
                    salt.as_ptr(),
                    salt.len(),
                    n,
                    r,
                    p,
                    maxmem,
                    out,
                    size,
                )
            }
        })
    }
}

pub(crate) struct Digest {
    provider: &'static Provider,
    md: NonNull<c_void>,
}

impl Drop for Digest {
    fn drop(&mut self) {
        // SAFETY: this is the reference returned by EVP_MD_fetch.
        unsafe { (self.provider.free)(self.md.as_ptr()) };
    }
}

impl Digest {
    pub(crate) fn size(&self) -> i32 {
        // SAFETY: the digest is owned by self through the call.
        unsafe { (self.provider.size)(self.md.as_ptr()) }
    }

    pub(crate) fn pbkdf2(
        &self,
        password: &[u8],
        salt: &[u8],
        iterations: i32,
        dklen: i32,
    ) -> Result<Vec<u8>> {
        let password_len = c_int::try_from(password.len())
            .map_err(|_| Error::new("OverflowError", "password is too long."))?;
        let salt_len = c_int::try_from(salt.len())
            .map_err(|_| Error::new("OverflowError", "salt is too long."))?;
        let size = usize::try_from(dklen).map_err(|_| Error::new("ValueError", "invalid dklen"))?;
        self.provider.derive(size, |out| {
            // SAFETY: lengths and allocation are checked; the EVP_MD and input
            // buffers remain live throughout this synchronous operation.
            unsafe {
                (self.provider.clear)();
                (self.provider.pbkdf2)(
                    password.as_ptr().cast(),
                    password_len,
                    salt.as_ptr(),
                    salt_len,
                    iterations,
                    self.md.as_ptr(),
                    dklen,
                    out,
                )
            }
        })
    }
}
