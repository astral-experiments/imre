//! OpenSSL's in-memory byte transport, owned by one Python MemoryBIO.
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::rc::Rc;

use super::{Api, Failure};
use crate::dynamic::{
    Context, builtins, stdlib,
    value::{Error, Id, NONE, Result, Value},
};

pub(in crate::dynamic) const CLASS: &str = "_ssl.MemoryBIO";

// OpenSSL bio.h: BIO_set_mem_eof_return, BIO_set_retry_read and BIO_clear_retry_flags.
const SET_MEM_EOF_RETURN: i32 = 130;
const RETRY_READ: i32 = 0x09;
const RETRY_FLAGS: i32 = 0x0f;

pub(in crate::dynamic) struct Bio {
    api: &'static Api,
    handle: NonNull<c_void>,
    eof_written: bool,
}

impl Drop for Bio {
    fn drop(&mut self) {
        // SAFETY: release the Python MemoryBIO's reference; attached SSL objects own theirs.
        unsafe { (self.api.BIO_free)(self.handle.as_ptr()) };
    }
}

impl Bio {
    fn new() -> Result<Self> {
        let api = super::api()
            .ok_or_else(|| Error::new("ImportError", "system TLS provider unavailable"))?;
        // SAFETY: the memory method and its entry points are from the same live library.
        let handle = NonNull::new(unsafe { (api.BIO_new)((api.BIO_s_mem)()) })
            .ok_or_else(|| Error::new("MemoryError", "failed to allocate BIO"))?;
        // SAFETY: this is a fresh memory BIO. Like CPython, an empty BIO retries until marked EOF.
        unsafe {
            (api.BIO_set_flags)(handle.as_ptr(), RETRY_READ);
            (api.BIO_ctrl)(handle.as_ptr(), SET_MEM_EOF_RETURN, -1, ptr::null_mut());
        }
        Ok(Self {
            api,
            handle,
            eof_written: false,
        })
    }

    fn pending(&self) -> usize {
        // SAFETY: the BIO is live for the duration of the call.
        unsafe { (self.api.BIO_ctrl_pending)(self.handle.as_ptr()) }
    }

    fn mark_eof(&mut self) {
        // SAFETY: both operations only change the live memory BIO's retry behavior.
        unsafe {
            (self.api.BIO_clear_flags)(self.handle.as_ptr(), RETRY_FLAGS);
            (self.api.BIO_ctrl)(self.handle.as_ptr(), SET_MEM_EOF_RETURN, 0, ptr::null_mut());
        }
        self.eof_written = true;
    }
}

pub(in crate::dynamic) struct BioSession {
    api: &'static Api,
    handle: NonNull<c_void>,
    handshake_complete: bool,
    server: bool,
}

/// An SSL I/O failure, including the direction needed to retry an incomplete operation.
pub(in crate::dynamic) enum BioFailure {
    WantRead,
    WantWrite,
    ZeroReturn,
    Eof,
    UnexpectedEof,
    ProtocolIsShutdown,
    ShutdownInInit,
    ProtocolError {
        reason: Option<&'static str>,
        message: String,
    },
    Tls(Failure),
}

impl From<Failure> for BioFailure {
    fn from(error: Failure) -> Self {
        Self::Tls(error)
    }
}

impl Api {
    /// Classify the immediately preceding operation, before any other OpenSSL call.
    fn ssl_failure(&self, ssl: NonNull<c_void>, rc: i32, operation: &str) -> BioFailure {
        let os_error = std::io::Error::last_os_error();
        // SAFETY: SSL_get_error examines the same thread and live SSL as the failed call.
        let kind = unsafe { (self.SSL_get_error)(ssl.as_ptr(), rc) };
        if kind == super::SSL_ERROR_SYSCALL && os_error.raw_os_error() != Some(0) {
            return BioFailure::Tls(Failure::Io(os_error));
        }
        self.ssl_failure_kind(kind, operation)
    }

