//! The five URL fields preserve their original spelling except for the explicit scheme.
use super::super::bytes;
use super::super::exceptions::{Error, Kind, Result, finish, string_repr};
use crate::{Bytes, BytesOwner, Int, Str};
use std::{
    borrow::Cow,
    ffi::c_void,
    net::{Ipv4Addr, Ipv6Addr},
};
use unicode_normalization::UnicodeNormalization;

fn invalid(message: impl Into<String>) -> Error {
    Error::new(Kind::ValueError, message.into())
}

// SAFETY: the caller roots the exact string or bytes layout selected by the flag.
unsafe fn data<'a>(word: *const c_void, is_bytes: bool) -> &'a [u8] {
    if is_bytes {
        unsafe { &*word.cast::<Bytes>() }.as_slice()
    } else {
        unsafe { &*word.cast::<Str>() }.as_bytes()
    }
}

unsafe fn decoded<'a>(word: *const c_void, is_bytes: bool) -> Result<&'a str> {
    let raw = unsafe { data(word, is_bytes) };
    if is_bytes && let Some(i) = raw.iter().position(|b| !b.is_ascii()) {
        return Err(bytes::ascii_decode_error(
            unsafe { crate::acquire_bytes(word as usize) },
            i,
        ));
    }
    // SAFETY: bytes were checked for ASCII; string storage is UTF-8.
    Ok(unsafe { std::str::from_utf8_unchecked(raw) })
}

fn owned(value: &str, is_bytes: bool) -> Result<usize> {
    if is_bytes {
        Ok(BytesOwner::into_raw(bytes::owned_input(value.as_bytes())?) as usize)
    } else {
        Ok(
            crate::checked_owned_str(Str::try_new(value).map_err(|_| Error::memory_error())?)?
                as usize,
        )
    }
}

fn clean(value: &str, scheme: bool) -> Cow<'_, str> {
    let c0 = |c: char| c <= ' ';
    let value = if scheme {
        value.trim_matches(c0)
    } else {
        value.trim_start_matches(c0)
    };
    if value.contains(['\t', '\n', '\r']) {
        Cow::Owned(value.replace(['\t', '\n', '\r'], ""))
    } else {
        Cow::Borrowed(value)
    }
}

fn check_netloc(netloc: &str) -> Result<()> {
    match (netloc.contains('['), netloc.contains(']')) {
        (true, false) | (false, true) => return Err(invalid("Invalid IPv6 URL")),
        (true, true) => {
            let hostport = netloc.rsplit('@').next().unwrap();
            let host = if let Some((prefix, rest)) = hostport.split_once('[') {
                let (host, suffix) = rest.split_once(']').unwrap_or((rest, ""));
                if !prefix.is_empty() || (!suffix.is_empty() && !suffix.starts_with(':')) {
                    return Err(invalid("Invalid IPv6 URL"));
                }
                host
            } else {
                hostport.split(':').next().unwrap()
            };
            if let Some(future) = host.strip_prefix('v') {
                if !future.split_once('.').is_some_and(|(v, rest)| {
                    !v.is_empty() && v.bytes().all(|b| b.is_ascii_hexdigit()) && !rest.is_empty()
                }) {
                    return Err(invalid("IPvFuture address is invalid"));
                }
            } else if host.parse::<Ipv4Addr>().is_ok() {
                return Err(invalid("An IPv4 address cannot be in brackets"));
            } else {
                let (ip, scope) = host
                    .split_once('%')
                    .map_or((host, None), |(ip, zone)| (ip, Some(zone)));
                if ip.parse::<Ipv6Addr>().is_err()
                    || scope.is_some_and(|z| z.is_empty() || z.contains('%'))
                {
                    return Err(invalid(format!(
                        "{} does not appear to be an IPv4 or IPv6 address",
                        string_repr(host)
                    )));
                }
            }
        }
        _ => {}
    }
    // The authority cannot contain a slash. Remove its known delimiters before
    // checking whether a Unicode compatibility character introduces a new one.
    if !netloc.is_ascii()
        && netloc
            .chars()
            .filter(|c| !matches!(c, '@' | ':' | '#' | '?'))
            .nfkc()
            .any(|c| matches!(c, '/' | '?' | '#' | '@' | ':'))
    {
        return Err(invalid(format!(
            "netloc '{netloc}' contains invalid characters under NFKC normalization"
        )));
    }
    Ok(())
}

