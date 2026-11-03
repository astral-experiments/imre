//! Decode the general names and URI extensions exposed by SSLObject.getpeercert.
use std::ffi::c_int;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::ptr::{self, NonNull};

use super::super::{Api, Handle};
use super::{name, object_text, printed, utf8};
use crate::dynamic::{
    Context,
    value::{Error, Id, Result, Value},
};

// OpenSSL obj_mac.h and x509v3.h constants.
const NID_SUBJECT_ALT_NAME: c_int = 85;
const NID_INFO_ACCESS: c_int = 177;
const NID_AD_OCSP: c_int = 178;
const NID_AD_CA_ISSUERS: c_int = 179;
const NID_CRL_DISTRIBUTION_POINTS: c_int = 103;
const GEN_EMAIL: c_int = 1;
const GEN_DNS: c_int = 2;
const GEN_DIRNAME: c_int = 4;
const GEN_URI: c_int = 6;
const GEN_IPADD: c_int = 7;
const GEN_RID: c_int = 8;

// These fields are public in OpenSSL 3's x509v3.h. Only the leading fields
// of DIST_POINT and DIST_POINT_NAME are needed.
#[repr(C)]
struct AccessDescription {
    method: Handle,
    location: Handle,
}

#[repr(C)]
struct DistributionPoint {
    name: *const DistributionPointName,
}

#[repr(C)]
struct DistributionPointName {
    kind: c_int,
    names: Handle,
}

struct Extension {
    stack: NonNull<std::ffi::c_void>,
    free: unsafe extern "C" fn(Handle),
}

impl Extension {
    fn decode(
        api: &Api,
        cert: Handle,
        nid: c_int,
        free: unsafe extern "C" fn(Handle),
    ) -> Option<Self> {
        // SAFETY: each caller pairs the extension's NID with its OpenSSL destructor.
        // A missing, duplicate or undecodable extension returns null, as in CPython.
        let stack = NonNull::new(unsafe {
            (api.X509_get_ext_d2i)(cert, nid, ptr::null_mut(), ptr::null_mut())
        })?;
        Some(Self { stack, free })
    }
}

impl Drop for Extension {
    fn drop(&mut self) {
        // SAFETY: free the decoded stack, including all nested ASN.1 values, once.
        unsafe { (self.free)(self.stack.as_ptr()) };
    }
}

fn entries(api: &Api, stack: Handle) -> impl Iterator<Item = Handle> {
    // SAFETY: callers hold the enclosing extension until traversal is complete.
    let count = unsafe { (api.OPENSSL_sk_num)(stack) };
    (0..count).map(move |i| unsafe { (api.OPENSSL_sk_value)(stack, i) })
}

fn general_value(api: &Api, general_name: Handle) -> (c_int, Handle) {
    let mut kind = -1;
    // SAFETY: general_name is part of a successfully decoded GENERAL_NAMES or AIA.
    let value = unsafe { (api.GENERAL_NAME_get0_value)(general_name, &mut kind) };
    (kind, value)
}

/// Borrow the ASN.1 bytes with their explicit length, including interior NULs.
unsafe fn asn1_bytes<'a>(api: &Api, value: Handle, _owner: &'a Extension) -> &'a [u8] {
    // SAFETY: callers pass an ASN1_STRING owned by this extension. OpenSSL may
    // return null for an empty string, so avoid constructing a slice in that case.
    unsafe {
        let n = (api.ASN1_STRING_length)(value);
        if n <= 0 {
            &[]
        } else {
            std::slice::from_raw_parts((api.ASN1_STRING_get0_data)(value), n as usize)
        }
    }
}

