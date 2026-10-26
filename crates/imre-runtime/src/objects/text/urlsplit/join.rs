//! Relative references keep explicit empty query and fragment delimiters.
use super::*;

fn uses_relative(scheme: &str) -> bool {
    matches!(
        scheme,
        "" | "ftp"
            | "http"
            | "gopher"
            | "nntp"
            | "imap"
            | "wais"
            | "file"
            | "https"
            | "shttp"
            | "mms"
            | "prospero"
            | "rtsp"
            | "rtsps"
            | "rtspu"
            | "sftp"
            | "svn"
            | "svn+ssh"
            | "ws"
            | "wss"
    )
}

fn resolve_path(base: &str, target: &str) -> Result<String> {
    let absolute = target.starts_with('/');
    let directory = if absolute {
        None
    } else if base.is_empty() || base.ends_with('/') {
        Some(base)
    } else {
        base.rsplit_once('/').map(|(dir, _)| dir)
    };
    let segments = directory
        .into_iter()
        .flat_map(|p| p.split('/'))
        .chain(target.split('/'));
    let mut segments = segments.enumerate().peekable();
    let mut resolved = Vec::new();
    while let Some((index, segment)) = segments.next() {
        match segment {
            ".." => {
                resolved.pop();
            }
            "." => {}
            "" if !absolute && index != 0 && segments.peek().is_some() => {}
            _ => {
                resolved.try_reserve(1).map_err(|_| Error::allocation())?;
                resolved.push(segment);
            }
        }
    }
    let mut path = String::new();
    let capacity = directory
        .unwrap_or_default()
        .len()
        .checked_add(target.len())
        .and_then(|n| n.checked_add(2))
        .ok_or_else(Error::allocation)?;
    path.try_reserve_exact(capacity)
        .map_err(|_| Error::allocation())?;
    for (index, segment) in resolved.iter().enumerate() {
        if index != 0 {
            path.push('/');
        }
        path.push_str(segment);
    }
    if path.is_empty() || matches!(target.rsplit('/').next(), Some("." | "..")) {
        path.push('/');
    }
    Ok(path)
}

fn join(base: &str, url: &str, fragments: bool) -> Result<String> {
    let clean_base = clean(base, false);
    let clean_url = clean(url, false);
    let base = split(&clean_base, "", fragments)?;
    let target = split(&clean_url, "", fragments)?;
    let scheme = if target.scheme.is_empty() {
        &base.scheme
    } else {
        &target.scheme
    };
    if scheme != &base.scheme || !uses_relative(scheme) {
        let mut unchanged = String::new();
        unchanged
            .try_reserve_exact(url.len())
            .map_err(|_| Error::allocation())?;
        unchanged.push_str(url);
        return Ok(unchanged);
    }
    let mut netloc = target.netloc;
    if scheme.is_empty() || uses_netloc(scheme) {
        if netloc.is_some_and(|n| !n.is_empty()) {
            return unsplit(scheme, netloc, target.path, target.query, target.fragment);
        }
        netloc = base.netloc;
    }
    if target.path.is_empty() {
        let (query, fragment) = if target.query.is_some() {
            (target.query, target.fragment)
        } else {
            (base.query, target.fragment.or(base.fragment))
        };
        return unsplit(scheme, netloc, base.path, query, fragment);
    }
    let path = resolve_path(base.path, target.path)?;
    unsplit(scheme, netloc, &path, target.query, target.fragment)
}

/// # Safety
/// Both arguments are rooted for the call and have the layouts identified by the flags.
/// The returned owner is an untagged pointer word for like layouts, or the
/// compiler's str | bytes union word (low bit set for bytes) for mixed layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_url_join(
    base: *const c_void,
    base_bytes: i32,
    url: *const c_void,
    url_bytes: i32,
    fragments: i32,
) -> usize {
    let (base_bytes, url_bytes) = (base_bytes != 0, url_bytes != 0);
    finish(
        (|| {
            // SAFETY: Both tagged inputs are kept alive by the caller.
            let retained = unsafe {
                if data(base, base_bytes).is_empty() {
                    Some((url, url_bytes))
                } else if data(url, url_bytes).is_empty() {
                    Some((base, base_bytes))
                } else {
                    None
                }
            };
            if let Some((value, bytes)) = retained {
                // SAFETY: The returned reference acquires an independent owner of the selected layout.
                let word = unsafe {
                    if bytes {
                        BytesOwner::into_raw(crate::acquire_bytes(value as usize)) as usize
                    } else {
                        crate::StrOwner::into_raw(crate::StrOwner::acquire_word(value as usize))
                            as usize
                    }
                };
                return Ok(word | usize::from(bytes && base_bytes != url_bytes));
            }
            if base_bytes != url_bytes {
                return Err(Error::new(
                    Kind::TypeError,
                    "Cannot mix str and non-str arguments",
                ));
            }
            // SAFETY: Both nonempty operands now have identical layouts.
            let result = join(
                unsafe { decoded(base, base_bytes)? },
                unsafe { decoded(url, url_bytes)? },
                fragments != 0,
            )?;
            owned(&result, base_bytes)
        })(),
        0,
    )
}
