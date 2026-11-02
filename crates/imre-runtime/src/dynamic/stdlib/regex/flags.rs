//! Find CPython's leading flag groups in the original spelling before engine translation.
use super::*;

pub(super) struct Flags {
    pub bits: i64,
    pub skip: usize,
    pub deferred: Option<DeferredError>,
}

pub(super) struct DeferredError {
    pub error: (&'static str, usize),
    pub prefix: Vec<u8>,
}

const PLACEMENT: &str = "global flags not at the start of the expression";

fn bit(c: u8) -> Option<i64> {
    Some(match c {
        b'a' => ASCII,
        b'i' => IGNORECASE,
        b'L' => LOCALE,
        b'm' => MULTILINE,
        b's' => DOTALL,
        b'u' => UNICODE,
        b'x' => VERBOSE,
        _ => return None,
    })
}

// Return just past the delimiter, skipping each escaped byte. A backslash at EOF must
// still reach the regular parser, including inside a verbose comment.
fn delimited(source: &[u8], mut i: usize, delimiter: u8) -> Option<usize> {
    while let Some(&c) = source.get(i) {
        i += 1;
        if c == delimiter {
            return Some(i);
        }
        if c == b'\\' {
            source.get(i)?;
            i += 1;
        }
    }
    (delimiter == b'\n').then_some(i)
}

pub(super) fn parse(
    source: &[u8],
    bits: i64,
    bytes: bool,
) -> std::result::Result<Flags, (&'static str, usize)> {
    let mut result = Flags {
        bits,
        skip: 0,
        deferred: None,
    };
    if !source.windows(2).any(|pair| pair == b"(?") {
        return Ok(result);
    }
    let position = |i| {
        if bytes {
            i
        } else {
            std::str::from_utf8(&source[..i]).unwrap().chars().count()
        }
    };
    let mut verbose = bits & VERBOSE != 0;
    let mut parents = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let leading = i == result.skip;
        let rest = &source[i..];
        let ignored = match rest {
            [b' ' | b'\t' | b'\n' | 11 | 12 | b'\r', ..] if verbose => Some(i + 1),
            [b'#', ..] if verbose => {
                let Some(end) = delimited(source, i + 1, b'\n') else {
                    break;
                };
                Some(end)
            }
            [b'(', b'?', b'#', ..] => {
                let Some(end) = delimited(source, i + 3, b')') else {
                    break;
                };
                Some(end)
            }
            _ => None,
        };
        if let Some(end) = ignored {
            i = end;
            if leading {
                result.skip = end;
            }
            continue;
        }
        match rest[0] {
            b'\\' => i += 2,
            b'[' => {
                let mut start = i + 1 + usize::from(rest.get(1) == Some(&b'^'));
                start += usize::from(source.get(start) == Some(&b']'));
                let Some(end) = delimited(source, start, b']') else {
                    break;
                };
                i = end;
            }
            b')' => {
                let Some(outer) = parents.pop() else { break };
                verbose = outer;
                i += 1;
            }
            b'(' => {
                let open = i;
                i += 1;
                if source.get(i) == Some(&b'?') {
                    let mut j = i + 1;
                    let mut added = 0;
                    let mut removed = 0;
                    let mut invalid = None;
                    while let Some(flag) = source.get(j).and_then(|c| bit(*c)) {
                        j += 1;
                        let message = match flag {
                            LOCALE if !bytes => {
                                Some("bad inline flags: cannot use 'L' flag with a str pattern")
                            }
                            UNICODE if bytes => {
                                Some("bad inline flags: cannot use 'u' flag with a bytes pattern")
                            }
                            ASCII | LOCALE | UNICODE
                                if added & (ASCII | LOCALE | UNICODE) & !flag != 0 =>
                            {
                                Some("bad inline flags: flags 'a', 'u' and 'L' are incompatible")
                            }
                            _ => None,
                        };
                        if let Some(msg) = message {
                            invalid.get_or_insert_with(|| (msg, position(j)));
                        }
                        added |= flag;
                    }
                    let removing = source.get(j) == Some(&b'-');
                    if removing {
                        j += 1;
                        while let Some(flag) = source.get(j).and_then(|c| bit(*c)) {
                            j += 1;
                            if flag & (ASCII | LOCALE | UNICODE) != 0 {
                                invalid.get_or_insert_with(|| {
                                    (
                                        "bad inline flags: cannot turn off flags 'a', 'u' and 'L'",
                                        position(j),
                                    )
                                });
                            }
                            removed |= flag;
                        }
                    }
                    if (added != 0 || removing) && source.get(j) == Some(&b')') {
                        // Removing flags requires both a flag and a scoped body.
                        if removing {
                            let message = if removed == 0 {
                                "missing flag"
                            } else {
                                "missing :"
                            };
                            invalid.get_or_insert_with(|| (message, position(j)));
                        }
                        if leading {
                            if let Some(error) = invalid {
                                return Err(error);
                            }
                            result.bits |= added;
                            verbose = result.bits & VERBOSE != 0;
                            i = j + 1;
                            result.skip = i;
                            continue;
                        }
                        // Check the actual prefix with the ordinary compiler. Closing its open
                        // groups lets errors earlier than this token retain their precedence.
                        let mut prefix = source[result.skip..open].to_vec();
                        prefix.extend(std::iter::repeat_n(b')', parents.len()));
                        result.deferred = Some(DeferredError {
                            error: invalid.unwrap_or_else(|| (PLACEMENT, position(open))),
                            prefix,
                        });
                        break;
                    }
                    if (added | removed) != 0 && source.get(j) == Some(&b':') {
                        parents.push(verbose);
                        verbose = (verbose || added & VERBOSE != 0) && removed & VERBOSE == 0;
                        i = j + 1;
                        continue;
                    }
                }
                parents.push(verbose);
            }
            _ => i += 1,
        }
    }
    Ok(result)
}
