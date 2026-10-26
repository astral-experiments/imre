//! Translate the supported Python lexical forms, then let the native regex parser
//! validate precedence, character ranges, repetition, captures, and engine limits.

use std::fmt::Write;

use regex_syntax::hir::HirKind;

// Python str.isspace(), also used by Unicode `re`, includes four C0 separators
// omitted from the Unicode White_Space property used by the Rust engine.
const SPACE: &str =
    r"\x09-\x0d\x1c-\x20\x85\xa0\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}";

/// The synthetic capture used to implement Python's terminal dollar before a final newline.
pub const FINAL_NEWLINE_ANCHOR: &str = r"(?P<imre.eol>\n)?\z";

/// Translate Python's ASCII character classes inside or outside a bracket expression.
pub fn ascii_class(escape: char) -> Option<&'static str> {
    Some(match escape {
        'd' => "[0-9]",
        'D' => "[^0-9]",
        'w' => "[A-Za-z0-9_]",
        'W' => "[^A-Za-z0-9_]",
        's' => r"[\x09-\x0d\x20]",
        'S' => r"[^\x09-\x0d\x20]",
        _ => return None,
    })
}

/// The engine expression and capture layout corresponding to a Python pattern.
pub struct FixedPattern {
    pub source: String,
    pub may_match_empty: bool,
    pub groups: Vec<Capture>,
}

/// A capture may be unnamed or absent on some successful paths.
pub struct Capture {
    pub name: Option<String>,
    pub required: bool,
}

/// Translate the supported Python pattern grammar without changing search semantics.
pub fn translate(pattern: &str, verbose: bool) -> Result<FixedPattern, String> {
    translate_with_ascii(pattern, verbose, false)
}