fn general_name(ctx: &mut Context, api: &Api, general: Handle, owner: &Extension) -> Result<Id> {
    let (kind, value) = general_value(api, general);
    let (label, text) = match kind {
        GEN_EMAIL | GEN_DNS | GEN_URI => {
            let label = match kind {
                GEN_EMAIL => "email",
                GEN_DNS => "DNS",
                _ => "URI",
            };
            // SAFETY: these three general-name variants hold an ASN1_IA5STRING.
            let text = utf8(ctx, unsafe { asn1_bytes(api, value, owner) })?;
            (label, ctx.text(text))
        }
        GEN_DIRNAME => ("DirName", name(ctx, api, value)?),
        GEN_IPADD => {
            // SAFETY: GEN_IPADD contains an ASN1_OCTET_STRING.
            let address = unsafe { asn1_bytes(api, value, owner) };
            let text = match address.len() {
                4 => Ipv4Addr::from(<[u8; 4]>::try_from(address).unwrap()).to_string(),
                16 => Ipv6Addr::from(<[u8; 16]>::try_from(address).unwrap())
                    .segments()
                    .map(|part| format!("{part:X}"))
                    .join(":"),
                _ => "<invalid>".to_owned(),
            };
            ("IP Address", ctx.text(text))
        }
        GEN_RID => {
            let text = object_text(ctx, api, value)?;
            (
                "Registered ID",
                ctx.text(if text.len() >= 2048 {
                    "<INVALID>"
                } else {
                    &text
                }),
            )
        }
        _ => {
            let text = printed(ctx, api, general, api.GENERAL_NAME_print)?;
            let (key, value) = text
                .split_once(':')
                .ok_or_else(|| Error::new("ValueError", format!("Invalid value {text}")))?;
            let key = ctx.text(key);
            let value = ctx.text(value);
            return Ok(ctx.alloc(Value::Tuple(vec![key, value])));
        }
    };
    let key = ctx.text(label);
    Ok(ctx.alloc(Value::Tuple(vec![key, text])))
}

fn uri(ctx: &mut Context, api: &Api, general: Handle, owner: &Extension) -> Result<Option<Id>> {
    let (kind, value) = general_value(api, general);
    if kind != GEN_URI {
        return Ok(None);
    }
    // SAFETY: the GEN_URI discriminator identifies a borrowed ASN1_IA5STRING.
    let text = utf8(ctx, unsafe { asn1_bytes(api, value, owner) })?;
    Ok(Some(ctx.text(text)))
}

fn put_tuple(ctx: &mut Context, dict: Id, key: &str, entries: Vec<Id>) -> Result<()> {
    let key = ctx.text(key);
    let tuple = ctx.alloc(Value::Tuple(entries));
    ctx.set_dictionary_key(dict, key, tuple, None, true)?;
    Ok(())
}

pub(super) fn add(ctx: &mut Context, api: &Api, cert: Handle, dict: Id) -> Result<()> {
    if let Some(san) = Extension::decode(api, cert, NID_SUBJECT_ALT_NAME, api.GENERAL_NAMES_free) {
        let names = entries(api, san.stack.as_ptr())
            .map(|entry| general_name(ctx, api, entry, &san))
            .collect::<Result<Vec<_>>>()?;
        put_tuple(ctx, dict, "subjectAltName", names)?;
    }
    if let Some(aia) = Extension::decode(api, cert, NID_INFO_ACCESS, api.AUTHORITY_INFO_ACCESS_free)
    {
        let mut ocsp = Vec::new();
        let mut issuers = Vec::new();
        for entry in entries(api, aia.stack.as_ptr()) {
            // SAFETY: each AUTHORITY_INFO_ACCESS entry is an ACCESS_DESCRIPTION.
            let description = unsafe { &*entry.cast::<AccessDescription>() };
            let destinations = match unsafe { (api.OBJ_obj2nid)(description.method) } {
                NID_AD_OCSP => &mut ocsp,
                NID_AD_CA_ISSUERS => &mut issuers,
                _ => continue,
            };
            if let Some(location) = uri(ctx, api, description.location, &aia)? {
                destinations.push(location);
            }
        }
        for (key, locations) in [("OCSP", ocsp), ("caIssuers", issuers)] {
            if !locations.is_empty() {
                put_tuple(ctx, dict, key, locations)?;
            }
        }
    }
    if let Some(crl) = Extension::decode(
        api,
        cert,
        NID_CRL_DISTRIBUTION_POINTS,
        api.CRL_DIST_POINTS_free,
    ) {
        let mut uris = Vec::new();
        for entry in entries(api, crl.stack.as_ptr()) {
            // SAFETY: a DIST_POINT may omit its name. Only the fullName choice
            // (0) stores a GENERAL_NAMES stack; the other choice is a relative RDN.
            let Some(point) = (unsafe { (*entry.cast::<DistributionPoint>()).name.as_ref() })
            else {
                continue;
            };
            if point.kind == 0 {
                for location in entries(api, point.names) {
                    if let Some(url) = uri(ctx, api, location, &crl)? {
                        uris.push(url);
                    }
                }
            }
        }
        if !uris.is_empty() {
            put_tuple(ctx, dict, "crlDistributionPoints", uris)?;
        }
    }
    Ok(())
}