    /// Interpret SSL_get_error after handling syscall errno.
    pub(super) fn ssl_failure_kind(&self, kind: i32, operation: &str) -> BioFailure {
        match kind {
            super::SSL_ERROR_WANT_READ => BioFailure::WantRead,
            super::SSL_ERROR_WANT_WRITE => BioFailure::WantWrite,
            super::SSL_ERROR_ZERO_RETURN => BioFailure::ZeroReturn,
            // SAFETY: inspect the current operation's error queue without removing entries.
            super::SSL_ERROR_SYSCALL if unsafe { (self.ERR_peek_error)() } == 0 => BioFailure::Eof,
            super::SSL_ERROR_SSL => {
                // OpenSSL 3 ERR_PACK(ERR_LIB_SSL, 0, SSL_R_*).
                const SHUTDOWN_IN_INIT: std::ffi::c_ulong = (20 << 23) | 407;
                const PROTOCOL_IS_SHUTDOWN: std::ffi::c_ulong = (20 << 23) | 207;
                // SAFETY: this thread's last error belongs to the failed SSL operation.
                let code = unsafe { (self.ERR_peek_last_error)() };
                match code {
                    super::UNEXPECTED_EOF => return BioFailure::UnexpectedEof,
                    PROTOCOL_IS_SHUTDOWN => return BioFailure::ProtocolIsShutdown,
                    SHUTDOWN_IN_INIT => return BioFailure::ShutdownInInit,
                    _ => (),
                }
                // Only ERR_LIB_SSL (20) indexes the SSL reason table.
                if code >> 23 != 20 {
                    return BioFailure::Tls(self.error(operation));
                }
                BioFailure::ProtocolError {
                    reason: super::reasons::reason_name((code & 0x7f_ffff) as u32),
                    message: self
                        .reason_text(code)
                        .map_or_else(|| operation.to_owned(), |s| s.into_owned()),
                }
            }
            _ => BioFailure::Tls(self.error(operation)),
        }
    }

    pub(super) fn ssl_pending(
        &self,
        ssl: NonNull<c_void>,
    ) -> std::result::Result<usize, BioFailure> {
        self.clear();
        super::clear_errno();
        // SAFETY: SSL_pending inspects decrypted bytes buffered by this SSL.
        let count = unsafe { (self.SSL_pending)(ssl.as_ptr()) };
        usize::try_from(count).map_err(|_| self.ssl_failure(ssl, count, "SSL_pending"))
    }

    pub(super) fn finish_read(
        &self,
        ssl: NonNull<c_void>,
        result: std::result::Result<usize, BioFailure>,
    ) -> std::result::Result<usize, BioFailure> {
        // CPython returns an empty read for a peer's close_notify. Once both
        // parties have sent close_notify, the same result is SSLZeroReturnError.
        const SSL_RECEIVED_SHUTDOWN: i32 = 2;
        if matches!(result, Err(BioFailure::ZeroReturn))
            // SAFETY: read the shutdown flags on the same SSL as this operation.
            && unsafe { (self.SSL_get_shutdown)(ssl.as_ptr()) } == SSL_RECEIVED_SHUTDOWN
        {
            return Ok(0);
        }
        result
    }
}

