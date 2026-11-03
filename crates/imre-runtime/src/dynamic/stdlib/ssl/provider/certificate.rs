//! Decode X.509 certificates into Python certificate dictionaries.
use std::ffi::c_int;
use std::{mem, ptr, slice};

use super::super::{context, provider_error};
use super::{Api, Handle};
use crate::dynamic::{
    Context,
    value::{Error, Id, Result, Value},
};
use crate::objects::Codec;

mod extensions;

pub(in crate::dynamic::stdlib::ssl) fn ca_certs(
    ctx: &mut Context,
    handle: Id,
    binary: bool,
) -> Result<Id> {
    let snapshot = context(ctx, handle)?
        .borrow()
        .store_snapshot()
        .ok_or_else(|| Error::new("MemoryError", "failed to query cert store"))?;
    let mut certificates = Vec::new();
    for cert in snapshot.authorities() {
        let item = if binary {
            let der = snapshot
                .api
                .encode_certificate(cert)
                .map_err(|e| provider_error(ctx, e))?;
            ctx.alloc(Value::Bytes(der))
        } else {
            decode(ctx, snapshot.api, cert)?
        };
        certificates.push(item);
    }
    Ok(ctx.alloc(Value::List(certificates)))
}

fn utf8(ctx: &mut Context, bytes: &[u8]) -> Result<String> {
    Codec::Utf8 { signature: false }
        .decode(bytes, None)
        .map_err(|err| ctx.decode_error(err))
}

/// Read one OpenSSL-formatted line, as CPython does for serials, dates and general names.
fn printed(
    ctx: &mut Context,
    api: &Api,
    value: Handle,
    print: unsafe extern "C" fn(Handle, Handle) -> c_int,
) -> Result<String> {
    let mut buf = [0u8; 2048];
    // SAFETY: the memory BIO belongs to this call, and each caller supplies a
    // live object of the type required by the paired OpenSSL printer.
    let len = unsafe {
        let bio = (api.BIO_new)((api.BIO_s_mem)());
        if bio.is_null() {
            return Err(provider_error(ctx, api.error("BIO_new")));
        }
        print(bio, value);
        let len = (api.BIO_gets)(bio, buf.as_mut_ptr().cast(), (buf.len() - 1) as c_int);
        (api.BIO_free)(bio);
        len
    };
    if len < 0 {
        return Err(provider_error(ctx, api.error("BIO_gets")));
    }
    utf8(ctx, &buf[..len as usize])
}

fn object_text(ctx: &mut Context, api: &Api, object: Handle) -> Result<String> {
    // SAFETY: both calls format the same borrowed OID. The first requests its size.
    let len = unsafe { (api.OBJ_obj2txt)(ptr::null_mut(), 0, object, 0) };
    if len < 0 {
        return Err(provider_error(ctx, api.error("OBJ_obj2txt")));
    }
    let size = len
        .checked_add(1)
        .ok_or_else(|| Error::new("OverflowError", "OID too long"))?;
    let mut buf = vec![0u8; size as usize];
    // SAFETY: buf also includes room for the trailing terminator.
    if unsafe { (api.OBJ_obj2txt)(buf.as_mut_ptr().cast(), size, object, 0) } != len {
        return Err(provider_error(ctx, api.error("OBJ_obj2txt")));
    }
    utf8(ctx, &buf[..len as usize])
}

