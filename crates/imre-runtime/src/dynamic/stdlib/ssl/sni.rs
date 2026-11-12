//! Dispatch the OpenSSL ClientHello decision outside the provider's session and buffer borrows.
use super::{Context, Error, Id, NONE, Result, Value, closed, context, provider, provider_error};
use provider::bio::BioFailure;

#[derive(Clone, Copy, Debug)]
pub(in crate::dynamic) struct Owner {
    handle: Id,
    context: Id,
    weak: Id,
    // OpenSSL tries the original session context if the selected context has no
    // callback. Keep weak references after a switch so the old context can die.
    original_handle: Id,
    original_context: Id,
}

impl Owner {
    pub(super) fn new(handle: Id, context: Id, weak: Id) -> Self {
        Self {
            handle,
            context,
            weak,
            original_handle: NONE,
            original_context: NONE,
        }
    }

    pub(in crate::dynamic) fn context(&self) -> Id {
        self.context
    }

    pub(super) fn receiver(&self, ctx: &Context) -> Option<Id> {
        weak_target(ctx, self.weak)
    }

    pub(in crate::dynamic) fn references(&self) -> [Id; 5] {
        [
            self.handle,
            self.context,
            self.weak,
            self.original_handle,
            self.original_context,
        ]
    }
}

/// A reentrant registration can leave a Python callable installed while the native
/// hook is disabled. During Py_CLEAR its former hook may also see an empty slot.
#[derive(Clone, Copy, Debug)]
pub(in crate::dynamic) struct Registration {
    pub(in crate::dynamic) callback: Id,
    enabled: bool,
}

impl Registration {
    pub(super) const EMPTY: Self = Self {
        callback: NONE,
        enabled: false,
    };
}

pub(super) fn callback(ctx: &mut Context, id: Id, new: Option<Id>) -> Result<Id> {
    let Value::TlsContext(handle, registration) = ctx.heap.get(id) else {
        return Err(Error::new("TypeError", "expected a TLS context"));
    };
    let old = registration.callback;
    let Some(new) = new else { return Ok(old) };
    if handle.borrow().protocol == provider::Protocol::Client {
        return Err(Error::new(
            "ValueError",
            "sni_callback cannot be set on TLS_CLIENT context",
        ));
    }
    let handle = std::rc::Rc::clone(handle);
    let callable = ctx.callable(new);
    // If OpenSSL rejects the change, preserve the previous live registration.
    handle
        .borrow()
        .set_sni_callback(callable)
        .map_err(|err| provider_error(ctx, err))?;
    if old != NONE {
        let Value::TlsContext(_, slot) = ctx.heap.get_mut(id) else {
            unreachable!()
        };
        slot.callback = NONE;
        // _ssl clears the slot before releasing its owner. A destructor or weakref
        // callback may inspect it, perform a handshake, or install another callback.
        ctx.heap.keep(id);
        ctx.heap.keep(new);
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(old);
        ctx.discard_temporaries(mark, None);
        if handle.borrow().sni_callback_enabled() != callable {
            handle
                .borrow()
                .set_sni_callback(callable)
                .map_err(|err| provider_error(ctx, err))?;
        }
    }
    let Value::TlsContext(_, slot) = ctx.heap.get_mut(id) else {
        unreachable!()
    };
    slot.enabled = callable;
    if callable {
        // Py_CLEAR above can run a finalizer that installs an intervening callable.
        // CPython overwrites that owned reference without decref, even if the context dies.
        if slot.callback != NONE {
            ctx.displaced_sni_callbacks
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            ctx.displaced_sni_callbacks.insert(slot.callback);
        }
        slot.callback = new;
    } else if new != NONE {
        return Err(Error::new("TypeError", "not a callable object"));
    }
    Ok(NONE)
}

pub(super) fn set_context(
    ctx: &mut Context,
    session: Id,
    id: Id,
    public_context: Id,
) -> Result<()> {
    let mut owner = match *ctx.heap.get(session) {
        Value::TlsSession(_, owner) | Value::TlsBioSession { owner, .. } => owner,
        _ => return Err(closed()),
    };
    if owner.weak != NONE && owner.handle != id && owner.original_context == NONE {
        owner.original_context = ctx.heap.native_weak_reference(owner.context)?;
        owner.original_handle = ctx.heap.native_weak_reference(owner.handle)?;
    }
    let handle = context(ctx, id)?.borrow();
    let result = match ctx.heap.get(session) {
        Value::TlsSession(state, _) => state
            .borrow_mut()
            .as_mut()
            .ok_or_else(closed)?
            .set_context(&handle),
        Value::TlsBioSession { session, .. } => session.borrow_mut().set_context(&handle),
        _ => return Err(closed()),
    };
    drop(handle);
    result.map_err(|err| provider_error(ctx, err))?;
    let slot = match ctx.heap.get_mut(session) {
        Value::TlsSession(_, owner) | Value::TlsBioSession { owner, .. } => owner,
        _ => unreachable!(),
    };
    owner.handle = id;
    owner.context = public_context;
    *slot = owner;
    Ok(())
}

