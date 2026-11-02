//! Extract bounded zero-width assertions checked at private NFA capture slots.
use regex_automata::{Anchored, Input, meta::Regex};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Ahead,
    PositiveBehind,
    NegativeBehind,
}

#[derive(Clone)]
pub(super) struct Guard {
    pub(super) regex: Regex,
    pub(super) kind: Kind,
    pub(super) width: usize,
}

impl Guard {
    pub(super) fn matches(&self, input: &Input<'_>, at: usize) -> bool {
        let start = match self.kind {
            Kind::Ahead => Some(at),
            _ => at.checked_sub(self.width),
        };
        // Keep the real prefix and logical end for anchors inside the guard.
        let matched = start.is_some_and(|start| {
            self.regex.is_match(
                input
                    .clone()
                    .span(start..input.end())
                    .anchored(Anchored::Yes),
            )
        });
        matched != (self.kind == Kind::NegativeBehind)
    }
}

pub(super) struct Extracted {
    pub(super) source: String,
    pub(super) guards: Vec<(String, Kind, String)>,
}

/// Replace each assertion with a private empty capture marking its exact match position.
/// Callers compile the guard separately and check its width after applying the pattern flags.
pub(super) fn extract(
    source: &str,
    verbose: bool,
    byte_lookbehind: bool,
) -> Result<Option<Extracted>, String> {
    if !source.contains("(?=") && !(byte_lookbehind && source.contains("(?<")) {
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
                if let Some((_, _, kind)) = active {
                    if !(tail.starts_with(b"(?:")
                        || kind == Kind::NegativeBehind
                            && (!tail.starts_with(b"(?") || tail.starts_with(b"(?P<")))
                    {
                        return Err(match kind {
                            Kind::Ahead => "only capture-free expressions are supported inside a positive lookahead",
                            _ => "unsupported group inside a bytes lookbehind",
                        }.into());
                    }
                } else if tail.starts_with(b"(?=") {
                    active = Some((at, depth, Kind::Ahead));
                } else if byte_lookbehind && tail.starts_with(b"(?<=") {
                    active = Some((at, depth, Kind::PositiveBehind));
                } else if byte_lookbehind && tail.starts_with(b"(?<!") {
                    active = Some((at, depth, Kind::NegativeBehind));
                } else if tail.starts_with(b"(?")
                    && !tail.starts_with(b"(?:")
                    && !tail.starts_with(b"(?P<")
                {
                    // Leave inline flags and other context-dependent groups to the original parser.
                    return Ok(None);
                }
                depth += 1;
                at += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                if let Some((start, level, kind)) = active
                    && depth == level
                {
                    assertions.push((start, at, kind));
                    active = None;
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    if let Some((_, _, kind)) = active {
        return Err(match kind {
            Kind::Ahead => "unterminated positive lookahead",
            _ => "unterminated bytes lookbehind",
        }
        .into());
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
    for (start, end, kind) in assertions {
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
        let prefix = if kind == Kind::Ahead { 3 } else { 4 };
        guards.push((name, kind, source[start + prefix..end].to_owned()));
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
    use super::{Kind, extract};

    #[test]
    fn assertions_ignore_escaped_delimiters_classes_and_verbose_comments() {
        let source = "(?P<_imre_lookahead_0>a)\\(\\?=b[](?=] # (?=c)\\\n (?=d)\n(?=(?:x|[)]))z";
        let extracted = extract(source, true, false).unwrap().unwrap();
        assert_eq!(
            extracted.guards,
            [(
                "_imre_lookahead_1".to_owned(),
                Kind::Ahead,
                "(?:x|[)])".to_owned()
            )]
        );
        assert_eq!(
            extracted.source,
            source.replace("(?=(?:x|[)]))", "(?P<_imre_lookahead_1>)")
        );
    }

    #[test]
    fn unsupported_group_state_stays_with_the_existing_parser() {
        assert!(extract("(?x) # (?=a)", false, false).unwrap().is_none());
        assert!(extract("(?s:(?=.))", false, false).unwrap().is_none());
        assert!(extract("(?=(a))", false, false).is_err());
        assert!(extract("(?=(?=a))", false, false).is_err());
    }
}