impl BioSession {
    pub(in crate::dynamic) fn wrap(
        ctx: &super::Context,
        incoming: &Bio,
        outgoing: &Bio,
        host: Option<&str>,
        server: bool,
    ) -> std::result::Result<Self, Failure> {
        let api = ctx.api;
        api.clear();
        // SAFETY: SSL_new acquires a reference to the live context.
        let handle = NonNull::new(unsafe { (api.SSL_new)(ctx.handle.as_ptr()) })
            .ok_or_else(|| api.error("SSL_new"))?;
        let session = Self {
            api,
            handle,
            handshake_complete: false,
            server,
        };
        let ssl = handle.as_ptr();
        if let Some(host) = host {
            api.set_peer_name(ssl, host, ctx.check_hostname)?;
        }
        // SAFETY: each set0 operation consumes exactly one newly acquired reference,
        // including when incoming and outgoing are the same BIO. SSL_free releases
        // already-installed references on failures and on normal destruction.
        unsafe {
            if (api.BIO_up_ref)(incoming.handle.as_ptr()) != 1 {
                return Err(api.error("BIO_up_ref"));
            }
            (api.SSL_set0_rbio)(ssl, incoming.handle.as_ptr());
            if (api.BIO_up_ref)(outgoing.handle.as_ptr()) != 1 {
                return Err(api.error("BIO_up_ref"));
            }
            (api.SSL_set0_wbio)(ssl, outgoing.handle.as_ptr());
            // Match CPython's SSL_set_mode: a write retry may use an equivalent
            // Python buffer at a different address, and reads process control records.
            const SSL_CTRL_MODE: i32 = 33;
            const ACCEPT_MOVING_WRITE_BUFFER: std::ffi::c_long = 2;
            const AUTO_RETRY: std::ffi::c_long = 4;
            (api.SSL_ctrl)(
                ssl,
                SSL_CTRL_MODE,
                ACCEPT_MOVING_WRITE_BUFFER | AUTO_RETRY,
                ptr::null_mut(),
            );
            if server {
                (api.SSL_set_accept_state)(ssl);
            } else {
                (api.SSL_set_connect_state)(ssl);
            }
        }
        Ok(session)
    }

    pub(in crate::dynamic) fn handshake(&mut self) -> std::result::Result<(), BioFailure> {
        self.handshake_complete = false;
        self.api.clear();
        super::clear_errno();
        // SAFETY: the SSL and both of its memory BIOs remain live across every retry.
        let rc = unsafe { (self.api.SSL_do_handshake)(self.handle.as_ptr()) };
        let result = if rc == 1 {
            Ok(())
        } else {
            Err(self.api.ssl_failure(self.handle, rc, "SSL_do_handshake"))
        };
        if !matches!(result, Err(BioFailure::WantRead | BioFailure::WantWrite)) {
            self.api
                .verify_peer(self.handle.as_ptr(), !self.server && result.is_ok())
                .map_err(BioFailure::Tls)?;
        }
        self.handshake_complete = result.is_ok();
        result
    }

    fn transfer(
        &mut self,
        label: &str,
        operation: impl FnOnce(&Api, super::Handle, &mut usize) -> i32,
    ) -> std::result::Result<usize, BioFailure> {
        // Reads and writes can initiate a handshake. Apply the same peer checks
        // as do_handshake before OpenSSL releases or sends application bytes.
        if !self.handshake_complete {
            self.handshake()?;
        }
        self.api.clear();
        super::clear_errno();
        let mut count = 0;
        let rc = operation(self.api, self.handle.as_ptr(), &mut count);
        if rc == 1 {
            Ok(count)
        } else {
            Err(self.api.ssl_failure(self.handle, rc, label))
        }
    }

    pub(in crate::dynamic) fn read(
        &mut self,
        data: &mut [u8],
    ) -> std::result::Result<usize, BioFailure> {
        if data.is_empty() {
            return Ok(0);
        }
        let result = self.transfer("SSL_read", |api, ssl, count| {
            // SAFETY: the SSL is live and data has space for exactly data.len() bytes.
            unsafe { (api.SSL_read_ex)(ssl, data.as_mut_ptr().cast(), data.len(), count) }
        });
        self.api.finish_read(self.handle, result)
    }

    pub(in crate::dynamic) fn write(
        &mut self,
        data: &[u8],
    ) -> std::result::Result<usize, BioFailure> {
        self.transfer("SSL_write", |api, ssl, count| {
            // SAFETY: the SSL and the input slice remain live for this operation.
            unsafe { (api.SSL_write_ex)(ssl, data.as_ptr().cast(), data.len(), count) }
        })
    }

    pub(in crate::dynamic) fn pending(&self) -> std::result::Result<usize, BioFailure> {
        self.api.ssl_pending(self.handle)
    }

