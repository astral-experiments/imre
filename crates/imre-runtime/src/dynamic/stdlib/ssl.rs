//! Private bindings for the system TLS provider. The Python ssl module owns the public types.

use std::{cell::RefCell, ffi::CString, io, rc::Rc};

use super::{Context, Error, Id, NONE, Result, Value};
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
        "security_level",
        "verify_flags",
        "default_verify_paths",
        "default_verify_flags",
        "set_alpn_protocols",
        "set_post_handshake_auth",
        "set_ciphers",
        "load_verify_locations",
        "wrap",
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

fn session(ctx: &Context, id: Id) -> Result<&RefCell<Option<provider::Session>>> {
    match ctx.heap.get(id) {
        Value::TlsSession(handle) => Ok(handle),
        _ => Err(Error::new("TypeError", "expected a TLS session")),
    }
}

fn closed() -> Error {
    errno_error(io::Error::from_raw_os_error(libc::EBADF)).into()
}

fn provider_error(ctx: &Context, failure: provider::Failure) -> Error {
    let message = match failure {
        provider::Failure::Io(error) => return errno_error(error).into(),
        provider::Failure::Verify { message, .. } | provider::Failure::Other(message) => message,
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
        ("context", []) => {
            let handle = provider::Context::client().map_err(|e| provider_error(ctx, e))?;
            Ok(ctx.alloc(Value::TlsContext(Rc::new(RefCell::new(handle)))))
        }
        ("security_level", &[handle]) => {
            let level = context(ctx, handle)?.borrow().security_level();
            Ok(ctx.int(i64::from(level)))
        }
        ("verify_flags", &[handle]) => {
            let flags = context(ctx, handle)?.borrow().verify_flags();
            Ok(ctx.alloc(Value::Int(crate::Int::new(flags.into()))))
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
            context(ctx, handle)?
                .borrow_mut()
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
            context(ctx, handle)?
                .borrow_mut()
                .set_post_handshake_auth(enabled);
            Ok(NONE)
        }
        ("load_verify_locations", &[handle, cafile]) => {
            let path = super::posixpath::fspath(ctx, cafile)?;
            let path = super::os::c_path(ctx, path, "embedded null byte in cafile")?;
            context(ctx, handle)?
                .borrow_mut()
                .load_verify_locations(&path)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
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
        ("wrap", &[handle, socket, hostname]) => {
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
            let handle = context(ctx, handle)?;
            let Value::Socket(socket) = ctx.heap.get(socket) else {
                return Err(Error::new("TypeError", "expected a socket"));
            };
            let fd = socket::take_tls_stream(socket)?;
            // Provider calls cannot invoke Python. Release the handle borrow before VM allocation.
            let result = provider::Session::connect(&handle.borrow(), fd, host);
            match result {
                Ok(session) => {
                    Ok(ctx.alloc(Value::TlsSession(Rc::new(RefCell::new(Some(session))))))
                }
                Err(provider::Failure::Verify { code, message }) => {
                    let code = ctx.int(code);
                    let message = ctx.text(message);
                    Ok(ctx.alloc(Value::Tuple(vec![code, message])))
                }
                Err(e) => Err(provider_error(ctx, e)),
            }
        }
        ("sendall", &[handle, data]) => {
            let data = ctx.native_buffer_bytes(data)?;
            session(ctx, handle)?
                .borrow_mut()
                .as_mut()
                .ok_or_else(closed)?
                .sendall(data)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(NONE)
        }
        ("recv", &[handle, size]) => {
            let size = usize::try_from(ctx.as_integer(size)?)
                .map_err(|_| Error::new("ValueError", "negative buffersize in recv"))?;
            let data = session(ctx, handle)?
                .borrow_mut()
                .as_mut()
                .ok_or_else(closed)?
                .recv(size)
                .map_err(|e| provider_error(ctx, e))?;
            Ok(ctx.alloc(Value::Bytes(data)))
        }
        ("close", &[handle]) => {
            session(ctx, handle)?.borrow_mut().take();
            Ok(NONE)
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid positional arguments for {name}()"),
        )),
    }
}