fn weak_target(ctx: &Context, reference: Id) -> Option<Id> {
    ctx.heap.get(reference).weak_reference()?.target
}

fn decision(ctx: &mut Context, owner: Owner, name: Option<Vec<u8>>) -> Option<i32> {
    const INTERNAL_ERROR: i32 = 80;
    const HANDSHAKE_FAILURE: i32 = 40;
    const CALL_ERROR: &str =
        "Exception ignored in ssl servername callback while calling set SNI callback";
    let registered = |handle, context| match *ctx.heap.get(handle) {
        Value::TlsContext(_, slot) if slot.enabled => Some((handle, context, slot.callback)),
        _ => None,
    };
    let (handle, context, callback) = registered(owner.handle, owner.context).or_else(|| {
        registered(
            weak_target(ctx, owner.original_handle)?,
            weak_target(ctx, owner.original_context)?,
        )
    })?;
    if callback == NONE {
        return None;
    }
    // A callback can switch contexts, replace itself and collect the heap.
    ctx.heap.keep(handle);
    let operands = ctx.heap.temporary_mark();
    ctx.heap.keep(callback);
    ctx.heap.keep(context);
    let Some(peer) = weak_target(ctx, owner.weak) else {
        return Some(INTERNAL_ERROR);
    };
    ctx.heap.keep(peer);
    let name = match name {
        None => NONE,
        Some(bytes) => {
            let data = ctx.alloc(Value::Bytes(bytes));
            match crate::dynamic::stdlib::codecs::decode_bytes(
                ctx,
                crate::objects::Codec::Ascii,
                data,
                None,
            ) {
                Ok(name) => name,
                Err(err) => {
                    crate::dynamic::unraisable::report(
                        ctx,
                        err,
                        true,
                        Some(data),
                        "Exception ignored in ssl servername callback while decoding name",
                    );
                    return Some(INTERNAL_ERROR);
                }
            }
        }
    };
    let result = ctx.call(callback, vec![peer, name, context], vec![]);
    // A self-cleared callable may finalize and change the context before OpenSSL
    // selects the certificate. Keep its return value or exception across that release.
    match ctx.release_with_result(operands, result) {
        Ok(NONE) => None,
        Ok(result) => match ctx.index_c_long(result) {
            Ok(alert) => Some(alert as i32),
            Err(err) => {
                crate::dynamic::unraisable::report_with_message(ctx, err, true, |ctx| {
                    Ok(format!(
                        "{CALL_ERROR} (result={})",
                        ctx.checked_format(result, true)?
                    ))
                });
                Some(INTERNAL_ERROR)
            }
        },
        Err(err) => {
            // _ssl reports the registration as it stands after the callback raises.
            let Value::TlsContext(_, slot) = *ctx.heap.get(handle) else {
                unreachable!()
            };
            let current = slot.callback;
            ctx.heap.keep(current);
            crate::dynamic::unraisable::report_with_message(ctx, err, true, |ctx| {
                let callback = if current == NONE {
                    "<NULL>".to_owned()
                } else {
                    ctx.checked_format(current, true)?
                };
                Ok(format!("{CALL_ERROR} {callback}"))
            });
            Some(HANDSHAKE_FAILURE)
        }
    }
}

pub(super) fn drive<T>(
    ctx: &mut Context,
    id: Id,
    mut operation: impl FnMut(&mut Context) -> Result<std::result::Result<T, BioFailure>>,
) -> Result<std::result::Result<T, BioFailure>> {
    loop {
        let name = match operation(ctx)? {
            Err(BioFailure::Tls(provider::Failure::Io(e)))
                if e.kind() == std::io::ErrorKind::Interrupted =>
            {
                // The provider and its destination have both been released before Python runs.
                super::callback_result(ctx, Context::dispatch_signals)?;
                continue;
            }
            Err(BioFailure::ClientHello(name)) => name,
            result => return Ok(result),
        };
        let owner = match *ctx.heap.get(id) {
            Value::TlsSession(_, owner) | Value::TlsBioSession { owner, .. } => owner,
            _ => unreachable!(),
        };
        let alert = super::callback_result(ctx, |ctx| Ok(decision(ctx, owner, name)))?;
        let resumed = match ctx.heap.get(id) {
            Value::TlsSession(state, _) => state
                .borrow()
                .as_ref()
                .ok_or_else(closed)?
                .resolve_sni(alert),
            Value::TlsBioSession { session, .. } => session.borrow().resolve_sni(alert),
            Value::TlsTransport(_) => return super::inactive_write(ctx, id),
            _ => unreachable!(),
        };
        resumed.map_err(|error| provider_error(ctx, error))?;
    }
}