    pub(in crate::dynamic) fn peer_certificate(
        &self,
        ctx: &mut Context,
        binary: bool,
    ) -> Result<Id> {
        let ssl = self.handle.as_ptr();
        let api = self.api;
        // SAFETY: this BioSession owns the SSL throughout the query.
        if unsafe { (api.SSL_is_init_finished)(ssl) } == 0 {
            return Err(Error::new("ValueError", "handshake not done yet"));
        }
        // SAFETY: the peer certificate is borrowed from the live SSL.
        let cert = unsafe { (api.SSL_get0_peer_certificate)(ssl) };
        if cert.is_null() {
            return Ok(NONE);
        }
        if binary {
            let der = api
                .encode_certificate(cert)
                .map_err(|e| super::super::provider_error(ctx, e))?;
            return Ok(ctx.alloc(Value::Bytes(der)));
        }
        // SAFETY: SSL owns a reference to its context. CPython uses the context's
        // current verify mode when choosing whether to return the decoded fields.
        let mode = unsafe { (api.SSL_CTX_get_verify_mode)((api.SSL_get_SSL_CTX)(ssl)) };
        if mode & super::SSL_VERIFY_PEER == 0 {
            return Ok(ctx.alloc(Value::Dict(Default::default())));
        }
        super::certificate::decode(ctx, api, cert)
    }

    pub(in crate::dynamic) fn shutdown(&mut self) -> std::result::Result<(), BioFailure> {
        let ssl = self.handle.as_ptr();
        self.api.clear();
        super::clear_errno();
        // SAFETY: SSL_shutdown operates on this live SSL with nonblocking memory BIOs.
        let mut rc = unsafe { (self.api.SSL_shutdown)(ssl) };
        if rc == 0 {
            // We sent close_notify. Try receiving the peer's alert without reading
            // past it; the peer's ciphertext might already be queued in our input.
            self.api.clear();
            // SAFETY: read-ahead is a setting on this owned SSL.
            unsafe { (self.api.SSL_set_read_ahead)(ssl, 0) };
            super::clear_errno();
            // SAFETY: retry the same shutdown on the same live SSL.
            rc = unsafe { (self.api.SSL_shutdown)(ssl) };
        }
        match rc {
            1 => Ok(()),
            0 => Err(BioFailure::WantRead),
            _ => Err(self.api.ssl_failure(self.handle, rc, "SSL_shutdown")),
        }
    }
}

impl Drop for BioSession {
    fn drop(&mut self) {
        // SAFETY: releasing the SSL drops the references taken to its context and BIOs.
        unsafe { (self.api.SSL_free)(self.handle.as_ptr()) };
    }
}

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("_ssl", Some(scope));
    let module_name = ctx.text("_ssl");
    let qualname = ctx.text("MemoryBIO");
    ctx.bind(fields, "__module__", module_name);
    ctx.bind(fields, "__qualname__", qualname);
    ctx.bind(fields, "__doc__", NONE);
    let object = ctx.builtin("object");
    let owner = ctx.allocate_class("MemoryBIO".into(), fields, vec![object])?;
    let Value::Class { heap_type, .. } = ctx.heap.get_mut(owner) else {
        unreachable!()
    };
    *heap_type = Some(CLASS);
    let Value::Scope(frame) = ctx.heap.get_mut(fields) else {
        unreachable!()
    };
    let namespace = frame.class_namespace.as_mut().unwrap();
    namespace.slot_names = Some([].into());
    namespace.immutable_type = true;
    namespace.defining_module = Some(module);
    for name in ["__new__", "read", "write", "write_eof", "pending", "eof"] {
        let method = ctx.alloc(Value::HeapTypeMethod { owner, name });
        let member = match name {
            "__new__" => ctx.alloc(Value::Method {
                callable: method,
                receiver: owner,
                explicit: false,
            }),
            "pending" | "eof" => ctx.alloc(Value::GetSetDescriptor {
                owner: CLASS,
                name,
                get: method,
                set: None,
            }),
            _ => method,
        };
        ctx.bind(fields, name, member);
    }
    ctx.bind(scope, "MemoryBIO", owner);
    Ok(())
}

