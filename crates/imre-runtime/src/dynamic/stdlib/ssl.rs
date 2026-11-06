//! Private bindings for the system TLS provider. The Python ssl module owns the public types.

use std::{cell::RefCell, ffi::CString, io, rc::Rc};

use super::{Context, Error, FALSE, Id, NONE, Result, Value};
use crate::objects::{errno_error, socket};

pub(crate) mod provider;

const PROVIDER_ERROR: &str = "_imre_ssl.Error";

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) -> Result<()> {
    let version = provider::version()
        .ok_or_else(|| Error::new("ImportError", "system TLS provider unavailable"))?;
    let version = ctx.text(version);
    ctx.bind(scope, "OPENSSL_VERSION", version);
    let class = if let Some(&class) = ctx.builtins.get(PROVIDER_ERROR) {
        class
    } else {
        let fields = ctx.scope("_imre_ssl", None);
        let module = ctx.text("_imre_ssl");
        ctx.bind(fields, "__module__", module);
        let base = ctx.builtin("ValueError");
        let class = ctx.allocate_class("Error".into(), fields, vec![base])?;
        ctx.builtins.insert(PROVIDER_ERROR.into(), class);
        class
    };
    ctx.bind(scope, "Error", class);
    for name in [
        "context",
        "server_context",
        "generic_context",
        "protocol",
        "verify_mode",
        "check_hostname",
        "num_tickets",
        "security_level",
        "minimum_version",
        "maximum_version",
        "options",
        "verify_flags",
        "cert_store_stats",
        "get_ca_certs",
        "session_stats",
        "raise_context_receiver_error",
        "default_verify_paths",
        "default_verify_flags",
        "set_alpn_protocols",
        "set_post_handshake_auth",
        "get_ciphers",
        "set_ciphers",
        "set_ecdh_curve",
        "load_verify_locations",
        "load_dh_params",
        "load_cert_chain",
        "wrap",
        "is_session",
        "release_session",
        "read",
        "pending",
        "tls_shutdown",
        "shutdown",
        "fileno",
        "getsockname",
        "getpeername",
        "listen",
        "accept",
        "wrap_bio",
        "bio_handshake",
        "bio_read",
        "bio_write",
        "bio_pending",
        "bio_getpeercert",
        "bio_shutdown",
        "channel_binding",
        "hostname_checks_common_name",
        "set_hostname_checks_common_name",
        "get_verified_chain",
        "get_unverified_chain",
        "send",
        "sendall",
        "recv",
        "close",
    ] {
        let function = ctx.builtin(&format!("_imre_ssl.{name}"));
        ctx.bind(scope, name, function);
    }
    Ok(())
}

fn context(ctx: &Context, id: Id) -> Result<&RefCell<provider::Context>> {
    match ctx.heap.get(id) {
        Value::TlsContext(handle) => Ok(handle),
        _ => Err(Error::new("TypeError", "expected a TLS context")),
    }
}

fn context_for_role(ctx: &Context, id: Id, server: bool) -> Result<&RefCell<provider::Context>> {
    let context = context(ctx, id)?;
    if matches!(
        (context.borrow().protocol, server),
        (provider::Protocol::Client, true) | (provider::Protocol::Server, false)
    ) {
        return Err(Error::new(
            "ValueError",
            "TLS role is incompatible with the context protocol",
        ));
    }
    Ok(context)
}

fn session(ctx: &Context, id: Id) -> Result<&RefCell<Option<provider::Session>>> {
    match ctx.heap.get(id) {
        Value::TlsSession(handle) => Ok(handle),
        Value::None => Err(closed()),
        _ => Err(Error::new("TypeError", "expected a TLS session")),
    }
}

fn without_session(ctx: &Context, handle: Id) -> bool {
    matches!(
        ctx.heap.get(handle),
        Value::Socket(_) | Value::TlsListener(_) | Value::None
    )
}

/// Pending sockets must never consume or emit application bytes before TLS authenticates them.
fn pending_io_error(ctx: &Context, handle: Id, errno: i32) -> Error {
    match ctx.heap.get(handle) {
        Value::TlsListener(s) if socket::fd(s) >= 0 => {
            errno_error(io::Error::from_raw_os_error(errno)).into()
        }
        _ => closed(),
    }
}

fn memory_bio<'a>(ctx: &'a Context, id: Id, arg: &str) -> Result<&'a RefCell<provider::bio::Bio>> {
    if let Value::MemoryBio(bio) = ctx.heap.get(ctx.scalar_value(id)) {
        Ok(bio)
    } else {
        let ty = if id == NONE {
            "None"
        } else {
            ctx.type_name(id)
        };
        Err(Error::new(
            "TypeError",
            format!("_wrap_bio() argument '{arg}' must be _ssl.MemoryBIO, not {ty}"),
        ))
    }
}