fn attribute(ctx: &mut Context, api: &Api, entry: Handle) -> Result<Id> {
    // SAFETY: both objects are borrowed from this entry in the live X509 name.
    let (object, value) = unsafe {
        (
            (api.X509_NAME_ENTRY_get_object)(entry),
            (api.X509_NAME_ENTRY_get_data)(entry),
        )
    };
    let key = object_text(ctx, api, object)?;
    // SAFETY: the X509 entry owns value. BIT STRING is returned as original bytes.
    // Other ASN.1 string forms are converted to a separately allocated UTF-8 buffer.
    let attr = unsafe {
        if (api.ASN1_STRING_type)(value) == 3 {
            let len = (api.ASN1_STRING_length)(value);
            let bytes = (api.ASN1_STRING_get0_data)(value);
            if len < 0 || (len > 0 && bytes.is_null()) {
                return Err(provider_error(ctx, api.error("ASN1_STRING_get0_data")));
            }
            let bytes = if len == 0 {
                vec![]
            } else {
                slice::from_raw_parts(bytes, len as usize).to_vec()
            };
            ctx.alloc(Value::Bytes(bytes))
        } else {
            let mut data = ptr::null_mut();
            let len = (api.ASN1_STRING_to_UTF8)(&mut data, value);
            if len < 0 || (len > 0 && data.is_null()) {
                return Err(provider_error(ctx, api.error("ASN1_STRING_to_UTF8")));
            }
            let bytes = if len == 0 {
                &[]
            } else {
                slice::from_raw_parts(data, len as usize)
            };
            let text = utf8(ctx, bytes);
            (api.CRYPTO_free)(data.cast(), ptr::null(), 0);
            ctx.text(text?)
        }
    };
    let key = ctx.text(key);
    Ok(ctx.alloc(Value::Tuple(vec![key, attr])))
}

/// Preserve each relative distinguished name as one tuple of attribute pairs.
fn name(ctx: &mut Context, api: &Api, name: Handle) -> Result<Id> {
    // SAFETY: the subject, issuer or general name owns the X509_NAME for this walk.
    let count = unsafe { (api.X509_NAME_entry_count)(name) };
    if count < 0 {
        return Err(provider_error(ctx, api.error("X509_NAME_entry_count")));
    }
    let mut names = Vec::new();
    let mut group = Vec::new();
    let mut previous_set = -1;
    for index in 0..count {
        // SAFETY: index is in bounds and the name remains live and unchanged.
        let entry = unsafe { (api.X509_NAME_get_entry)(name, index) };
        // SAFETY: the live name owns the entry returned for this in-bounds index.
        let set = unsafe { (api.X509_NAME_ENTRY_set)(entry) };
        if !group.is_empty() && previous_set != set {
            names.push(ctx.alloc(Value::Tuple(mem::take(&mut group))));
        }
        group.push(attribute(ctx, api, entry)?);
        previous_set = set;
    }
    if !group.is_empty() {
        names.push(ctx.alloc(Value::Tuple(group)));
    }
    Ok(ctx.alloc(Value::Tuple(names)))
}

pub(super) fn decode(ctx: &mut Context, api: &Api, cert: Handle) -> Result<Id> {
    // SAFETY: these fields are borrowed from the live, non-null peer certificate.
    let (subject, issuer, version, serial, before, after) = unsafe {
        (
            (api.X509_get_subject_name)(cert),
            (api.X509_get_issuer_name)(cert),
            (api.X509_get_version)(cert),
            (api.X509_get_serialNumber)(cert),
            (api.X509_get0_notBefore)(cert),
            (api.X509_get0_notAfter)(cert),
        )
    };
    let subject = name(ctx, api, subject)?;
    let issuer = name(ctx, api, issuer)?;
    let serial = printed(ctx, api, serial, api.i2a_ASN1_INTEGER)?;
    let before = printed(ctx, api, before, api.ASN1_TIME_print)?;
    let after = printed(ctx, api, after, api.ASN1_TIME_print)?;
    #[allow(clippy::useless_conversion, reason = "X509_get_version returns C long")]
    let version = i64::from(version);
    let entries = vec![
        (ctx.text("subject"), subject),
        (ctx.text("issuer"), issuer),
        (ctx.text("version"), ctx.int(version + 1)),
        (ctx.text("serialNumber"), ctx.text(serial)),
        (ctx.text("notBefore"), ctx.text(before)),
        (ctx.text("notAfter"), ctx.text(after)),
    ];
    let cert_dict = ctx.dict(entries);
    extensions::add(ctx, api, cert, cert_dict)?;
    Ok(cert_dict)
}
