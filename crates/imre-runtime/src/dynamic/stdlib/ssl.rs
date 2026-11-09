//! Private bindings for the system TLS provider. The Python ssl module owns the public types.

use std::{cell::RefCell, ffi::CString, io, rc::Rc};

use super::{Context, Error, FALSE, Id, NONE, Result, Value};
use crate::objects::{errno_error, socket};

pub(crate) mod provider;
mod sni;
pub(in crate::dynamic) use sni::{Owner, Registration};

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
        "sni_callback",
        "set_context",
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
        "selected_alpn_protocol",
        "cipher",
        "version",
        "post_handshake_auth",
        "verify_client_post_handshake",
        "get_ciphers",
        "set_ciphers",
        "set_ecdh_curve",
        "load_verify_locations",
        "load_dh_params",
        "load_cert_chain",
        "wrap",
        "is_session",
        "check_pending_client",
        "nonzero_flags",
        "do_handshake",
        "release_session",
        "transport_shutdown",
        "read",
        "pending",
        "tls_shutdown",
        "wrap_bio",
        "bio_handshake",
        "bio_server_hostname",
        "bio_read",
        "bio_write",
        "bio_pending",
        "bio_getpeercert",
        "getpeercert",
        "bio_shutdown",
        "channel_binding",
        "hostname_checks_common_name",
        "set_hostname_checks_common_name",
        "get_verified_chain",
        "get_unverified_chain",
        "send",
        "sendall",
        "recv",
    ] {
        let function = ctx.builtin(&format!("_imre_ssl.{name}"));
        ctx.bind(scope, name, function);
    }
    Ok(())
}

fn context(ctx: &Context, id: Id) -> Result<&RefCell<provider::Context>> {
    match ctx.heap.get(id) {
        Value::TlsContext(handle, _) => Ok(handle),
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
        Value::TlsSession(handle, _) => Ok(handle),
        Value::None => Err(closed()),
        _ => Err(Error::new("TypeError", "expected a TLS session")),
    }
}

fn retained_session(ctx: &Context, handle: Id) -> Result<Option<provider::OwnedSsl>> {
    let session = match ctx.heap.get(handle) {
        Value::TlsBioSession { session, .. } => Some(session.borrow().retain_ssl()),
        Value::TlsSession(session, _) => session.borrow().as_ref().map(|ssl| ssl.retain_ssl()),
        Value::TlsTransport(_) | Value::None => None,
        _ => return Err(Error::new("TypeError", "expected a TLS session")),
    };
    session.transpose().map_err(|e| provider_error(ctx, e))
}

fn without_session(ctx: &Context, handle: Id) -> bool {
    matches!(ctx.heap.get(handle), Value::TlsTransport(_) | Value::None)
}

/// Inactive TLS transports can report an error or EOF without reading from the socket.
fn inactive_io(ctx: &Context, handle: Id, pending_errno: i32) -> Result<()> {
    let Value::TlsTransport(endpoint) = ctx.heap.get(handle) else {
        return Err(closed());
    };
    let errno = endpoint.inactive_error(pending_errno)?;
    if errno == 0 {
        Ok(())
    } else {
        Err(errno_error(io::Error::from_raw_os_error(errno)).into())
    }
}

fn inactive_write<T>(ctx: &Context, handle: Id) -> Result<T> {
    inactive_io(ctx, handle, libc::EPIPE)?;
    Err(errno_error(io::Error::from_raw_os_error(libc::EPIPE)).into())
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
        Value::TlsBioSession { session, .. } => Ok(Rc::clone(session)),
        _ => Err(Error::new("TypeError", "expected a TLS memory BIO session")),
    }
}