fn ssl_error(ctx: &Context, owner: Id, message: &str) -> Error {
    let Value::Class { scope, .. } = ctx.heap.get(owner) else {
        unreachable!()
    };
    let module = ctx
        .scope_data(*scope)
        .class_namespace
        .as_ref()
        .and_then(|namespace| namespace.defining_module)
        .expect("MemoryBIO retains its defining extension module");
    super::objects::ssl_error(ctx, module, message)
}

pub(in crate::dynamic) fn construct(
    ctx: &mut Context,
    owner: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            "MemoryBIO() takes no positional arguments",
        ));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "MemoryBIO() takes no keyword arguments",
        ));
    }
    let state = ctx.alloc(Value::MemoryBio(Rc::new(RefCell::new(Bio::new()?))));
    let instance = ctx.instance(owner, None);
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(instance) else {
        unreachable!()
    };
    *scalar = Some(state);
    Ok(instance)
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method MemoryBIO.{name}() needs an argument"),
        ));
    };
    if name == "__new__" {
        let (&ty, rest) = rest.split_first().ok_or_else(|| {
            Error::new(
                "TypeError",
                "_ssl.MemoryBIO.__new__(): not enough arguments",
            )
        })?;
        if ty != owner {
            return Err(ctx.invalid_new_subtype(CLASS, ty));
        }
        return construct(ctx, owner, rest, kwargs);
    }
    ctx.check_heap_method_receiver(owner, name, receiver)?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("MemoryBIO.{name}() takes no keyword arguments"),
        ));
    }
    let Value::MemoryBio(state) = ctx.heap.get(ctx.scalar_value(receiver)) else {
        return Err(Error::new("TypeError", "expected an initialized MemoryBIO"));
    };
    let state = Rc::clone(state);
    match name {
        "pending" => Ok(ctx.int(state.borrow().pending() as i64)),
        "eof" => {
            let bio = state.borrow();
            Ok(builtins::boolean(bio.eof_written && bio.pending() == 0))
        }
        "write_eof" => {
            if !rest.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "MemoryBIO.write_eof() takes no arguments ({} given)",
                        rest.len()
                    ),
                ));
            }
            state.borrow_mut().mark_eof();
            Ok(NONE)
        }
        "read" => {
            let len = match rest {
                [] => -1,
                [len] => stdlib::index_c_int(ctx, *len)?,
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("read expected at most 1 argument, got {}", rest.len()),
                    ));
                }
            };
            // __index__ can call into this same BIO. Inspect its state only after conversion.
            let bio = state.borrow_mut();
            let limit = if len < 0 { i32::MAX } else { len };
            let size = bio.pending().min(limit as usize);
            let mut data = Vec::new();
            data.try_reserve_exact(size)
                .map_err(|_| Error::new("MemoryError", ""))?;
            data.resize(size, 0);
            if size != 0 {
                // SAFETY: data owns size writable bytes, and size fits a C int.
                let read = unsafe {
                    (bio.api.BIO_read)(bio.handle.as_ptr(), data.as_mut_ptr().cast(), size as i32)
                };
                if read < 0 {
                    return Err(ssl_error(ctx, owner, "BIO_read failed"));
                }
                data.truncate(read as usize);
            }
            Ok(ctx.alloc(Value::Bytes(data)))
        }
        "write" => {
            let input = builtins::one_positional_arg("MemoryBIO.write", rest, kwargs)?;
            ctx.check_buffer_exporter(input)?;
            let data = ctx.native_buffer_bytes(input)?;
            let len = i32::try_from(data.len())
                .map_err(|_| Error::new("OverflowError", "string longer than 2147483647 bytes"))?;
            let bio = state.borrow_mut();
            if bio.eof_written {
                return Err(ssl_error(ctx, owner, "cannot write() after write_eof()"));
            }
            // SAFETY: data is a contiguous buffer rooted by the call, and BIO_write copies it.
            let written =
                unsafe { (bio.api.BIO_write)(bio.handle.as_ptr(), data.as_ptr().cast(), len) };
            if written < 0 {
                return Err(ssl_error(ctx, owner, "BIO_write failed"));
            }
            Ok(ctx.int(i64::from(written)))
        }
        _ => unreachable!("registered MemoryBIO method"),
    }
}
