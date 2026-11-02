use super::Capture;

/// Encode a literal replacement as UTF-8 runs and capture indices. The high
/// bit distinguishes a group from a byte count, so user text needs no sentinel.
pub fn replacement(template: &str, groups: &[Capture]) -> Result<Vec<u8>, &'static str> {
    let parts = parse_replacement(template, groups.len(), 0, |name| {
        groups.iter().position(|g| g.name.as_deref() == Some(name))
    })
    .map_err(|error| match error {
        ReplacementError::Syntax { compiler, .. } => compiler,
        ReplacementError::GroupName { .. } | ReplacementError::DigitLimit { .. } => {
            "unknown replacement group"
        }
    })?;
    let mut out = Vec::new();
    for part in parts {
        match part {
            ReplacementPart::Literal(text) => {
                let n = u32::try_from(text.len())
                    .ok()
                    .filter(|&n| n < 1 << 31)
                    .ok_or("replacement literal is too long")?;
                if n != 0 {
                    out.extend(n.to_le_bytes());
                    out.extend_from_slice(text.as_bytes());
                }
            }
            ReplacementPart::Group(index) => out.extend((index as u32 | 1 << 31).to_le_bytes()),
        }
    }
    Ok(out)
}

/// A replacement contains literal runs interspersed with public capture indices.
pub enum ReplacementPart<T = String> {
    Literal(T),
    Group(usize),
}

pub enum ReplacementError {
    DigitLimit {
        count: usize,
    },
    Syntax {
        compiler: &'static str,
        message: String,
        position: usize,
    },
    GroupName {
        name: String,
        position: usize,
    },
}

// The replacement tokenizer reads one token ahead. A trailing backslash can
// therefore raise before an invalid group or escape immediately preceding it.
struct Tokens<'a> {
    chars: std::iter::Enumerate<std::str::Chars<'a>>,
    next: Option<(usize, char, bool)>,
}

impl Tokens<'_> {
    fn advance(&mut self) -> Result<Option<(usize, char, bool)>, ReplacementError> {
        let next = match self.chars.next() {
            Some((position, '\\')) => {
                let (_, ch) = self.chars.next().ok_or_else(|| ReplacementError::Syntax {
                    compiler: "incomplete replacement escape",
                    message: "bad escape (end of pattern)".into(),
                    position,
                })?;
                Some((position, ch, true))
            }
            Some((position, ch)) => Some((position, ch, false)),
            None => None,
        };
        Ok(std::mem::replace(&mut self.next, next))
    }

    fn take(
        &mut self,
        predicate: impl FnOnce(char) -> bool,
    ) -> Result<Option<char>, ReplacementError> {
        if let Some((_, ch, false)) = self.next
            && predicate(ch)
        {
            self.advance()?;
            Ok(Some(ch))
        } else {
            Ok(None)
        }
    }
}

/// Parse text or Latin-1-decoded bytes. Offsets count characters in the original template.
pub fn parse_replacement(
    template: &str,
    groups: usize,
    digit_limit: u32,
    named_group: impl Fn(&str) -> Option<usize>,
) -> Result<Vec<ReplacementPart>, ReplacementError> {
    let syntax = |position, compiler, message| ReplacementError::Syntax {
        compiler,
        message,
        position,
    };
    let numeric_group = |digits: &str, start| {
        if digit_limit != 0 && digits.len() > digit_limit as usize {
            return Err(ReplacementError::DigitLimit {
                count: digits.len(),
            });
        }
        let digits = digits.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        digits.parse().ok().filter(|&i| i < groups).ok_or_else(|| {
            syntax(
                start,
                "unknown replacement group",
                format!("invalid group reference {digits}"),
            )
        })
    };
    let mut tokens = Tokens {
        chars: template.chars().enumerate(),
        next: None,
    };
    tokens.advance()?;
    let mut parts = Vec::new();
    let mut literal = String::new();
    while let Some((pos, escape, escaped)) = tokens.advance()? {
        if !escaped {
            literal.push(escape);
            continue;
        }
        let group = match escape {
            'g' => {
                if tokens.take(|ch| ch == '<')?.is_none() {
                    return Err(syntax(
                        pos + 2,
                        "expected < in replacement group",
                        "missing <".into(),
                    ));
                }
                let start = pos + 3;
                let mut name = String::new();
                loop {
                    match tokens.advance()? {
                        Some((_, '>', false)) if !name.is_empty() => break,
                        Some((_, '>', false)) | None if name.is_empty() => {
                            return Err(syntax(
                                start,
                                "unknown replacement group",
                                "missing group name".into(),
                            ));
                        }
                        Some((_, ch, escaped)) => {
                            if escaped {
                                name.push('\\');
                            }
                            name.push(ch);
                        }
                        None => {
                            return Err(syntax(
                                start,
                                "unterminated replacement group",
                                "missing >, unterminated name".into(),
                            ));
                        }
                    }
                }
                if name.bytes().all(|ch| ch.is_ascii_digit()) {
                    numeric_group(&name, start)?
                } else {
                    named_group(&name).filter(|&index| index < groups).ok_or(
                        ReplacementError::GroupName {
                            name,
                            position: start,
                        },
                    )?
                }
            }
            '1'..='9' => {
                let mut digits = String::from(escape);
                if let Some(c) = tokens.take(|ch| ch.is_ascii_digit())? {
                    digits.push(c);
                    if digits.bytes().all(|d| (b'0'..=b'7').contains(&d))
                        && let Some(c) = tokens.take(|ch| matches!(ch, '0'..='7'))?
                    {
                        digits.push(c);
                        let octal = u8::from_str_radix(&digits, 8).map_err(|_| {
                            syntax(
                                pos,
                                "octal escape exceeds 0o377",
                                format!("octal escape value \\{digits} outside of range 0-0o377"),
                            )
                        })?;
                        literal.push(char::from(octal));
                        continue;
                    }
                }
                numeric_group(&digits, pos + 1)?
            }
            '0' => {
                let mut octal = 0;
                for _ in 0..2 {
                    if let Some(c) = tokens.take(|ch| matches!(ch, '0'..='7'))? {
                        octal = octal * 8 + (c as u8 - b'0');
                    }
                }
                literal.push(char::from(octal));
                continue;
            }
            _ => {
                let decoded = match escape {
                    '\\' => '\\',
                    'a' => '\x07',
                    'b' => '\x08',
                    'f' => '\x0c',
                    'v' => '\x0b',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    c if c.is_ascii_alphabetic() => {
                        return Err(syntax(
                            pos,
                            "invalid replacement escape",
                            format!("bad escape \\{c}"),
                        ));
                    }
                    c => {
                        literal.push('\\');
                        c
                    }
                };
                literal.push(decoded);
                continue;
            }
        };
        if parts.is_empty() || !literal.is_empty() {
            parts.push(ReplacementPart::Literal(std::mem::take(&mut literal)));
        }
        parts.push(ReplacementPart::Group(group));
    }
    if parts.is_empty() || !literal.is_empty() {
        parts.push(ReplacementPart::Literal(literal));
    }
    Ok(parts)
}