fn bio_session(ctx: &Context, handle: Id) -> Result<Rc<RefCell<provider::bio::BioSession>>> {
    match ctx.heap.get(handle) {
        Value::TlsBioSession(session) => Ok(Rc::clone(session)),
        _ => Err(Error::new("TypeError", "expected a TLS memory BIO session")),
    }
}

fn bio_failure(ctx: &mut Context, error: provider::bio::BioFailure) -> Result<Id> {
    use provider::bio::BioFailure;
    let status = match error {
        BioFailure::WantRead => "WANT_READ",
        BioFailure::WantWrite => "WANT_WRITE",
        BioFailure::ZeroReturn => "ZERO_RETURN",
        BioFailure::Eof => "EOF",
        BioFailure::UnexpectedEof => "UNEXPECTED_EOF_WHILE_READING",
        BioFailure::ProtocolIsShutdown => "PROTOCOL_IS_SHUTDOWN",
        BioFailure::ShutdownInInit => "SHUTDOWN_WHILE_IN_INIT",
        BioFailure::ProtocolError { reason, message } => {
            let reason = reason.map_or(NONE, |s| ctx.text(s));
            let message = ctx.text(message);
            return Ok(ctx.alloc(Value::Tuple(vec![reason, message])));
        }
        BioFailure::Tls(provider::Failure::Verify { code, message }) => {
            return Ok(verify_failure(ctx, code, message));
        }
        BioFailure::Tls(error) => return Err(provider_error(ctx, error)),
    };
    Ok(ctx.text(status))
}

fn bio_count(
    ctx: &mut Context,
    result: std::result::Result<usize, provider::bio::BioFailure>,
) -> Result<Id> {
    match result {
        Ok(count) => Ok(ctx.int(count as i64)),
        Err(error) => bio_failure(ctx, error),
    }
}

