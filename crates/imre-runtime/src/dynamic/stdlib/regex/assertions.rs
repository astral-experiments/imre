//! Separate capture-free positive lookaheads from the expression consumed by the NFA.
use std::collections::HashSet;

pub(super) struct Extracted {
    pub(super) source: String,
    pub(super) guards: Vec<(String, String)>,
}

/// Replace each assertion with a private empty capture marking its exact match position.
/// Callers compile the guard separately and check its width after applying the pattern flags.
pub(super) fn extract(source: &str, verbose: bool) -> Result<Option<Extracted>, String> {
    if !source.contains("(?=") {
        return Ok(None);
    }
    let bytes = source.as_bytes();
    let mut at = 0;
    let mut depth = 0usize;
    let mut active = None;
    let mut assertions = Vec::new();
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'[' => {
                at += 1;
                if bytes.get(at) == Some(&b'^') {
                    at += 1;
                }
                // A closing bracket in the first member position is a literal.
                if bytes.get(at) == Some(&b']') {
                    at += 1;
                }
                at = past_delimiter(bytes, at, b']');
            }
            b'#' if verbose => at = past_delimiter(bytes, at + 1, b'\n'),
            b'(' if bytes[at..].starts_with(b"(?#") => {
                at = past_delimiter(bytes, at + 3, b')');
            }
            b'(' => {
                let tail = &bytes[at..];
                if active.is_some() && !tail.starts_with(b"(?:") {
                    return Err(
                        "only capture-free expressions are supported inside a positive lookahead"
                            .into(),
                    );
                }
                if tail.starts_with(b"(?=") {
                    active = Some((at, depth));
                } else if tail.starts_with(b"(?")
                    && !tail.starts_with(b"(?:")
                    && !tail.starts_with(b"(?P<")
                {
                    // In particular, inline flags can change the meaning of a guard
                    // or start a verbose comment. Leave these to the original parser.
                    return Ok(None);
                }
                depth += 1;
                at += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                if let Some((start, level)) = active
                    && depth == level
                {
                    assertions.push((start, at));
                    active = None;
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    if active.is_some() {
        return Err("unterminated positive lookahead".into());
    }
    if assertions.is_empty() {
        return Ok(None);
    }

    let names: HashSet<_> = source
        .split("(?P<")
        .skip(1)
        .filter_map(|tail| tail.split_once('>').map(|(name, _)| name))
        .collect();
    let mut serial = 0;
    let mut rewritten = String::with_capacity(source.len());
    let mut guards = Vec::with_capacity(assertions.len());
    let mut copied = 0;
    for (start, end) in assertions {
        let name = loop {
            let candidate = format!("_imre_lookahead_{serial}");
            serial += 1;
            if !names.contains(candidate.as_str()) {
                break candidate;
            }
        };
        rewritten.push_str(&source[copied..start]);
        rewritten.push_str("(?P<");
        rewritten.push_str(&name);
        rewritten.push_str(">)");
        guards.push((name, source[start + 3..end].to_owned()));
        copied = end + 1;
    }
    rewritten.push_str(&source[copied..]);
    Ok(Some(Extracted {
        source: rewritten,
        guards,
    }))
}

/// Skip to an unescaped ASCII delimiter in a class or comment.
fn past_delimiter(bytes: &[u8], mut at: usize, delimiter: u8) -> usize {
    while let Some(&byte) = bytes.get(at) {
        at += 1;
        if byte == delimiter {
            break;
        }
        if byte == b'\\' {
            at += 1;
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::extract;

    #[test]
    fn assertions_ignore_escaped_delimiters_classes_and_verbose_comments() {
        let source = "(?P<_imre_lookahead_0>a)\\(\\?=b[](?=] # (?=c)\\\n (?=d)\n(?=(?:x|[)]))z";
        let extracted = extract(source, true).unwrap().unwrap();
        assert_eq!(
            extracted.guards,
            [("_imre_lookahead_1".to_owned(), "(?:x|[)])".to_owned())]
        );
        assert_eq!(
            extracted.source,
            source.replace("(?=(?:x|[)]))", "(?P<_imre_lookahead_1>)")
        );
    }

    #[test]
    fn unsupported_group_state_stays_with_the_existing_parser() {
        assert!(extract("(?x) # (?=a)", false).unwrap().is_none());
        assert!(extract("(?s:(?=.))", false).unwrap().is_none());
        assert!(extract("(?=(a))", false).is_err());
        assert!(extract("(?=(?=a))", false).is_err());
    }
}
