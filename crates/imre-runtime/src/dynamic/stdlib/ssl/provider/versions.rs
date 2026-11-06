//! OpenSSL's TLS protocol bounds and CPython's minimum/maximum sentinels.
use std::ffi::{c_int, c_long, c_ulong};
use std::ptr;

use super::Context;
use crate::dynamic::{
    Context as Vm,
    value::{Error, FALSE, Id, Result, TRUE},
};
use crate::objects::exceptions::Kind;

pub(super) const SSL_OP_NO_SSLV3: u64 = 1 << 25;
const SSL3: i32 = 0x300;
const VERSIONS: [&str; 5] = ["SSLv3", "TLSv1", "TLSv1_1", "TLSv1_2", "TLSv1_3"];
const MINIMUM: i32 = -2;
const MAXIMUM: i32 = -1;

unsafe extern "C" {
    fn imre_ssl_protocol_mask(runtime_version: c_ulong) -> c_int;
}

pub(super) fn capabilities(version: c_ulong) -> Option<u32> {
    // SAFETY: the shim reads only its build-time OpenSSL header constants. A
    // missing header or a runtime/header version mismatch returns -1.
    u32::try_from(unsafe { imre_ssl_protocol_mask(version) }).ok()
}

pub(super) fn install(ctx: &mut Vm, scope: Id, runtime_version: c_ulong) {
    let capabilities = capabilities(runtime_version);
    let min = ctx.int(i64::from(MINIMUM));
    let max = ctx.int(i64::from(MAXIMUM));
    ctx.bind(scope, "PROTO_MINIMUM_SUPPORTED", min);
    ctx.bind(scope, "PROTO_MAXIMUM_SUPPORTED", max);
    // SSLv2 was removed before OpenSSL 3.
    ctx.bind(scope, "HAS_SSLv2", FALSE);
    for (index, name) in VERSIONS.iter().enumerate() {
        let value = ctx.int(i64::from(SSL3) + index as i64);
        ctx.bind(scope, &format!("PROTO_{name}"), value);
        if let Some(mask) = capabilities {
            let has = mask & (1 << index) != 0;
            ctx.bind(
                scope,
                &format!("HAS_{name}"),
                if has { TRUE } else { FALSE },
            );
            if has && index == 0 {
                let protocol = ctx.int(1);
                ctx.bind(scope, "PROTOCOL_SSLv3", protocol);
            }
        }
    }
}

impl Context {
    pub(in crate::dynamic) fn protocol_bound(&self, minimum: bool) -> i32 {
        // SSL_CTRL_GET_MIN_PROTO_VERSION and SSL_CTRL_GET_MAX_PROTO_VERSION.
        let command = if minimum { 130 } else { 131 };
        // SAFETY: SSL_CTX_ctrl reads the requested bound from our live context.
        let value =
            unsafe { (self.api.SSL_CTX_ctrl)(self.handle.as_ptr(), command, 0, ptr::null_mut()) };
        if value != 0 {
            value as i32
        } else if minimum {
            MINIMUM
        } else {
            MAXIMUM
        }
    }

    fn set_bound(&mut self, minimum: bool, value: i32) -> Result<()> {
        let version = match (minimum, value) {
            (true, MINIMUM) | (false, MAXIMUM) => 0,
            (true, MAXIMUM) | (false, MINIMUM) => {
                // OpenSSL has no "highest minimum" or "lowest maximum" API.
                // Use the available versions, just as CPython does from its headers.
                // SAFETY: OpenSSL_version_num queries the loaded library.
                let number = unsafe { (self.api.OpenSSL_version_num)() };
                let mask = capabilities(number)
                    .filter(|mask| *mask != 0)
                    .ok_or_else(|| {
                        Error::new(
                            "NotImplementedError",
                            "OpenSSL protocol bounds require matching OpenSSL 3 headers",
                        )
                    })?;
                SSL3 + if minimum {
                    mask.ilog2()
                } else {
                    mask.trailing_zeros()
                } as i32
            }
            _ => value,
        };
        // SSL_CTRL_SET_MIN_PROTO_VERSION and SSL_CTRL_SET_MAX_PROTO_VERSION.
        let command = if minimum {
            super::SSL_CTRL_SET_MIN_PROTO_VERSION
        } else {
            124
        };
        self.api.clear();
        // SAFETY: OpenSSL copies the integer bound onto the live SSL_CTX.
        if unsafe {
            (self.api.SSL_CTX_ctrl)(
                self.handle.as_ptr(),
                command,
                c_long::from(version),
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(Error::new(
                "ValueError",
                format!("Unsupported protocol version 0x{:x}", version as u32),
            ));
        }
        Ok(())
    }
}

pub(in crate::dynamic) fn set(ctx: &mut Vm, handle: Id, minimum: bool, arg: Id) -> Result<()> {
    let value = ctx.index_c_int(arg)?;
    if super::super::context(ctx, handle)?
        .borrow()
        .protocol
        .is_fixed()
    {
        return Err(Error::new(
            "ValueError",
            "The context's protocol doesn't support modification of highest and lowest version.",
        ));
    }
    match value {
        0x300..=0x302 => {
            let name = VERSIONS[(value - SSL3) as usize];
            ctx.emit_builtin_warning(
                Kind::DeprecationWarning,
                &format!("ssl.TLSVersion.{name} is deprecated"),
                2,
            )?;
        }
        MINIMUM | MAXIMUM | 0x303 | 0x304 => {}
        _ => {
            return Err(Error::new(
                "ValueError",
                format!("Unsupported TLS/SSL version 0x{:x}", value as u32),
            ));
        }
    }
    super::super::context(ctx, handle)?
        .borrow_mut()
        .set_bound(minimum, value)
}
