//! Serde validates JSON proper. Python also accepts three non-finite tokens.
//! On a serde error at such a token, substitute same-width number spellings so
//! every raw value still points to the corresponding span of the original text.

pub(super) fn normalize(text: &str, error: &serde_json::Error) -> Option<String> {
    let line = text.split('\n').nth(error.line().checked_sub(1)?)?;
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
        return None;
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
            let output = output.get_or_insert_with(|| bytes.to_vec());
            output[at..at + token.len()].copy_from_slice(token);
            at += token.len();
        }
    }
    output.map(|bytes| String::from_utf8(bytes).expect("only ASCII bytes were substituted"))
}