fn bio_failure(ctx: &mut Context, error: provider::bio::BioFailure) -> Result<Id> {
    use provider::bio::BioFailure;
    let status = match error {
        BioFailure::ClientHello(_) => {
            return Err(Error::new("RuntimeError", "unhandled TLS ClientHello"));
        }
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

/// Finish Python conversion and release its temporaries before reentering the TLS provider.
/// The provider acquires the socket's current registration when the operation resumes.
fn callback_result<T>(
    ctx: &mut Context,
    call: impl FnOnce(&mut Context) -> Result<T>,
) -> Result<T> {
    let mark = ctx.heap.temporary_mark();
    let result = call(ctx);
    ctx.discard_temporaries(
        mark,
        result.as_ref().err().into_iter().flat_map(Error::roots),
    );
    result
}

fn read_plaintext(
    ctx: &mut Context,
    handle: Id,
    size: Id,
    buffer: Id,
    mut read: impl FnMut(&mut [u8]) -> std::result::Result<usize, provider::bio::BioFailure>,
) -> Result<Id> {
    // __index__ may resize the destination or reenter the TLS session.
    let size = match ctx.heap.get(ctx.scalar_value(size)) {
        Value::Int(_) | Value::Bool(_) => ctx.index_integer(size),
        _ => callback_result(ctx, |ctx| ctx.index_integer(size)),
    }?;
    let size = size
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
        return ctx.with_buffer_export(buffer, |ctx| {
            let result = sni::drive(ctx, handle, |ctx| {
                let dest =
                    &mut ctx.writable_storage_bytes(storage)?[range.start..range.start + size];
                Ok(read(dest))
            })?;
            bio_count(ctx, result)
        });
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
    match sni::drive(ctx, handle, |_| Ok(read(&mut data)))? {
        Ok(count) => {
            data.truncate(count);
            crate::compact_bytes(&mut data);
            Ok(ctx.alloc(Value::Bytes(data)))
        }
        Err(error) => bio_failure(ctx, error),
    }
}

fn socket_read(ctx: &mut Context, handle: Id, size: Id, buffer: Id) -> Result<Id> {
    let Value::TlsSession(socket, _) = ctx.heap.get(handle) else {
        return Err(closed());
    };
    let socket = Rc::clone(socket);
    let mut deadline = None;
    read_plaintext(ctx, handle, size, buffer, |data| {
        // The length's __index__ may have closed or unwrapped this socket.
        let mut socket = socket.borrow_mut();
        let session = socket.as_mut().ok_or_else(|| {
            provider::bio::BioFailure::Tls(provider::Failure::Io(io::Error::from_raw_os_error(
                libc::EBADF,
            )))
        })?;
        let until = *deadline.get_or_insert_with(|| socket::deadline(session.socket()));
        session.read(data, until)
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

fn load_ca_data(ctx: &mut Context, handle: Id, input: Id) -> Result<Id> {
    let invalid = || {
        Error::new(
            "TypeError",
            "cadata should be an ASCII string or a bytes-like object",
        )
    };
    let load = |ctx: &Context, bytes: &[u8], pem| {
        if bytes.is_empty() {
            return Err(Error::new("ValueError", "Empty certificate data"));
        }
        if bytes.len() > i32::MAX as usize {
            return Err(Error::new("OverflowError", "Certificate data is too long."));
        }
        let data = if pem {
            provider::CaData::Pem(bytes)
        } else {
            provider::CaData::Der(bytes)
        };
        Ok(context(ctx, handle)?
            .borrow_mut()
            .load_verify_locations(None, None, Some(data)))
    };
    let result = match ctx.heap.get(ctx.scalar_value(input)) {
        Value::Str(text) if text.is_ascii() => load(ctx, text.as_bytes(), true)?,
        Value::Str(_) | Value::SurrogateStr(_) => return Err(invalid()),
        _ => {
            // Only non-exporters receive the cadata-specific TypeError.
            if ctx.buffer_slot(input, "__buffer__").is_none()
                && matches!(ctx.native_buffer_bytes(input), Err(e) if e.kind == "TypeError")
            {
                return Err(invalid());
            }
            ctx.with_read_buffer(input, |ctx, buffer| load(ctx, buffer.bytes(ctx)?, false))?
        }
    };
    trust_result(ctx, result)
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
            Ok(ctx.alloc(Value::TlsContext(
                Rc::new(RefCell::new(handle)),
                sni::Registration::EMPTY,
            )))
        }
        ("sni_callback", [handle, callback @ ..]) if callback.len() <= 1 => {
            sni::callback(ctx, *handle, callback.first().copied())
        }
        ("set_context", &[session, handle, public_context]) => {
            sni::set_context(ctx, session, handle, public_context)?;
            Ok(NONE)
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
        ("cipher" | "version", &[handle]) => {
            let Some(ssl) = retained_session(ctx, handle)? else {
                return Ok(NONE);
            };
            if name == "cipher" {
                Ok(ssl.negotiated_cipher(ctx))
            } else {
                Ok(ssl
                    .negotiated_version()
                    .map_or(NONE, |v| ctx.text(v.to_string_lossy())))
            }
        }
        ("selected_alpn_protocol", &[handle]) => {
            let selected = match ctx.heap.get(handle) {
                Value::TlsBioSession { session, .. } => session.borrow().selected_alpn_protocol(),
                Value::TlsSession(session, _) => session
                    .borrow()
                    .as_ref()
                    .and_then(|s| s.selected_alpn_protocol()),
                Value::None | Value::TlsTransport(_) | Value::Socket(_) => None,
                _ => return Err(Error::new("TypeError", "expected a TLS connection")),
            };
            let Some(bytes) = selected else {
                return Ok(NONE);
            };
            let name = crate::objects::Codec::Utf8 { signature: false }
                .decode(&bytes, None)
                .map_err(|err| ctx.decode_error(err))?;
            Ok(ctx.text(name))
        }
        ("set_alpn_protocols", &[handle, wire]) => {
            let Value::Bytes(protocols) = ctx.heap.get(ctx.scalar_value(wire)) else {
                return Err(Error::new("TypeError", "expected ALPN wire bytes"));
            };
            context(ctx, handle)?
                .borrow_mut()
                .set_alpn_protocols(protocols)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("post_handshake_auth", &[handle]) => Ok(super::builtins::boolean(
            context(ctx, handle)?.borrow().post_handshake_auth,
        )),
        ("post_handshake_auth", &[handle, value]) => {
            let enabled = ctx.truth(value)?;
            context(ctx, handle)?.borrow_mut().post_handshake_auth = enabled;
            Ok(NONE)
        }
        ("verify_client_post_handshake", &[handle]) => {
            let result = match ctx.heap.get(handle) {
                Value::TlsBioSession { session, .. } => {
                    session.borrow().verify_client_post_handshake()
                }
                _ => session(ctx, handle)?
                    .borrow()
                    .as_ref()
                    .ok_or_else(closed)?
                    .verify_client_post_handshake(),
            };
            trust_result(ctx, result)
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
            if cadata != NONE {
                // Release the DER export after installing its certificates and before opening paths.
                let result = load_ca_data(ctx, handle, cadata)?;
                if result != NONE || (cafile.is_none() && capath.is_none()) {
                    return Ok(result);
                }
            }
            let result = context(ctx, handle)?.borrow_mut().load_verify_locations(
                cafile.as_deref(),
                capath.as_deref(),
                None,
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
        (
            "wrap_bio",
            &[
                handle,
                incoming,
                outgoing,
                hostname,
                side,
                public_context,
                weak_owner,
            ],
        ) => {
            let server = ctx.truth(side)?;
            let owner = Owner::new(handle, public_context, weak_owner);
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
            Ok(ctx.alloc(Value::TlsBioSession {
                session: Rc::new(RefCell::new(result)),
                server_hostname: hostname,
                owner,
            }))
        }
        ("bio_server_hostname", &[handle]) => match ctx.heap.get(handle) {
            Value::TlsBioSession {
                server_hostname, ..
            } => Ok(*server_hostname),
            _ => Err(Error::new("TypeError", "expected a TLS memory BIO session")),
        },
        ("getpeercert" | "bio_getpeercert", &[handle, binary]) => {
            // The bound SSL must survive __bool__, which can handshake or close the Python peer.
            let ssl = if name == "bio_getpeercert" {
                bio_session(ctx, handle)?.borrow().retain_ssl()
            } else {
                let Value::TlsSession(state, _) = ctx.heap.get(handle) else {
                    return Err(Error::new(
                        "AttributeError",
                        "'NoneType' object has no attribute 'getpeercert'",
                    ));
                };
                state.borrow().as_ref().ok_or_else(closed)?.retain_ssl()
            }
            .map_err(|e| provider_error(ctx, e))?;
            let binary = ctx.truth(binary)?;
            ssl.peer_certificate(ctx, binary)
        }
        ("bio_read", &[handle, size, buffer]) => {
            let session = bio_session(ctx, handle)?;
            read_plaintext(ctx, handle, size, buffer, |data| {
                session.borrow_mut().read(data)
            })
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
        ("tls_shutdown" | "do_handshake", &[handle]) => {
            let result = sni::drive(ctx, handle, |ctx| {
                let mut state = session(ctx, handle)?.borrow_mut();
                let session = state.as_mut().ok_or_else(closed)?;
                Ok(if name == "do_handshake" {
                    session.handshake()
                } else {
                    session.shutdown()
                })
            })?;
            match result {
                Ok(()) => Ok(NONE),
                Err(error) => bio_failure(ctx, error),
            }
        }
        ("bio_write", &[handle, data]) => {
            ctx.check_buffer_exporter(data)?;
            let session = bio_session(ctx, handle)?;
            ctx.with_buffer_export(data, |ctx| {
                let result = sni::drive(ctx, handle, |ctx| {
                    Ok(session.borrow_mut().write(ctx.native_buffer_bytes(data)?))
                })?;
                bio_count(ctx, result)
            })
        }
        ("bio_pending", &[handle]) => {
            let result = bio_session(ctx, handle)?.borrow().pending();
            bio_count(ctx, result)
        }
        ("bio_handshake" | "bio_shutdown", &[handle]) => {
            let session = bio_session(ctx, handle)?;
            let result = if name == "bio_shutdown" {
                session.borrow_mut().shutdown()
            } else {
                sni::drive(ctx, handle, |_| Ok(session.borrow_mut().handshake()))?
            };
            match result {
                Ok(()) => Ok(NONE),
                Err(error) => bio_failure(ctx, error),
            }
        }
        (
            "wrap",
            [
                handle,
                raw,
                hostname,
                side,
                with_session,
                public_context,
                weak_owner,
            ],
        ) => {
            let owner = Owner::new(*handle, *public_context, *weak_owner);
            let with_session = ctx.truth(*with_session)?;
            let server = ctx.truth(*side)?;
            let configuration = context_for_role(ctx, *handle, server)?;
            let hostname = if *hostname == NONE {
                if configuration.borrow().check_hostname {
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
                let host = ctx.str_value(*hostname)?;
                if host.is_empty() || host.starts_with('.') || host.contains('\0') {
                    return Err(Error::new("ValueError", "invalid server_hostname"));
                }
                if !host.is_ascii() {
                    return Err(Error::new(
                        "NotImplementedError",
                        "TLS requires an ASCII server_hostname",
                    ));
                }
                Some(*hostname)
            };
            let Some(raw) = ctx.heap.get(*raw).socket_handle() else {
                return Err(Error::new("TypeError", "expected a socket"));
            };
            let raw = raw.clone();
            socket::check_tls_mode(&raw)?;
            if !with_session {
                return ctx
                    .heap
                    .alloc_native_drop_observer(Value::TlsTransport(Box::new(
                        socket::TlsEndpoint::new(raw, !server)
                            .map_err(crate::objects::errno_error)?,
                    )));
            }
            let host = hostname.map(|id| ctx.str_value(id)).transpose()?;
            let result =
                provider::Session::wrap(&context(ctx, *handle)?.borrow(), raw, host, server);
            match result {
                Ok(session) => ctx.heap.alloc_native_drop_observer(Value::TlsSession(
                    Rc::new(RefCell::new(Some(session))),
                    owner,
                )),
                Err(error) => bio_failure(ctx, error),
            }
        }
        ("check_pending_client", &[handle]) => {
            if let Value::TlsTransport(endpoint) = ctx.heap.get(handle) {
                endpoint.check_pending_client()?;
            }
            Ok(NONE)
        }
        ("nonzero_flags", &[flags]) => {
            if matches!(ctx.heap.get(flags), Value::Int(_) | Value::Bool(_)) {
                return Ok(super::builtins::boolean(ctx.truth(flags)?));
            }
            let nonzero = callback_result(ctx, |ctx| {
                let zero = ctx.int(0);
                let compared = ctx.binary("!=", flags, zero)?;
                ctx.truth(compared)
            })?;
            Ok(super::builtins::boolean(nonzero))
        }
        ("is_session", &[handle]) => Ok(super::builtins::boolean(matches!(
            ctx.heap.get(handle),
            Value::TlsSession(s, _) if s.borrow().is_some()
        ))),
        ("transport_shutdown", &[handle, how]) => {
            let how = super::socket::integer(ctx, how)?;
            match ctx.heap.get_mut(handle) {
                Value::TlsTransport(endpoint) => endpoint.shutdown(how)?,
                Value::TlsSession(state, _) => {
                    let mut state = state.borrow_mut();
                    let live = state.as_mut().ok_or_else(closed)?;
                    live.endpoint.shutdown(how)?;
                    if live.endpoint.fully_shut() {
                        // A suspended read may also hold this state. Retire its SSL before
                        // keeping just the guarded transport for error, EOF and final close.
                        let endpoint = live.endpoint.clone();
                        state.take();
                        drop(state);
                        *ctx.heap.get_mut(handle) = Value::TlsTransport(Box::new(endpoint));
                    }
                }
                _ => return Err(closed()),
            }
            Ok(NONE)
        }
        ("release_session", &[handle]) => match ctx.heap.get(handle) {
            Value::TlsSession(state, _) => {
                // An in-flight OpenSSL operation still needs its callback result. The
                // Python socket owns the fd and may close it before that operation resumes.
                let mut state = state.borrow_mut();
                if !state.as_ref().is_some_and(provider::Session::sni_suspended) {
                    state.take();
                }
                Ok(NONE)
            }
            Value::TlsTransport(_) | Value::None => Ok(NONE),
            _ => Err(Error::new("TypeError", "expected a TLS socket")),
        },
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
            let data = retained_session(ctx, handle)?.and_then(|ssl| ssl.channel_binding());
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
            ctx.native_buffer_bytes(*data)?;
            if without_session(ctx, *handle) {
                if let Some(&flags) = flags.first() {
                    ctx.index_c_int(flags)?;
                }
                return inactive_write(ctx, *handle);
            }
            ctx.with_buffer_export(*data, |ctx| {
                let result = sni::drive(ctx, *handle, |ctx| {
                    let mut state = session(ctx, *handle)?.borrow_mut();
                    let session = state.as_mut().ok_or_else(closed)?;
                    let data = ctx.native_buffer_bytes(*data)?;
                    Ok(if name == "sendall" {
                        session.sendall(data).map(|()| 0)
                    } else {
                        session.send(data)
                    })
                })?;
                if result.is_ok() && name == "sendall" {
                    return Ok(NONE);
                }
                bio_count(ctx, result)
            })
        }
        ("recv", [handle, size, args @ ..]) if args.len() <= 2 => {
            let buffer = args.get(1).copied();
            if !without_session(ctx, *handle) {
                return socket_read(ctx, *handle, *size, buffer.unwrap_or(NONE));
            }
            let receive = |ctx: &mut Context, capacity: Option<usize>| -> Result<()> {
                let amount = super::socket::receive_size(ctx, *size)?;
                let flags = args
                    .first()
                    .map(|&id| ctx.index_c_int(id))
                    .transpose()?
                    .unwrap_or(0);
                let method = if capacity.is_some() {
                    "recv_into"
                } else {
                    "recv"
                };
                let amount = usize::try_from(amount).map_err(|_| {
                    Error::new("ValueError", format!("negative buffersize in {method}"))
                })?;
                if capacity.is_some_and(|cap| amount > cap) {
                    return Err(Error::new(
                        "ValueError",
                        "buffer too small for requested bytes",
                    ));
                }
                if amount != 0 || capacity.is_some_and(|cap| cap != 0) {
                    match ctx.heap.get(*handle) {
                        Value::TlsTransport(endpoint) if endpoint.fully_shut() => {
                            let endpoint = endpoint.as_ref().clone();
                            endpoint.read_eof(flags, || ctx.dispatch_signals())?;
                        }
                        _ => inactive_io(ctx, *handle, libc::ENOTCONN)?,
                    }
                }
                Ok(())
            };
            if let Some(buffer) = buffer {
                // A pending listener keeps SSLSocket.recv_into's existing flags and read errors.
                if !matches!(ctx.heap.get(*handle), Value::TlsTransport(e) if e.fully_shut()) {
                    return Ok(NONE);
                }
                ctx.with_writable_byte_span(buffer, "recv_into() argument 1", |ctx, _, span| {
                    receive(ctx, Some(span.len()))?;
                    Ok(ctx.int(0))
                })
            } else {
                receive(ctx, None)?;
                Ok(ctx.alloc(Value::Bytes(Vec::new())))
            }
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid positional arguments for {name}()"),
        )),
    }
}
