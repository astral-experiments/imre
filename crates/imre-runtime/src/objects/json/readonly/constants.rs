//! Serde validates JSON proper. Python also accepts three non-finite tokens.
//! On a serde error at such a token, substitute same-width number spellings so
//! every raw value still points to the corresponding span of the original text.

use super::Error;

pub(super) fn normalize(text: &str, error: &serde_json::Error) -> Result<Option<String>, Error> {
    let Some(line) = error
        .line()
        .checked_sub(1)
        .and_then(|n| text.split('\n').nth(n))
    else {
        return Ok(None);
    };
    let column = error.column().saturating_sub(1);
    let bytes = line.as_bytes();
    if ![column.saturating_sub(1), column, column + 1]
        .into_iter()
        .any(|at| {
            bytes
                .get(at..)
                .is_some_and(|s| s.starts_with(b"NaN") || s.starts_with(b"Infinity"))
        })
    {
        return Ok(None);
    }
    let bytes = text.as_bytes();
    let mut output = None;
    let mut at = 0;
    while let Some(n) = memchr::memchr3(b'"', b'N', b'I', &bytes[at..]) {
        at += n;
        if bytes[at] == b'"' {
            at += 1;
            while let Some(n) = memchr::memchr2(b'\\', b'"', &bytes[at..]) {
                at += n + 1;
                if bytes[at - 1] == b'"' {
                    break;
                }
                at += usize::from(at < bytes.len());
            }
        } else {
            let token: &[u8] = if bytes[at..].starts_with(b"NaN") {
                b"0e0"
            } else if bytes[at..].starts_with(b"Infinity") {
                b"0.000e00"
            } else {
                at += 1;
                continue;
            };
            let start = if bytes[at] == b'I' && at > 0 && bytes[at - 1] == b'-' {
                at - 1
            } else {
                at
            };
            // Substitution must not join two invalid tokens into one valid number.
            // In particular, Python accepts -Infinity but rejects -NaN and NaN1.
            let before = start == 0
                || matches!(
                    bytes[start - 1],
                    b'[' | b',' | b':' | b' ' | b'\t' | b'\n' | b'\r'
                );
            let after = bytes
                .get(at + token.len())
                .is_none_or(|b| matches!(b, b']' | b'}' | b',' | b' ' | b'\t' | b'\n' | b'\r'));
            if !before || !after {
                at += token.len();
                continue;
            }
            let output = match &mut output {
                Some(output) => output,
                None => {
                    let mut copy = Vec::new();
                    copy.try_reserve_exact(bytes.len())
                        .map_err(|_| Error::allocation())?;
                    copy.extend_from_slice(bytes);
                    output.insert(copy)
                }
            };
            output[at..at + token.len()].copy_from_slice(token);
            at += token.len();
        }
    }
    Ok(output.map(|bytes| String::from_utf8(bytes).expect("only ASCII bytes were substituted")))
}