/// Translate with the character classes selected by Python's ASCII flag.
pub fn translate_with_ascii(
    pattern: &str,
    verbose: bool,
    ascii: bool,
) -> Result<FixedPattern, String> {
    let mut input = pattern.chars().peekable();
    let mut output = String::new();
    let mut groups = vec![None]; // Group zero is the whole match.
    let mut can_repeat = false;
    let mut may_backtrack = false;
    let mut outer_prefixes = Vec::new();
    while let Some(c) = input.next() {
        match c {
            c if verbose && verbose_space(c) => continue,
            '#' if verbose => {
                // The Python tokenizer consumes a backslash and its next character
                // as one token even inside a comment, including a physical newline.
                while let Some(ch) = input.next() {
                    match ch {
                        '\\' => {
                            input.next().ok_or("incomplete regex escape in comment")?;
                        }
                        '\n' => break,
                        _ => {}
                    }
                }
                continue;
            }
            '(' => {
                outer_prefixes.push(may_backtrack);
                may_backtrack = false;
                output.push('(');
                if input.next_if_eq(&'?').is_some() {
                    if input.next_if_eq(&':').is_some() {
                        output.push_str("?:");
                    } else if input.next_if_eq(&'P').is_some() && input.next_if_eq(&'<').is_some() {
                        let mut name = String::new();
                        loop {
                            match input.next() {
                                Some('>') if !name.is_empty() => break,
                                Some(c)
                                    if c == '_'
                                        || c.is_ascii_alphabetic()
                                        || !name.is_empty() && c.is_ascii_digit() =>
                                {
                                    name.push(c)
                                }
                                _ => return Err("expected an ASCII Python capture name".into()),
                            }
                        }
                        write!(output, "?P<{name}>").unwrap();
                        groups.push(Some(name));
                    } else {
                        return Err("unsupported regex group, assertion, or inline flag".into());
                    }
                } else {
                    groups.push(None);
                }
                can_repeat = false;
            }
            '|' => {
                // regex-syntax factors identical prefixes. Moving a repetition
                // or nested choice before the branch can change the first match.
                // An empty character class is an unreachable branch that keeps
                // these alternatives separate without adding a capture.
                output.push_str(if may_backtrack { r"|\P{any}|" } else { "|" });
                if let Some(outer) = outer_prefixes.last_mut() {
                    *outer = true; // The enclosing prefix now contains a choice.
                }
                may_backtrack = false;
                can_repeat = false;
            }
            ')' => {
                may_backtrack |= outer_prefixes.pop().unwrap_or(false);
                output.push(c);
                can_repeat = true;
            }
            '.' | '^' => {
                output.push(c);
                can_repeat = c == '.';
            }
            '$' if input.peek().is_none() || verbose && verbose_tail(input.clone()) => {
                // Remember a consumed final newline so the runtime can exclude it
                // from group zero. Dots are not valid in Python group names.
                output.push_str(FINAL_NEWLINE_ANCHOR);
                can_repeat = false;
            }
            '$' => return Err("the $ anchor is supported only at the end of a pattern".into()),
            '[' => {
                class(&mut input, &mut output, ascii)?;
                can_repeat = true;
            }
            '\\' => {
                let kind = escaped(&mut input, &mut output, false, ascii)?;
                can_repeat = kind != Escape::Anchor;
            }
            '*' | '+' | '?' | '{' => {
                may_backtrack = true;
                if !can_repeat {
                    return Err("regex quantifier requires a preceding atom".into());
                }
                output.push(c);
                if c == '{' {
                    if input.peek() == Some(&',') {
                        output.push('0');
                    } else if !count(&mut input, &mut output)? {
                        return Err("expected a compact regex repetition count".into());
                    }
                    if input.next_if_eq(&',').is_some() {
                        output.push(',');
                        count(&mut input, &mut output)?;
                    }
                    if input.next_if_eq(&'}').is_none() {
                        return Err("expected a compact regex repetition count".into());
                    }
                    output.push('}');
                }
                if input.next_if_eq(&'?').is_some() {
                    output.push('?');
                }
                // Python requires lazy modifiers to be adjacent and rejects nested
                // or possessive quantifiers in this subset. Verbose stripping must
                // not accidentally turn a separated '?' into a modifier.
                can_repeat = false;
            }
            c => {
                literal(&mut output, c);
                can_repeat = true;
            }
        }
    }
    let hir = regex_syntax::Parser::new()
        .parse(&output)
        .map_err(|e| e.to_string())?;
    let mut required = vec![false; groups.len()];
    required[0] = true;
    let mut nodes = vec![(&hir, true)];
    while let Some((node, always)) = nodes.pop() {
        if let HirKind::Repetition(repeat) = node.kind()
            && repeat.max != Some(1)
            && (repeat.sub.properties().minimum_len() == Some(0)
                || repeat.sub.properties().explicit_captures_len() != 0
                    && !matches!(repeat.sub.kind(), HirKind::Capture(c) if c.sub.properties().explicit_captures_len() == 0))
        {
            return Err("repeating a nullable expression or capture is not supported".into());
        }
        let always = match node.kind() {
            HirKind::Capture(capture) => {
                // The hidden final-newline capture is beyond Python's group count.
                if let Some(required) = required.get_mut(capture.index as usize) {
                    *required = always;
                }
                always
            }
            HirKind::Repetition(repeat) => always && repeat.min > 0,
            HirKind::Alternation(_) => false,
            _ => always,
        };
        nodes.extend(node.kind().subs().iter().map(|sub| (sub, always)));
    }
    let regex = regex::Regex::new(&output).map_err(|e| e.to_string())?;
    if !regex
        .capture_names()
        .filter(|name| *name != Some("imre.eol"))
        .eq(groups.iter().map(Option::as_deref))
    {
        return Err("regex optimization would change the Python capture groups".into());
    }
    Ok(FixedPattern {
        source: output,
        may_match_empty: hir.properties().minimum_len() == Some(0),
        groups: groups
            .into_iter()
            .zip(required)
            .map(|(name, required)| Capture { name, required })
            .collect(),
    })
}