fn split<'a>(
    mut url: &'a str,
    default_scheme: &'a str,
    fragments: bool,
) -> Result<[Cow<'a, str>; 5]> {
    let mut scheme = Cow::Borrowed(default_scheme);
    if let Some((prefix, rest)) = url.split_once(':')
        && prefix
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
    {
        scheme = Cow::Owned(prefix.to_ascii_lowercase());
        url = rest;
    }
    let netloc = if let Some(rest) = url.strip_prefix("//") {
        let (authority, remainder) =
            rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
        url = remainder;
        check_netloc(authority)?;
        authority
    } else {
        ""
    };
    let (url, fragment) = if fragments {
        url.split_once('#').unwrap_or((url, ""))
    } else {
        (url, "")
    };
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    Ok([
        scheme,
        netloc.into(),
        path.into(),
        query.into(),
        fragment.into(),
    ])
}

/// # Safety
/// Both input pointers match their flags and stay rooted. The output is a
/// zero-initialized five-field record; its caller owns any fields written before an error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_split(
    url: *const c_void,
    is_bytes: i32,
    scheme: *const c_void,
    scheme_bytes: i32,
    fragments: i32,
    out: *mut usize,
) {
    let (is_bytes, scheme_bytes) = (is_bytes != 0, scheme_bytes != 0);
    finish(
        (|| {
            if is_bytes != scheme_bytes && !unsafe { data(scheme, scheme_bytes) }.is_empty() {
                return Err(Error::new(
                    Kind::TypeError,
                    "Cannot mix str and non-str arguments",
                ));
            }
            let url = clean(unsafe { decoded(url, is_bytes)? }, false);
            if !is_bytes && scheme_bytes {
                return Err(Error::new(
                    Kind::TypeError,
                    "a bytes-like object is required, not 'str'",
                ));
            }
            let scheme = clean(unsafe { decoded(scheme, scheme_bytes)? }, true);
            for (i, field) in split(&url, &scheme, fragments != 0)?
                .into_iter()
                .enumerate()
            {
                unsafe { out.add(i).write(owned(&field, is_bytes)?) };
            }
            Ok(())
        })(),
        (),
    );
}

fn port(word: &[u8], is_bytes: bool) -> Result<usize> {
    if !word.iter().all(u8::is_ascii_digit) {
        let mut repr = String::new();
        if is_bytes {
            bytes::write_repr(&mut repr, word);
        } else {
            repr = string_repr(std::str::from_utf8(word).unwrap()).to_string();
        }
        return Err(invalid(format!(
            "Port could not be cast to integer value as {repr}"
        )));
    }
    let number = std::str::from_utf8(word).unwrap().trim_start_matches('0');
    if word.len() > 4300 {
        return Err(invalid(format!(
            "Exceeds the limit (4300 digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
            word.len()
        )));
    }
    let n = if number.is_empty() {
        0
    } else {
        number
            .parse::<u16>()
            .map_err(|_| invalid("Port out of range 0-65535"))?
    };
    Ok(Int::small(i64::from(n)).unwrap().into_raw())
}

fn partition(input: &[u8], sep: u8) -> (&[u8], Option<&[u8]>) {
    input
        .iter()
        .position(|&b| b == sep)
        .map_or((input, None), |i| (&input[..i], Some(&input[i + 1..])))
}

