//! Load trust anchors into the context's existing OpenSSL store.
use std::ffi::{CStr, c_int, c_ulong};
use std::ptr::{self, NonNull};

use super::{Api, Context, Failure, Handle, clear_errno, reject_password};

pub(crate) enum CaData<'a> {
    Pem(&'a [u8]),
    Der(&'a [u8]),
}

// OpenSSL 3 ERR_PACK(library, 0, reason) from <openssl/{err,pemerr,x509err}.h>.
const PEM_NO_START_LINE: c_ulong = (9 << 23) | 108;
const X509_ALREADY_IN_HASH_TABLE: c_ulong = (11 << 23) | 101;

impl Context {
    pub(crate) fn load_verify_locations(
        &mut self,
        cafile: Option<&CStr>,
        capath: Option<&CStr>,
        cadata: Option<CaData<'_>>,
    ) -> Result<(), Failure> {
        let api = self.api;
        api.clear();
        if let Some(data) = cadata {
            let result = self.add_ca_certs(data);
            api.clear();
            result?;
        }
        if cafile.is_some() || capath.is_some() {
            clear_errno();
            // SAFETY: both optional paths are terminated and outlive this call.
            let ok = unsafe {
                (api.SSL_CTX_load_verify_locations)(
                    self.handle.as_ptr(),
                    cafile.map_or(ptr::null(), CStr::as_ptr),
                    capath.map_or(ptr::null(), CStr::as_ptr),
                )
            };
            if ok != 1 {
                // File reads can fail with errno even when only an X509 error was queued.
                let os_error = std::io::Error::last_os_error();
                let error = if os_error.raw_os_error() != Some(0) {
                    Failure::Io(os_error)
                } else {
                    // SAFETY: this thread's last error belongs to the failed OpenSSL call.
                    api.trust_error(unsafe { (api.ERR_peek_last_error)() }, None)
                };
                api.clear();
                return Err(error);
            }
        }
        Ok(())
    }

    fn add_ca_certs(&self, data: CaData<'_>) -> Result<(), Failure> {
        let bytes = match data {
            CaData::Pem(bytes) | CaData::Der(bytes) => bytes,
        };
        // The Python boundary rejects empty and oversized inputs before opening the BIO.
        let len = c_int::try_from(bytes.len())
            .map_err(|_| Failure::Other("Certificate data is too long.".into()))?;
        let api = self.api;
        // SAFETY: the read-only memory BIO borrows exactly len bytes until BIO_free below.
        let bio = NonNull::new(unsafe { (api.BIO_new_mem_buf)(bytes.as_ptr().cast(), len) })
            .ok_or_else(|| api.trust_error(0, Some("Can't allocate buffer")))?;
        // SAFETY: this live SSL_CTX owns the store. The BIO remains valid through decoding.
        let store = unsafe { (api.SSL_CTX_get_cert_store)(self.handle.as_ptr()) };
        let result = api.read_ca_certs(store, bio.as_ptr(), matches!(data, CaData::Pem(_)));
        // SAFETY: all decoded certificates are now owned by the store or have been freed.
        unsafe { (api.BIO_free)(bio.as_ptr()) };
        result
    }
}

impl Api {
    fn read_ca_certs(&self, store: Handle, bio: Handle, pem: bool) -> Result<(), Failure> {
        let mut loaded = false;
        // SAFETY: the caller retains the context, BIO and its backing slice throughout.
        // The decoders return owned X509s. Adding one takes a separate store reference.
        unsafe {
            loop {
                if !pem && (self.BIO_ctrl_pending)(bio) == 0 {
                    return Ok(());
                }
                let cert = if pem {
                    (self.PEM_read_bio_X509)(bio, ptr::null_mut(), reject_password, ptr::null_mut())
                } else {
                    (self.d2i_X509_bio)(bio, ptr::null_mut())
                };
                if cert.is_null() {
                    break;
                }
                let ok = (self.X509_STORE_add_cert)(store, cert);
                (self.X509_free)(cert);
                if ok == 0 {
                    if (self.ERR_peek_last_error)() != X509_ALREADY_IN_HASH_TABLE {
                        break;
                    }
                    self.clear();
                }
                loaded = true;
            }

            let error = (self.ERR_peek_last_error)();
            if !loaded {
                let detail = if pem {
                    "no start line: cadata does not contain a certificate"
                } else {
                    "not enough data: cadata does not contain a certificate"
                };
                Err(self.trust_error(0, Some(detail)))
            } else if error == 0 || (pem && error == PEM_NO_START_LINE) {
                Ok(())
            } else {
                Err(self.trust_error(error, None))
            }
        }
    }

    pub(super) fn trust_error(&self, error: c_ulong, detail: Option<&str>) -> Failure {
        // OpenSSL 3 ERR_GET_LIB and ERR_GET_REASON. Common errors, such as
        // ERR_R_PEM_LIB, carry a numeric errno but no library-specific mnemonic.
        let code = (error & 0x7f_ffff) as i64;
        let library = match error >> 23 {
            9 => Some("PEM"),
            11 => Some("X509"),
            13 => Some("ASN1"),
            20 => Some("SSL"),
            32 => Some("BIO"),
            34 => Some("X509V3"),
            44 => Some("OSSL_STORE"),
            _ => None,
        };
        // SAFETY: the provider owns the optional static NUL-terminated reason.
        let raw = unsafe { (self.ERR_reason_error_string)(error) };
        let text = NonNull::new(raw.cast_mut()).map(|p| {
            // SAFETY: a non-null result points to the provider's static, NUL-terminated string.
            unsafe { CStr::from_ptr(p.as_ptr()).to_string_lossy() }
        });
        let reason = text
            .as_ref()
            .filter(|_| code & (1 << 19) == 0 && library.is_some())
            .map(|text| text.to_ascii_uppercase().replace(' ', "_"));
        let message = detail.map(str::to_owned).unwrap_or_else(|| {
            let detail = text.map_or_else(
                || format!("unknown error (0x{error:x})"),
                |s| s.into_owned(),
            );
            match (library, &reason) {
                (Some(lib), Some(reason)) => format!("[{lib}: {reason}] {detail}"),
                (Some(lib), None) => format!("[{lib}] {detail}"),
                _ => detail,
            }
        });
        Failure::Trust {
            code,
            library,
            reason,
            message,
        }
    }
}