/// Encode a literal replacement as UTF-8 runs and capture indices. The high
/// bit distinguishes a group from a byte count, so user text needs no sentinel.
pub fn replacement(template: &str, groups: &[Capture]) -> Result<Vec<u8>, &'static str> {
    fn flush(out: &mut Vec<u8>, literal: &mut String) -> Result<(), &'static str> {
        if !literal.is_empty() {
            let n = u32::try_from(literal.len())
                .ok()
                .filter(|&n| n < 1 << 31)
                .ok_or("replacement literal is too long")?;
            out.extend(n.to_le_bytes());
            out.extend_from_slice(literal.as_bytes());
            literal.clear();
        }
        Ok(())
    }
    let mut chars = template.chars().peekable();
    let mut out = Vec::new();
    let mut literal = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            literal.push(c);
            continue;
        }
        let escape = chars.next().ok_or("incomplete replacement escape")?;
        let group = match escape {
            'g' => {
                if chars.next() != Some('<') {
                    return Err("expected < in replacement group");
                }
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('>') => break,
                        Some(c) => name.push(c),
                        None => return Err("unterminated replacement group"),
                    }
                }
                if !name.is_empty() && name.bytes().all(|c| c.is_ascii_digit()) {
                    name.parse().ok()
                } else {
                    groups.iter().position(|g| g.name.as_deref() == Some(&name))
                }
            }
            '1'..='9' => {
                let mut digits = String::from(escape);
                if let Some(c) = chars.next_if(char::is_ascii_digit) {
                    digits.push(c);
                    if digits.bytes().all(|d| (b'0'..=b'7').contains(&d))
                        && let Some(c) = chars.next_if(|c| matches!(c, '0'..='7'))
                    {
                        digits.push(c);
                        let octal = u8::from_str_radix(&digits, 8)
                            .map_err(|_| "octal escape exceeds 0o377")?;
                        literal.push(char::from(octal));
                        continue;
                    }
                }
                digits.parse().ok()
            }
            '0' => {
                let mut octal = 0;
                for _ in 0..2 {
                    if let Some(c) = chars.next_if(|c| matches!(c, '0'..='7')) {
                        octal = octal * 8 + c.to_digit(8).unwrap() as u8;
                    } else {
                        break;
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
                    c if c.is_ascii_alphabetic() => return Err("invalid replacement escape"),
                    c => {
                        literal.push('\\');
                        c
                    }
                };
                literal.push(decoded);
                continue;
            }
        };
        let group = group
            .filter(|&i| i < groups.len())
            .ok_or("unknown replacement group")?;
        flush(&mut out, &mut literal)?;
        out.extend((group as u32 | 1 << 31).to_le_bytes());
    }
    flush(&mut out, &mut literal)?;
    Ok(out)
}

type Input<'a> = std::iter::Peekable<std::str::Chars<'a>>;

fn count(input: &mut Input<'_>, output: &mut String) -> Result<bool, String> {
    let digits: String = std::iter::from_fn(|| input.next_if(char::is_ascii_digit)).collect();
    if digits.is_empty() {
        return Ok(false);
    }
    // Python reserves UINT32_MAX to mean unbounded repetition.
    if digits.parse::<u32>().ok().is_none_or(|n| n == u32::MAX) {
        return Err("regex repetition count is too large".into());
    }
    output.push_str(&digits);
    Ok(true)
}

/// A terminal dollar may be followed by ignored layout and comments in VERBOSE mode.
fn verbose_tail(mut input: Input<'_>) -> bool {
    while let Some(c) = input.next() {
        if verbose_space(c) {
            continue;
        }
        if c != '#' {
            return false;
        }
        while let Some(c) = input.next() {
            match c {
                '\\' => {
                    if input.next().is_none() {
                        return false;
                    }
                }
                '\n' => break,
                _ => {}
            }
        }
    }
    true
}

