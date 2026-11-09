//! Pause the provider's servername callback before it commits a context or sends an alert.
use std::cell::Cell;
use std::ffi::{CStr, c_int, c_long, c_void};
use std::ptr::{self, NonNull};
use std::rc::Rc;

use super::{Api, Context, ContextCallbacks, Failure, Handle, bio::BioFailure};

pub(super) type Callback = unsafe extern "C" fn(Handle, *mut c_int, Handle) -> c_int;
const INTERNAL_ERROR: c_int = 80;
const SSL_MODE_ASYNC: c_long = 0x100;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Entry {
    Handshake,
    Peek,
}

impl Entry {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Handshake => "SSL_do_handshake",
            Self::Peek => "SSL_peek",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Off,
    Armed,
    Waiting,
    Resolved(Option<c_int>),
}

struct Exchange {
    phase: Cell<Phase>,
    entry: Cell<Entry>,
    // OpenSSL may retain the destination while SSL_peek is suspended.
    peek_byte: Cell<u8>,
}

pub(super) struct Connection {
    initial: Rc<ContextCallbacks>,
    selected: Option<Rc<ContextCallbacks>>,
    exchange: Option<Box<Exchange>>,
}

impl Connection {
    pub(super) fn new(ctx: &Context) -> Self {
        Self {
            initial: Rc::clone(&ctx.callbacks),
            selected: None,
            exchange: None,
        }
    }

    pub(super) fn selected_alpn_protocol(&self, ssl: NonNull<c_void>) -> Option<Vec<u8>> {
        self.initial.alpn.selected(ssl)
    }

    pub(super) fn set_context(&mut self, ssl: Handle, ctx: &Context) -> Result<(), Failure> {
        ctx.api.clear();
        // SAFETY: SSL_set_SSL_CTX takes a native reference on the same provider.
        if unsafe { (ctx.api.SSL_set_SSL_CTX)(ssl, ctx.handle.as_ptr()) }.is_null() {
            return Err(ctx.api.error("SSL_set_SSL_CTX"));
        }
        self.selected = Some(Rc::clone(&ctx.callbacks));
        Ok(())
    }

    pub(super) fn check_reentry(&self) -> Result<(), BioFailure> {
        if self
            .exchange
            .as_ref()
            .is_some_and(|e| e.phase.get() == Phase::Waiting)
        {
            return Err(BioFailure::WantRead);
        }
        Ok(())
    }

    fn enabled(&self) -> bool {
        self.initial.sni_enabled.get()
            || self.selected.as_ref().is_some_and(|c| c.sni_enabled.get())
    }

    fn allows_renegotiation(&self, api: &Api, ssl: NonNull<c_void>, server: bool) -> bool {
        const SSL_OP_ALLOW_CLIENT_RENEGOTIATION: u64 = 1 << 8;
        const SSL_OP_NO_RENEGOTIATION: u64 = 1 << 30;
        const TLS1_3_VERSION: c_int = 0x0304;
        if !self.enabled() {
            return false;
        }
        // SAFETY: both values belong to this negotiated SSL. A client can receive
        // a server's HelloRequest; a server must opt in to client renegotiation.
        let (version, options) = unsafe {
            (
                (api.SSL_version)(ssl.as_ptr()),
                (api.SSL_get_options)(ssl.as_ptr()),
            )
        };
        version < TLS1_3_VERSION
            && options & SSL_OP_NO_RENEGOTIATION == 0
            && (!server || options & SSL_OP_ALLOW_CLIENT_RENEGOTIATION != 0)
    }

    pub(super) fn continue_before_write(
        &self,
        api: &Api,
        ssl: NonNull<c_void>,
        server: bool,
    ) -> bool {
        self.allows_renegotiation(api, ssl, server)
            // SAFETY: query whether an earlier read already started a new handshake.
            && unsafe { (api.SSL_is_init_finished)(ssl.as_ptr()) } == 0
    }