/// # Safety
/// The authority is rooted and has the layout selected by the flag. The optional
/// result is a nullable owned string/bytes pointer or a tagged port integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_property(
    netloc: *const c_void,
    is_bytes: i32,
    property: u32,
) -> usize {
    let is_bytes = is_bytes != 0;
    let netloc = unsafe { data(netloc, is_bytes) };
    finish(
        (|| {
            let (info, host) = netloc
                .iter()
                .rposition(|&b| b == b'@')
                .map_or((None, netloc), |i| (Some(&netloc[..i]), &netloc[i + 1..]));
            let value = match property {
                0 | 1 => info.and_then(|info| {
                    let (user, pass) = partition(info, b':');
                    if property == 0 { Some(user) } else { pass }
                }),
                2 | 3 => {
                    let (hostname, port_bytes) = if let Some(bracketed) = partition(host, b'[').1 {
                        let (name, suffix) = partition(bracketed, b']');
                        (name, suffix.and_then(|s| partition(s, b':').1))
                    } else {
                        partition(host, b':')
                    };
                    if property == 3 {
                        return port_bytes
                            .filter(|p| !p.is_empty())
                            .map_or(Ok(0), |p| port(p, is_bytes));
                    }
                    if hostname.is_empty() {
                        return Ok(0);
                    }
                    let (name, zone) = partition(hostname, b'%');
                    if !is_bytes {
                        let name = std::str::from_utf8(name).unwrap();
                        let mut lower = name.to_lowercase();
                        if let Some(zone) = zone {
                            lower.push('%');
                            lower.push_str(std::str::from_utf8(zone).unwrap());
                        }
                        return owned(&lower, false);
                    }
                    let mut lower = hostname.to_vec();
                    lower[..name.len()].make_ascii_lowercase();
                    return Ok(BytesOwner::into_raw(bytes::owned_input(&lower)?) as usize);
                }
                _ => unreachable!(),
            };
            match value {
                None => Ok(0),
                Some(v) if is_bytes => Ok(BytesOwner::into_raw(bytes::owned_input(v)?) as usize),
                Some(v) => owned(std::str::from_utf8(v).unwrap(), false),
            }
        })(),
        0,
    )
}

fn uses_netloc(scheme: &str) -> bool {
    matches!(
        scheme,
        "ftp"
            | "http"
            | "gopher"
            | "nntp"
            | "telnet"
            | "imap"
            | "wais"
            | "file"
            | "mms"
            | "https"
            | "shttp"
            | "snews"
            | "prospero"
            | "rtsp"
            | "rtsps"
            | "rtspu"
            | "rsync"
            | "svn"
            | "svn+ssh"
            | "sftp"
            | "nfs"
            | "git"
            | "git+ssh"
            | "ws"
            | "wss"
            | "itms-services"
    )
}

/// # Safety
/// The rooted record has five fields of the same string/bytes layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_geturl(
    result: *const *const c_void,
    is_bytes: i32,
) -> *const c_void {
    let is_bytes = is_bytes != 0;
    finish(
        (|| {
            let mut fields = [""; 5];
            for (i, field) in fields.iter_mut().enumerate() {
                *field = unsafe { decoded(*result.add(i), is_bytes)? };
            }
            let [scheme, netloc, path, query, fragment] = fields;
            let mut text = String::new();
            if !scheme.is_empty() {
                text.push_str(scheme);
                text.push(':');
            }
            if !netloc.is_empty()
                || (uses_netloc(scheme) && (path.is_empty() || path.starts_with('/')))
            {
                text.push_str("//");
                text.push_str(netloc);
                if !path.is_empty() && !path.starts_with('/') {
                    text.push('/');
                }
            } else if path.starts_with("//") {
                text.push_str("//");
            }
            text.push_str(path);
            if !query.is_empty() {
                text.push('?');
                text.push_str(query);
            }
            if !fragment.is_empty() {
                text.push('#');
                text.push_str(fragment);
            }
            Ok(owned(&text, is_bytes)? as *const c_void)
        })(),
        std::ptr::null(),
    )
}