fn verbose_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// Emitting punctuation as a scalar prevents Rust-only syntax from appearing
/// when Python literal characters become adjacent after comment removal.
fn literal(output: &mut String, c: char) {
    if c.is_alphanumeric() || c == '_' {
        output.push(c);
    } else {
        write!(output, "\\x{{{:x}}}", c as u32).unwrap();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Escape {
    Scalar,
    Class,
    Anchor,
}

fn escaped(
    input: &mut Input<'_>,
    output: &mut String,
    in_class: bool,
    ascii: bool,
) -> Result<Escape, String> {
    let c = input.next().ok_or("incomplete regex escape")?;
    if ascii && let Some(class) = ascii_class(c) {
        output.push_str(class);
        return Ok(Escape::Class);
    }
    let value = match c {
        'b' | 'B' if ascii && !in_class => {
            output.push_str(if c == 'b' { r"(?-u:\b)" } else { r"(?-u:\B)" });
            return Ok(Escape::Anchor);
        }
        'w' | 'W' => {
            // Python uses Unicode letters, numbers and the literal underscore.
            // Rust's shorthand additionally includes marks, connector characters,
            // and join controls, all of which Python treats as non-word.
            output.push_str(if c == 'w' {
                r"[\p{L}\p{N}_]"
            } else {
                r"[^\p{L}\p{N}_]"
            });
            return Ok(Escape::Class);
        }
        'd' | 'D' => {
            output.push_str(if c == 'd' { r"\p{Nd}" } else { r"\P{Nd}" });
            return Ok(Escape::Class);
        }
        's' | 'S' => {
            output.push('[');
            if c == 'S' {
                output.push('^');
            }
            output.push_str(SPACE);
            output.push(']');
            return Ok(Escape::Class);
        }
        'A' | 'Z' | 'z' if !in_class => {
            output.push_str(if c == 'A' { r"\A" } else { r"\z" });
            return Ok(Escape::Anchor);
        }
        'a' => '\x07',
        'b' if in_class => '\x08',
        't' => '\t',
        'n' => '\n',
        'v' => '\x0b',
        'f' => '\x0c',
        'r' => '\r',
        'x' | 'u' | 'U' => {
            let digits = match c {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            let mut result = 0;
            for _ in 0..digits {
                let next = input
                    .next()
                    .and_then(|c| c.to_digit(16))
                    .ok_or("invalid hexadecimal regex escape")?;
                result = result * 16 + next;
            }
            char::from_u32(result).ok_or("regex escape is not a Unicode scalar")?
        }
        c if c.is_ascii_punctuation() || verbose_space(c) => c,
        _ => return Err(format!("unsupported Python regex escape \\{c}")),
    };
    literal(output, value);
    Ok(Escape::Scalar)
}

fn class(input: &mut Input<'_>, output: &mut String, ascii: bool) -> Result<(), String> {
    output.push('[');
    if input.next_if_eq(&'^').is_some() {
        output.push('^');
    }
    let mut first = true;
    // Only a standalone atom can begin a range. Once both endpoints are
    // consumed, a following hyphen starts a new atom and is literal.
    let mut left_atom: Option<bool> = None;
    let mut range_pending = false;
    while let Some(c) = input.next() {
        if c == ']' && !first {
            output.push(']');
            return Ok(());
        }
        if c == '[' || matches!(c, '&' | '~' | '|' | '-') && input.peek() == Some(&c) {
            return Err("nested or ambiguous regex character class is not supported".into());
        }
        if c == '-'
            && let Some(scalar) = left_atom
            && input.peek() != Some(&']')
        {
            if !scalar {
                return Err("regex range requires scalar endpoints".into());
            }
            output.push('-');
            left_atom = None;
            range_pending = true;
            continue;
        }
        let scalar = if c == '\\' {
            escaped(input, output, true, ascii)? == Escape::Scalar
        } else {
            literal(output, c);
            true
        };
        if range_pending && !scalar {
            return Err("regex range requires scalar endpoints".into());
        }
        left_atom = (!range_pending).then_some(scalar);
        range_pending = false;
        first = false;
    }
    Err("unclosed regex character class".into())
}

#[cfg(test)]
mod tests {
    use super::translate;
    use regex::Regex;

    #[test]
    fn preserves_verbose_class_contents_and_python_whitespace() {
        let pattern = r"\# \s* E # comment
             (?P<tag>[ #\]]+)";
        let translated = translate(pattern, true).unwrap();
        assert_eq!(
            translated
                .groups
                .iter()
                .map(|g| g.name.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("tag")]
        );
        let regex = Regex::new(&translated.source).unwrap();
        let caps = regex.captures("🌍#\u{1c}\u{85}E #]z").unwrap();
        assert_eq!(&caps["tag"], " #]");
        let regex = Regex::new(&translate(r"[\s][^\s]\S", false).unwrap().source).unwrap();
        assert!(regex.is_match("\u{1f}xé"));
        assert!(!regex.is_match("\u{200b}xé"));
        // Only the six ASCII VERBOSE whitespace characters are ignored.
        assert!(
            Regex::new(&translate("a\u{a0}b", true).unwrap().source)
                .unwrap()
                .is_match("a\u{a0}b")
        );
    }

    #[test]
    fn escaped_newline_inside_verbose_comment_is_still_comment_text() {
        for (slashes, expected) in [(1, "a"), (2, "ab"), (3, "a")] {
            let pattern = format!("a#comment{}{}b", "\\".repeat(slashes), '\n');
            assert_eq!(
                Regex::new(&translate(&pattern, true).unwrap().source)
                    .unwrap()
                    .find("ab")
                    .unwrap()
                    .as_str(),
                expected
            );
        }
        assert!(translate("a#comment\\", true).is_err());
        assert!(translate("(?:){4294967295}", false).is_err());
    }

    #[test]
    fn hyphen_after_a_complete_range_is_literal() {
        for (pattern, expected) in [(r"[a-b-c]", "abc-"), (r"[^a-b-c]", "dx")] {
            let regex = Regex::new(&translate(pattern, false).unwrap().source).unwrap();
            let matched: String = regex.find_iter("abc-dx").map(|m| m.as_str()).collect();
            assert_eq!(matched, expected);
        }
    }

    #[test]
    fn alternatives_keep_priority_over_a_shared_repetition_or_choice() {
        for (pattern, text, expected) in [
            (".*?b|.*?a", "ab", "ab"),
            (".*a|.*b", "ab", "a"),
            ("(?:a|ab)c|(?:a|ab)b", "abc", "abc"),
        ] {
            let compiled = Regex::new(&translate(pattern, false).unwrap().source).unwrap();
            assert_eq!(compiled.find(text).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn rejects_semantics_the_engine_does_not_share() {
        for source in [
            r"a$b",
            r"\1",
            r"\b",
            r"(?=a)",
            r"(?P<a>x)(?P=a)",
            r"(?i:a)",
            r"a*+",
            r"a* ?",
            r"a{1, 3}",
            r"((a)?b)+",
            r"(x?)*",
            r"[a&&b]",
            r"[[a]]",
            r"[\s-x]",
            r"[x-\s]",
            r"\uD800",
            r"(?P<1bad>x)",
        ] {
            assert!(translate(source, true).is_err(), "{source}");
        }
        for source in [
            r"\A(ab|c)(?P<maybe>z)?\Z",
            r"[a-z]{2,4}?",
            r"[\-\]]+",
            r"(?P<empty>x*)?",
            r"\w",
            r"[^\W]",
            r"(a)+",
        ] {
            assert!(translate(source, false).is_ok(), "{source}");
        }
    }
}