    /// A connection that can renegotiate reads through SSL_peek first. OpenSSL negotiates on
    /// its normal read path without consuming application data or retaining a VM buffer.
    pub(super) fn prepare(
        &mut self,
        api: &Api,
        ssl: NonNull<c_void>,
        server: bool,
        entry: Entry,
    ) -> Result<bool, BioFailure> {
        self.check_reentry()?;
        if let Some(e) = &self.exchange
            && matches!(e.phase.get(), Phase::Resolved(_))
        {
            if e.entry.get() == entry {
                return Ok(true);
            }
            return Err(BioFailure::WantRead);
        }
        let needed = match entry {
            Entry::Handshake => self.enabled(),
            Entry::Peek => self.allows_renegotiation(api, ssl, server),
        };
        if !needed {
            return Ok(false);
        }
        if self.exchange.is_none() {
            let exchange = Box::new(Exchange {
                phase: Cell::new(Phase::Off),
                entry: Cell::new(entry),
                peek_byte: Cell::new(0),
            });
            // SAFETY: SSL_set_app_data stores a stable address. Drop unwinds any
            // paused entry before freeing the SSL and then its exchange.
            if unsafe {
                (api.SSL_set_ex_data)(ssl.as_ptr(), 0, ptr::from_ref(&*exchange).cast_mut().cast())
            } != 1
            {
                return Err(api.error("SSL_set_app_data").into());
            }
            self.exchange = Some(exchange);
        }
        let e = self.exchange.as_ref().unwrap();
        e.entry.set(entry);
        // SAFETY: the only entries armed for ASYNC have no caller-owned output.
        unsafe { (api.SSL_ctrl)(ssl.as_ptr(), 33, SSL_MODE_ASYNC, ptr::null_mut()) };
        e.phase.set(Phase::Armed);
        Ok(true)
    }

    pub(super) fn invoke(&self, api: &Api, ssl: NonNull<c_void>, entry: Entry) -> c_int {
        match entry {
            // SAFETY: SSL is live for the complete handshake.
            Entry::Handshake => unsafe { (api.SSL_do_handshake)(ssl.as_ptr()) },
            Entry::Peek => {
                let e = self.exchange.as_ref().expect("armed SSL_peek");
                // SAFETY: only the one-byte scratch cell on this connection is
                // writable by OpenSSL. Its boxed address survives retry and close.
                unsafe { (api.SSL_peek)(ssl.as_ptr(), e.peek_byte.as_ptr().cast(), 1) }
            }
        }
    }

    pub(super) fn finish(&self, api: &Api, ssl: NonNull<c_void>) {
        if let Some(e) = &self.exchange
            && e.phase.get() == Phase::Armed
        {
            // SAFETY: clear the mode only after the async entry has returned.
            unsafe { (api.SSL_ctrl)(ssl.as_ptr(), 78, SSL_MODE_ASYNC, ptr::null_mut()) };
            e.phase.set(Phase::Off);
        }
    }

    pub(super) fn resolve(&self, alert: Option<c_int>) -> Result<(), Failure> {
        if let Some(e) = &self.exchange
            && e.phase.get() == Phase::Waiting
        {
            e.phase.set(Phase::Resolved(alert));
            return Ok(());
        }
        Err(Failure::Other(
            "no SNI callback is awaiting a result".into(),
        ))
    }

    fn suspended_exchange(&self) -> Option<&Exchange> {
        self.exchange
            .as_deref()
            .filter(|e| matches!(e.phase.get(), Phase::Waiting | Phase::Resolved(_)))
    }

    pub(super) fn suspended(&self) -> bool {
        self.suspended_exchange().is_some()
    }

    pub(super) fn detach_closing_socket(
        &self,
        api: &Api,
        ssl: NonNull<c_void>,
    ) -> Result<bool, Failure> {
        if !self.suspended() {
            return Ok(false);
        }
        const BIO_C_SET_FD: c_int = 104;
        let mut invalid: c_int = -1;
        // SAFETY: SSL_set_fd installed one non-closing socket BIO for both
        // directions. Invalidate its borrowed fd in place before closing the
        // descriptor. A duplicated fd stays usable by its owner.
        let rc = unsafe {
            let bio = (api.SSL_get_rbio)(ssl.as_ptr());
            (api.BIO_ctrl)(bio, BIO_C_SET_FD, 0, ptr::from_mut(&mut invalid).cast())
        };
        if rc <= 0 {
            return Err(api.error("BIO_set_fd"));
        }
        Ok(true)
    }