fn read_plaintext(
    ctx: &mut Context,
    size: Id,
    buffer: Id,
    read: impl FnOnce(&mut [u8]) -> std::result::Result<usize, provider::bio::BioFailure>,
) -> Result<Id> {
    // __index__ may resize the destination or reenter the TLS session.
    let size = ctx
        .index_integer(size)?
        .to_i64()
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| {
            Error::new(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )
        })?;
    if buffer != NONE {
        let (storage, range) = match ctx.writable_byte_span(buffer) {
            Ok(Some(span)) => span,
            Err(error) if error.kind == "NotImplementedError" => return Err(error),
            _ => {
                let kind = ctx.type_name(buffer);
                return Err(Error::new(
                    "TypeError",
                    format!("read() argument 2 must be read-write bytes-like object, not {kind}"),
                ));
            }
        };
        // CPython's optional-buffer form fills the buffer for nonpositive sizes
        // and narrows the available length to C int only when clamping.
        let size = if size <= 0 || size as usize > range.len() {
            i32::try_from(range.len())
                .map_err(|_| Error::new("OverflowError", "maximum length can't fit in a C 'int'"))?
                as usize
        } else {
            size as usize
        };
        let dest = &mut ctx.writable_storage_bytes(storage)?[range.start..range.start + size];
        let result = read(dest);
        return bio_count(ctx, result);
    }

    let size = usize::try_from(size)
        .map_err(|_| Error::new("ValueError", "size should not be negative"))?;
    if size > crate::MAX_BYTES_LEN {
        return Err(Error::new("OverflowError", "byte string is too large"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| Error::new("MemoryError", ""))?;
    // Match CPython's allocation check, but SSL_read_ex returns data from at
    // most one 16 KiB TLS record. Only that prefix needs initializing.
    data.resize(size.min(16 * 1024), 0);
    match read(&mut data) {
        Ok(count) => {
            data.truncate(count);
            crate::compact_bytes(&mut data);
            Ok(ctx.alloc(Value::Bytes(data)))
        }
        Err(error) => bio_failure(ctx, error),
    }
}

fn socket_read(ctx: &mut Context, handle: Id, size: Id, buffer: Id) -> Result<Id> {
    let Value::TlsSession(socket) = ctx.heap.get(handle) else {
        return Err(closed());
    };
    let socket = Rc::clone(socket);
    read_plaintext(ctx, size, buffer, |data| {
        // The length's __index__ may have closed or unwrapped this socket.
        let mut socket = socket.borrow_mut();
        let session = socket.as_mut().ok_or_else(|| {
            provider::bio::BioFailure::Tls(provider::Failure::Io(io::Error::from_raw_os_error(
                libc::EBADF,
            )))
        })?;
        session.read(data)
    })
}

fn verify_failure(ctx: &mut Context, code: i64, message: String) -> Id {
    let code = ctx.int(code);
    let message = ctx.text(message);
    ctx.alloc(Value::Tuple(vec![code, message]))
}

fn cert_path(ctx: &mut Context, path: Id, arg: &str) -> Result<CString> {
    let type_error = ctx.builtin("TypeError");
    let path = super::posixpath::fspath(ctx, path).map_err(|error| {
        if ctx.raised_error_matches(&error, type_error) {
            Error::new(
                "TypeError",
                format!("{arg} should be a valid filesystem path"),
            )
        } else {
            error
        }
    })?;
    super::os::c_path(ctx, path, "embedded null byte")
}

fn password_bytes(ctx: &mut Context, value: Id, type_error: &str) -> Result<Vec<u8>> {
    let value = ctx.scalar_value(value);
    let data = match ctx.heap.get(value) {
        Value::Str(text) => text.as_bytes(),
        Value::SurrogateStr(_) => return ctx.encode_utf8_codepoints(value, false, "strict"),
        Value::Bytes(bytes) | Value::ByteArray(bytes) => bytes.as_slice(),
        _ => return Err(Error::new("TypeError", type_error)),
    };
    if data.len() > i32::MAX as usize {
        return Err(password_too_long(i32::MAX as usize));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(data.len())
        .map_err(|_| Error::new("MemoryError", "unable to allocate password buffer"))?;
    bytes.extend_from_slice(data);
    Ok(bytes)
}

fn password_too_long(limit: usize) -> Error {
    Error::new(
        "ValueError",
        format!("password cannot be longer than {limit} bytes"),
    )
}

fn load_cert_chain(
    ctx: &mut Context,
    handle: Id,
    certfile: Id,
    keyfile: Id,
    password: Id,
) -> Result<Id> {
    let certfile = cert_path(ctx, certfile, "certfile")?;
    let keyfile = (keyfile != NONE)
        .then(|| cert_path(ctx, keyfile, "keyfile"))
        .transpose()?;
    let callable = ctx.callable(password);
    let mut bytes = if password != NONE && !callable {
        password_bytes(ctx, password, "password should be a string or callable")?
    } else {
        Vec::new()
    };

    // Stage on a separate SSL. No borrow of the live context may survive a Python callback.
    let credentials = context(ctx, handle)?
        .borrow()
        .credentials()
        .map_err(|e| provider_error(ctx, e))?;
    let mut callback_error = None;
    let result = credentials.load_cert_chain(
        &certfile,
        keyfile.as_deref().unwrap_or(&certfile),
        &mut |out| {
            if password == NONE || callback_error.is_some() {
                return None;
            }
            if callable {
                let result = ctx.call(password, vec![], vec![]).and_then(|value| {
                    password_bytes(ctx, value, "password callback must return a string")
                });
                match result {
                    Ok(value) => bytes = value,
                    Err(error) => {
                        callback_error = Some(error);
                        return None;
                    }
                }
            }
            if bytes.len() > out.len() {
                callback_error = Some(password_too_long(out.len()));
                return None;
            }
            out[..bytes.len()].copy_from_slice(&bytes);
            Some(bytes.len())
        },
    );
    if let Some(error) = callback_error {
        return Err(error);
    }
    trust_result(ctx, result)
}

fn ca_data(ctx: &Context, input: Id) -> Result<provider::CaData<'_>> {
    let invalid = || {
        Error::new(
            "TypeError",
            "cadata should be an ASCII string or a bytes-like object",
        )
    };
    let (bytes, pem) = match ctx.heap.get(ctx.scalar_value(input)) {
        Value::Str(text) if text.is_ascii() => (text.as_bytes(), true),
        Value::Str(_) | Value::SurrogateStr(_) => return Err(invalid()),
        _ => {
            ctx.check_buffer_exporter(input)?;
            let bytes = ctx.native_buffer_bytes(input).map_err(|error| {
                if error.kind == "TypeError" {
                    invalid()
                } else {
                    error
                }
            })?;
            (bytes, false)
        }
    };
    if bytes.is_empty() {
        return Err(Error::new("ValueError", "Empty certificate data"));
    }
    if bytes.len() > i32::MAX as usize {
        return Err(Error::new("OverflowError", "Certificate data is too long."));
    }
    Ok(if pem {
        provider::CaData::Pem(bytes)
    } else {
        provider::CaData::Der(bytes)
    })
}

fn trust_result(
    ctx: &mut Context,
    result: std::result::Result<(), provider::Failure>,
) -> Result<Id> {
    match result {
        Ok(()) => Ok(NONE),
        Err(provider::Failure::Trust {
            code,
            message,
            library,
            reason,
        }) => {
            let code = ctx.int(code);
            let message = ctx.text(message);
            let library = library.map_or(NONE, |s| ctx.text(s));
            let reason = reason.map_or(NONE, |s| ctx.text(s));
            Ok(ctx.alloc(Value::Tuple(vec![code, message, library, reason])))
        }
        Err(e) => Err(provider_error(ctx, e)),
    }
}

fn closed() -> Error {
    errno_error(io::Error::from_raw_os_error(libc::EBADF)).into()
}

fn provider_error(ctx: &Context, failure: provider::Failure) -> Error {
    let message = match failure {
        provider::Failure::UnsupportedProtocol(code) => {
            return Error::new(
                "ValueError",
                format!("invalid or unsupported protocol version {code}"),
            );
        }
        provider::Failure::Io(error) => return errno_error(error).into(),
        provider::Failure::Trust { message, .. }
        | provider::Failure::Verify { message, .. }
        | provider::Failure::Other(message) => message,
    };
    let mut error = Error::new(PROVIDER_ERROR, message);
    error.class = ctx.builtins[PROVIDER_ERROR];
    error
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{name}() takes positional arguments only"),
        ));
    }
    match (name, args) {
        ("context" | "server_context" | "generic_context", []) | ("context", [_]) => {
            let code = match (name, args) {
                ("context", &[value]) => ctx.index_c_int(value)?,
                ("context", _) => provider::Protocol::Client as i32,
                ("server_context", _) => provider::Protocol::Server as i32,
                _ => provider::Protocol::Tls as i32,
            };
            let method = provider::Method::lookup(code).map_err(|e| provider_error(ctx, e))?;
            if let Some(name) = method.deprecated {
                ctx.emit_builtin_warning(
                    crate::objects::exceptions::Kind::DeprecationWarning,
                    &format!("ssl.{name} is deprecated"),
                    3,
                )?;
            }
            let handle = provider::Context::new(method).map_err(|e| provider_error(ctx, e))?;
            Ok(ctx.alloc(Value::TlsContext(Rc::new(RefCell::new(handle)))))
        }
        ("protocol", &[handle]) => {
            let protocol = context(ctx, handle)?.borrow().protocol;
            Ok(ctx.int(protocol as i64))
        }
        ("verify_mode", &[handle]) => {
            let mode = context(ctx, handle)?
                .borrow()
                .verify_mode()
                .map_err(|e| provider_error(ctx, e))?;
            Ok(ctx.int(mode))
        }
        ("verify_mode", &[handle, value]) => {
            let mode = ctx.index_c_int(value)?;
            context(ctx, handle)?
                .borrow_mut()
                .set_verify_mode(mode)
                .map_err(|s| Error::new("ValueError", s))?;
            Ok(NONE)
        }
        ("check_hostname", &[handle]) => Ok(super::builtins::boolean(
            context(ctx, handle)?.borrow().check_hostname,
        )),
        ("check_hostname", &[handle, value]) => {
            let check = ctx.truth(value)?;
            context(ctx, handle)?.borrow_mut().set_check_hostname(check);
            Ok(NONE)
        }
        ("minimum_version" | "maximum_version", [handle]) => {
            let n = context(ctx, *handle)?
                .borrow()
                .protocol_bound(name == "minimum_version");
            Ok(ctx.int(i64::from(n)))
        }
        ("minimum_version" | "maximum_version", [handle, value]) => {
            provider::versions::set(ctx, *handle, name == "minimum_version", *value)?;
            Ok(NONE)
        }
        ("options", &[handle]) => {
            let options = context(ctx, handle)?.borrow().options();
            Ok(ctx.alloc(Value::Int(crate::Int::new(options.into()))))
        }
        ("options", &[handle, value]) => {
            use num_traits::{Signed, ToPrimitive};
            let integer = ctx.index_integer(value)?;
            let bits = integer.big();
            if bits.is_negative() {
                return Err(Error::new("ValueError", "Cannot convert negative int"));
            }
            let next = bits.to_u64().ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large for C unsigned long long",
                )
            })?;
            let old = context(ctx, handle)?.borrow().options();
            let set = next & !old;
            // The five OpenSSL 3 protocol-disable bits, from SSLv3 through TLSv1.3.
            const NO_PROTOCOL: u64 = 0x3e00_0000;
            if set & NO_PROTOCOL != 0 {
                ctx.emit_builtin_warning(
                    crate::objects::exceptions::Kind::DeprecationWarning,
                    "ssl.OP_NO_SSL*/ssl.OP_NO_TLS* options are deprecated",
                    2,
                )?;
            }
            context(ctx, handle)?
                .borrow_mut()
                .change_options(old & !next, set);
            Ok(NONE)
        }
        ("num_tickets", &[handle]) => {
            let count = context(ctx, handle)?.borrow().num_tickets();
            Ok(ctx.alloc(Value::Int(crate::Int::new(count.into()))))
        }
        ("num_tickets", &[handle, value]) => {
            let count = std::ffi::c_long::try_from(ctx.index_c_long(value)?).map_err(|_| {
                Error::new("OverflowError", "Python int too large to convert to C long")
            })?;
            let count = usize::try_from(count)
                .map_err(|_| Error::new("ValueError", "value must be non-negative"))?;
            context(ctx, handle)?
                .borrow_mut()
                .set_num_tickets(count)
                .map_err(|message| Error::new("ValueError", message))?;
            Ok(NONE)
        }
        ("security_level", &[handle]) => {
            let level = context(ctx, handle)?.borrow().security_level();
            Ok(ctx.int(i64::from(level)))
        }
        ("verify_flags", &[handle]) => {
            let flags = context(ctx, handle)?.borrow().verify_flags();
            Ok(ctx.alloc(Value::Int(crate::Int::new(flags.into()))))
        }
        ("verify_flags", &[handle, value]) => {
            use std::ffi::c_ulong;
            let Some(integer) = ctx.index_value(value)? else {
                let kind = if value == NONE {
                    "None".into()
                } else if let Some(name) = ctx
                    .heap
                    .get(value)
                    .instance_class()
                    .and_then(|class| ctx.heap.get(class).heap_type())
                {
                    name.into()
                } else {
                    super::diagnostic_type_name(ctx, value)
                };
                let mut message = b"argument must be int, not ".to_vec();
                message.extend_from_slice(&kind.as_bytes()[..kind.len().min(50)]);
                let message = crate::objects::Codec::Utf8 { signature: false }
                    .decode(&message, None)
                    .map_err(|error| ctx.decode_error(error))?;
                return Err(Error::new("TypeError", message));
            };
            // CPython's "k" converter keeps the low bits of an unsigned long.
            let masked = integer.big().as_ref() & num_bigint::BigInt::from(c_ulong::MAX);
            let flags = c_ulong::try_from(masked).expect("masked to unsigned long");
            context(ctx, handle)?
                .borrow_mut()
                .set_verify_flags(flags)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("raise_context_receiver_error", &[method, receiver]) => {
            let method = ctx.str_value(method)?;
            let kind = super::diagnostic_type_name(ctx, receiver);
            let kind = &kind[..kind.floor_char_boundary(100)];
            Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' for '_ssl._SSLContext' objects doesn't apply to a '{kind}' object"
                ),
            ))
        }
        ("session_stats", &[handle]) => {
            let counts = context(ctx, handle)?.borrow().session_stats();
            let entries = counts
                .into_iter()
                .map(|(name, count)| (ctx.text(name), ctx.int(count)))
                .collect();
            Ok(ctx.dict(entries))
        }
        ("get_ca_certs", &[handle, binary]) => {
            let binary = ctx.truth(binary)?;
            provider::certificate::ca_certs(ctx, handle, binary)
        }
        ("cert_store_stats", &[handle]) => {
            let counts = context(ctx, handle)?
                .borrow()
                .cert_store_stats()
                .ok_or_else(|| Error::new("MemoryError", "failed to query cert store"))?;
            let entries = ["x509", "crl", "x509_ca"]
                .into_iter()
                .zip(counts)
                .map(|(key, count)| (ctx.text(key), ctx.int(i64::from(count))))
                .collect();
            Ok(ctx.dict(entries))
        }
        ("default_verify_paths", &[handle]) => {
            context(ctx, handle)?
                .borrow_mut()
                .set_default_verify_paths()
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("default_verify_flags", &[handle]) => {
            context(ctx, handle)?
                .borrow_mut()
                .apply_default_verify_flags()
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("set_alpn_protocols", &[handle, wire]) => {
            let Value::Bytes(protocols) = ctx.heap.get(ctx.scalar_value(wire)) else {
                return Err(Error::new("TypeError", "expected ALPN wire bytes"));
            };
            let mut context = context(ctx, handle)?.borrow_mut();
            if context.protocol != provider::Protocol::Client && !protocols.is_empty() {
                return Err(Error::new(
                    "NotImplementedError",
                    "server ALPN selection is unsupported",
                ));
            }
            context
                .set_alpn_protocols(protocols)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("set_post_handshake_auth", &[handle, enabled]) => {
            let Value::Bool(enabled) = *ctx.heap.get(enabled) else {
                return Err(Error::new(
                    "TypeError",
                    "expected bool for post_handshake_auth",
                ));
            };
            let mut context = context(ctx, handle)?.borrow_mut();
            if context.protocol != provider::Protocol::Client && enabled {
                return Err(Error::new(
                    "NotImplementedError",
                    "server post-handshake client authentication is unsupported",
                ));
            }
            context.set_post_handshake_auth(enabled);
            Ok(NONE)
        }
        ("load_cert_chain", &[handle, certfile, keyfile, password]) => {
            load_cert_chain(ctx, handle, certfile, keyfile, password)
        }
        ("load_verify_locations", &[handle, cafile, capath, cadata]) => {
            if (cafile, capath, cadata) == (NONE, NONE, NONE) {
                return Err(Error::new(
                    "TypeError",
                    "cafile, capath and cadata cannot be all omitted",
                ));
            }
            let cafile = (cafile != NONE)
                .then(|| cert_path(ctx, cafile, "cafile"))
                .transpose()?;
            let capath = (capath != NONE)
                .then(|| cert_path(ctx, capath, "capath"))
                .transpose()?;
            let cadata = (cadata != NONE).then(|| ca_data(ctx, cadata)).transpose()?;
            let result = context(ctx, handle)?.borrow_mut().load_verify_locations(
                cafile.as_deref(),
                capath.as_deref(),
                cadata,
            );
            trust_result(ctx, result)
        }
        ("set_ecdh_curve", &[handle, name]) => {
            let path = super::posixpath::fspath(ctx, name)?;
            let name = super::os::c_path(ctx, path, "embedded null byte")?;
            let result = context(ctx, handle)?.borrow_mut().set_ecdh_curve(&name);
            match result {
                Ok(false) => Ok(FALSE),
                result => trust_result(ctx, result.map(|_| ())),
            }
        }
        ("get_ciphers", &[handle]) => provider::ciphers::get(ctx, handle),
        ("load_dh_params", &[handle, path]) => {
            let mode = ctx.text("rb");
            let flags = ctx.int(0);
            super::audit::event(ctx, "open", &[path, mode, flags])?;
            let spelling = super::posixpath::fspath(ctx, path)?;
            let filename = super::os::c_path(ctx, spelling, "embedded null byte")?;
            let result = context(ctx, handle)?.borrow_mut().load_dh_params(&filename);
            match result {
                Ok(()) => Ok(NONE),
                Err(provider::Failure::Io(error)) => {
                    Err(ctx.with_errno_filename(errno_error(error).into(), path)?)
                }
                Err(provider::Failure::Trust {
                    code,
                    message,
                    library,
                    reason,
                }) => {
                    let fields = vec![
                        ctx.int(code),
                        ctx.text(message),
                        library.map_or(NONE, |s| ctx.text(s)),
                        reason.map_or(NONE, |s| ctx.text(s)),
                    ];
                    Ok(ctx.alloc(Value::Tuple(fields)))
                }
                Err(other) => Err(provider_error(ctx, other)),
            }
        }
        ("set_ciphers", &[handle, text]) => {
            let ciphers = CString::new(ctx.str_value(text)?)
                .map_err(|_| Error::new("ValueError", "embedded null byte in cipher list"))?;
            context(ctx, handle)?
                .borrow_mut()
                .set_ciphers(&ciphers)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("wrap_bio", &[handle, incoming, outgoing, hostname, side]) => {
            let server = ctx.truth(side)?;
            let handle = context_for_role(ctx, handle, server)?.borrow();
            let incoming = memory_bio(ctx, incoming, "incoming")?.borrow();
            let outgoing = memory_bio(ctx, outgoing, "outgoing")?.borrow();
            let host = if hostname == NONE {
                None
            } else {
                let host = ctx.str_value(hostname)?;
                if host.is_empty() || host.starts_with('.') {
                    return Err(Error::new(
                        "ValueError",
                        "server_hostname cannot be an empty string or start with a leading dot.",
                    ));
                }
                if host.contains('\0') {
                    return Err(Error::new(
                        "TypeError",
                        "argument must be encoded string without null bytes, not str",
                    ));
                }
                if !host.is_ascii() {
                    return Err(Error::new(
                        "NotImplementedError",
                        "TLS requires an ASCII server_hostname",
                    ));
                }
                Some(host)
            };
            let result =
                provider::bio::BioSession::wrap(&handle, &incoming, &outgoing, host, server)
                    .map_err(|e| provider_error(ctx, e))?;
            drop((handle, incoming, outgoing));
            Ok(ctx.alloc(Value::TlsBioSession(Rc::new(RefCell::new(result)))))
        }
        ("bio_getpeercert", &[handle, binary]) => {
            let binary = ctx.truth(binary)?;
            bio_session(ctx, handle)?
                .borrow()
                .peer_certificate(ctx, binary)
        }
        ("bio_read", &[handle, size, buffer]) => {
            let session = bio_session(ctx, handle)?;
            read_plaintext(ctx, size, buffer, |data| session.borrow_mut().read(data))
        }
        ("read", &[handle, size, buffer]) => socket_read(ctx, handle, size, buffer),
        ("pending", &[handle]) => {
            let result = session(ctx, handle)?
                .borrow()
                .as_ref()
                .ok_or_else(closed)?
                .pending();
            bio_count(ctx, result)
        }
        ("tls_shutdown", &[handle]) => {
            let result = session(ctx, handle)?
                .borrow_mut()
                .as_mut()
                .ok_or_else(closed)?
                .shutdown();
            match result {
                Ok(()) => Ok(NONE),
                Err(error) => bio_failure(ctx, error),
            }
        }
        ("bio_write", &[handle, data]) => {
            ctx.check_buffer_exporter(data)?;
            let result = bio_session(ctx, handle)?
                .borrow_mut()
                .write(ctx.native_buffer_bytes(data)?);
            bio_count(ctx, result)
        }
        ("bio_pending", &[handle]) => {
            let result = bio_session(ctx, handle)?.borrow().pending();
            bio_count(ctx, result)
        }
        ("bio_handshake" | "bio_shutdown", &[handle]) => {
            let session = bio_session(ctx, handle)?;
            let result = if name == "bio_handshake" {
                session.borrow_mut().handshake()
            } else {
                session.borrow_mut().shutdown()
            };
            match result {
                Ok(()) => Ok(NONE),
                Err(error) => bio_failure(ctx, error),
            }
        }
        ("wrap", &[handle, socket, hostname, side]) => {
            let server = ctx.truth(side)?;
            let handle = context_for_role(ctx, handle, server)?;
            let host = if hostname == NONE {
                if handle.borrow().check_hostname {
                    return Err(Error::new(
                        "ValueError",
                        "check_hostname requires server_hostname",
                    ));
                }
                None
            } else {
                if server {
                    return Err(Error::new(
                        "ValueError",
                        "server_hostname can only be specified in client mode",
                    ));
                }
                let host = ctx.str_value(hostname)?;
                if host.is_empty() || host.starts_with('.') || host.contains('\0') {
                    return Err(Error::new("ValueError", "invalid server_hostname"));
                }
                if !host.is_ascii() {
                    return Err(Error::new(
                        "NotImplementedError",
                        "TLS requires an ASCII server_hostname",
                    ));
                }
                Some(host)
            };
            let Value::Socket(socket) = ctx.heap.get(socket) else {
                return Err(Error::new("TypeError", "expected a socket"));
            };
            let (transport, connected) = socket::take_tls_stream(socket, server)?;
            if !connected {
                // Leave the transferred fd owned on every error. The public wrapper turns
                // queued pre-handshake data into the corresponding structured SSLError.
                if socket::pending_tls_data(&transport)? {
                    return Ok(NONE);
                }
                return ctx
                    .heap
                    .alloc_native_drop_observer(Value::TlsListener(transport));
            }
            // Provider calls cannot invoke Python. Release the handle borrow before VM allocation.
            let result = provider::Session::wrap(&handle.borrow(), transport, host, server);
            match result {
                Ok(session) => {
                    let session = Value::TlsSession(Rc::new(RefCell::new(Some(session))));
                    ctx.heap.alloc_native_drop_observer(session)
                }
                Err(error) => bio_failure(ctx, error),
            }
        }
        ("release_session", &[handle]) => match ctx.heap.get(handle) {
            Value::TlsSession(state) => {
                let state = Rc::clone(state);
                let mut tls = state.borrow_mut();
                let Some(session) = tls.as_ref() else {
                    return Ok(NONE);
                };
                // Keep the session intact if registering its returned socket fails.
                let socket = ctx
                    .heap
                    .alloc_descriptor_owner(Value::Socket(session.socket().clone()))?;
                drop(tls.take().unwrap().into_socket());
                Ok(socket)
            }
            Value::Socket(_) | Value::TlsListener(_) | Value::None => Ok(handle),
            _ => Err(Error::new("TypeError", "expected a TLS socket")),
        },
        ("shutdown", &[handle, how]) => {
            let how = ctx.index_c_int(how)?;
            let raw = match ctx.heap.get(handle) {
                Value::Socket(raw) | Value::TlsListener(raw) => raw,
                Value::None => return Err(closed()),
                _ => return Err(Error::new("TypeError", "expected a raw TLS transport")),
            };
            socket::shutdown(raw, how)?;
            Ok(NONE)
        }
        ("is_session", &[handle]) => Ok(super::builtins::boolean(matches!(
            ctx.heap.get(handle),
            Value::TlsSession(s) if s.borrow().is_some()
        ))),
        ("fileno" | "getsockname" | "getpeername" | "listen" | "accept", [handle, rest @ ..])
            if rest.is_empty() || (name == "listen" && rest.len() == 1) =>
        {
            let socket = match ctx.heap.get(*handle) {
                Value::Socket(s) | Value::TlsListener(s) => Some(s.clone()),
                Value::TlsSession(session) => {
                    session.borrow().as_ref().map(|tls| tls.socket().clone())
                }
                Value::None => None,
                _ => return Err(Error::new("TypeError", "expected a TLS socket")),
            };
            let Some(socket) = socket else {
                if let Some(&backlog) = rest.first() {
                    ctx.index_c_int(backlog)?;
                }
                return if name == "fileno" {
                    Ok(ctx.int(-1))
                } else {
                    Err(closed())
                };
            };
            super::socket::endpoint_method(ctx, name, &socket, rest)
        }
        ("hostname_checks_common_name", &[handle]) => Ok(super::builtins::boolean(
            context(ctx, handle)?.borrow().hostname_checks_common_name(),
        )),
        ("set_hostname_checks_common_name", &[handle, enabled]) => {
            let enabled = ctx.truth(enabled)?;
            context(ctx, handle)?
                .borrow_mut()
                .set_hostname_checks_common_name(enabled);
            Ok(NONE)
        }
        ("channel_binding", &[handle, kind]) => {
            let storage = ctx.string_storage(kind).ok_or_else(|| {
                let ty = if kind == NONE {
                    "None"
                } else {
                    ctx.type_name(kind)
                };
                Error::new(
                    "TypeError",
                    format!(
                        "get_channel_binding() argument 'cb_type' must be str, not {}",
                        &ty[..ty.floor_char_boundary(ty.len().min(50))]
                    ),
                )
            })?;
            // Argument Clinic uses the underlying Unicode data for str subclasses.
            let kind = ctx.encode_utf8_codepoints(storage, false, "strict")?;
            if kind.contains(&0) {
                return Err(Error::new("ValueError", "embedded null character"));
            }
            if kind != b"tls-unique" {
                return Err(Error::new(
                    "ValueError",
                    format!(
                        "'{}' channel binding type not implemented",
                        String::from_utf8_lossy(&kind)
                    ),
                ));
            }
            if without_session(ctx, handle) {
                return Ok(NONE);
            }
            let data = session(ctx, handle)?
                .borrow()
                .as_ref()
                .and_then(|ssl| ssl.channel_binding());
            Ok(data.map_or(NONE, |bytes| ctx.alloc(Value::Bytes(bytes))))
        }
        ("get_verified_chain" | "get_unverified_chain", &[handle]) => {
            let closed_chain = || {
                Error::new(
                    "AttributeError",
                    format!("'NoneType' object has no attribute '{name}'"),
                )
            };
            if without_session(ctx, handle) {
                return Err(closed_chain());
            }
            let certificates = session(ctx, handle)?
                .borrow()
                .as_ref()
                .ok_or_else(closed_chain)?
                .certificate_chain(name == "get_verified_chain")
                .map_err(|e| provider_error(ctx, e))?;
            let chain = certificates
                .into_iter()
                .map(|der| ctx.alloc(Value::Bytes(der)))
                .collect();
            Ok(ctx.alloc(Value::List(chain)))
        }
        ("send" | "sendall", [handle, data, flags @ ..])
            if flags.len() == 1 || (flags.is_empty() && name == "sendall") =>
        {
            ctx.check_buffer_exporter(*data)?;
            if let Value::Socket(raw) = ctx.heap.get(*handle) {
                let raw = raw.clone();
                ctx.native_buffer_bytes(*data)?;
                return ctx.with_buffer_export(*data, |ctx| {
                    let flags = flags
                        .first()
                        .map(|&id| ctx.index_c_int(id))
                        .transpose()?
                        .unwrap_or(0);
                    if name == "sendall" {
                        super::socket::send_all_exported(ctx, &raw, *data, flags)?;
                        return Ok(NONE);
                    }
                    let bytes = ctx.native_buffer_bytes(*data)?;
                    Ok(ctx.int(socket::send(&raw, bytes, flags)? as i64))
                });
            }
            let data = ctx.native_buffer_bytes(*data)?;
            if without_session(ctx, *handle) {
                if let Some(&flags) = flags.first() {
                    ctx.index_c_int(flags)?;
                }
                return Err(pending_io_error(ctx, *handle, libc::EPIPE));
            }
            let sent = {
                let mut handle = session(ctx, *handle)?.borrow_mut();
                let session = handle.as_mut().ok_or_else(closed)?;
                if name == "sendall" {
                    session.sendall(data).map_err(|e| provider_error(ctx, e))?;
                    return Ok(NONE);
                }
                session.send(data).map_err(|e| provider_error(ctx, e))?
            };
            Ok(ctx.int(sent as i64))
        }
        ("recv", [handle, size, flags @ ..]) if flags.len() <= 1 => {
            if let Value::Socket(raw) = ctx.heap.get(*handle) {
                let raw = raw.clone();
                let length = ctx.index_c_long(*size)?;
                let flags = flags
                    .first()
                    .map(|&id| ctx.index_c_int(id))
                    .transpose()?
                    .unwrap_or(0);
                let length = usize::try_from(length)
                    .map_err(|_| Error::new("ValueError", "negative buffersize in recv"))?;
                let bytes = socket::recv(&raw, length, flags)?;
                return Ok(ctx.alloc(Value::Bytes(bytes)));
            }
            if !without_session(ctx, *handle) {
                return socket_read(ctx, *handle, *size, NONE);
            }
            let size = usize::try_from(ctx.as_integer(*size)?)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recv"))?;
            if let Some(&flags) = flags.first() {
                ctx.index_c_int(flags)?;
            }
            if size == 0 {
                return Ok(ctx.alloc(Value::Bytes(Vec::new())));
            }
            Err(pending_io_error(ctx, *handle, libc::ENOTCONN))
        }
        ("close", &[handle]) => {
            if let Value::Socket(s) | Value::TlsListener(s) = ctx.heap.get(handle) {
                socket::close(s)?;
            } else {
                session(ctx, handle)?.borrow_mut().take();
            }
            Ok(NONE)
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid positional arguments for {name}()"),
        )),
    }
}