    pub(super) fn cancel(&self, api: &Api, ssl: NonNull<c_void>) {
        if let Some(e) = self.suspended_exchange() {
            e.phase.set(Phase::Resolved(Some(INTERNAL_ERROR)));
            self.invoke(api, ssl, e.entry.get());
        }
    }
}

impl Context {
    pub(in crate::dynamic::stdlib::ssl) fn sni_callback_enabled(&self) -> bool {
        self.callbacks.sni_enabled.get()
    }

    pub(in crate::dynamic::stdlib::ssl) fn set_sni_callback(
        &self,
        enabled: bool,
    ) -> Result<(), Failure> {
        if enabled
            && (self.api.ASYNC_pause_job.is_none()
            // SAFETY: this predicate checks local OpenSSL async-stack capability.
            || !self.api.ASYNC_is_capable.is_some_and(|capable| unsafe { capable() } == 1))
        {
            return Err(Failure::Other(
                "OpenSSL async support is unavailable for SNI callbacks".into(),
            ));
        }
        self.api.clear();
        let ctx = self.handle.as_ptr();
        let callback = if enabled {
            // SAFETY: the argument is the process-lifetime API, also used by any
            // previous registration. Until the callback is installed it is inert.
            if unsafe {
                (self.api.SSL_CTX_ctrl)(ctx, 54, 0, ptr::from_ref(self.api).cast_mut().cast())
            } == 0
            {
                return Err(self.api.error("SSL_CTX_set_tlsext_servername_arg"));
            }
            // SAFETY: the OpenSSL macro erases this function pointer for
            // SSL_CTX_callback_ctrl. Command 53 restores its servername signature.
            Some(unsafe { std::mem::transmute::<Callback, unsafe extern "C" fn()>(servername) })
        } else {
            None
        };
        // SAFETY: the callback has static lifetime. Update our registration
        // only after the provider has accepted the change.
        if unsafe { (self.api.SSL_CTX_callback_ctrl)(ctx, 53, callback) } == 0 {
            return Err(self.api.error("SSL_CTX_set_tlsext_servername_callback"));
        }
        self.callbacks.sni_enabled.set(enabled);
        Ok(())
    }
}

unsafe extern "C" fn servername(ssl: Handle, alert: *mut c_int, arg: Handle) -> c_int {
    // SAFETY: our SSL_CTX registered its static provider API; the SSL owns app_data.
    let api = unsafe { &*arg.cast::<Api>() };
    // SAFETY: the connection owns this boxed cell for the full suspended call.
    let phase = unsafe { (api.SSL_get_ex_data)(ssl, 0).cast::<Exchange>().as_ref() };
    let mut decision = Some(INTERNAL_ERROR);
    if let (Some(e), Some(pause)) = (phase, api.ASYNC_pause_job)
        && e.phase.get() == Phase::Armed
    {
        e.phase.set(Phase::Waiting);
        // SAFETY: no mutable borrow or caller's plaintext buffer crosses the suspension.
        let resumed = unsafe { pause() };
        if let Phase::Resolved(result) = e.phase.replace(Phase::Armed)
            && resumed == 1
        {
            decision = result;
        }
    }
    if let Some(code) = decision {
        // SAFETY: OpenSSL supplied the writable alert for this handshake.
        unsafe { *alert = code };
        2 // SSL_TLSEXT_ERR_ALERT_FATAL
    } else {
        0 // SSL_TLSEXT_ERR_OK
    }
}

pub(super) fn paused(api: &Api, ssl: Handle) -> BioFailure {
    // SAFETY: only an outstanding servername callback publishes Phase::Waiting.
    let phase = unsafe { (api.SSL_get_ex_data)(ssl, 0).cast::<Exchange>().as_ref() };
    if phase.is_some_and(|e| e.phase.get() == Phase::Waiting) {
        // SAFETY: the real OpenSSL callback is suspended on this SSL while its
        // validated NUL-terminated hostname is copied. No Python has run yet.
        let ptr = unsafe { (api.SSL_get_servername)(ssl, 0) };
        // SAFETY: a non-null pointer here is the SSL-owned server name.
        let name = (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec());
        return BioFailure::ClientHello(name);
    }
    BioFailure::Tls(Failure::Other(
        "unexpected asynchronous OpenSSL operation".into(),
    ))
}
