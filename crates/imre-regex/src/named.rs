//! Unicode 16 single-character names used by Python regex escapes.

use super::EscapeError;

/// Consume the operand of a Python regex named escape, committing the iterator only on success.
/// Error offsets count characters from the backslash, including tokenizer lookahead.
pub(super) fn named_scalar(
    input: &mut (impl Iterator<Item = char> + Clone),
    bytes: bool,
) -> Result<char, EscapeError> {
    let syntax = |message: &str, at| EscapeError::Syntax(message.to_owned(), at);

    let eof = |at| syntax("bad escape (end of pattern)", at);
    let mut end = input.clone();
    if end.clone().eq(['\\']) {
        return Err(eof(2));
    }
    if bytes {
        return Err(syntax(r"bad escape \N", 0));
    }
    if end.next() != Some('{') {
        return Err(syntax("missing {", 2));
    }
    let mut name = String::new();
    let mut at = 3;
    while let Some(ch) = end.next() {
        if ch == '}' {
            // getuntil fetches one token beyond the delimiter before checking the name.
            if end.clone().eq(['\\']) {
                return Err(eof(at + 1));
            }
            if name.is_empty() {
                return Err(syntax("missing character name", 3));
            }
            let scalar = if name.is_ascii() && name.len() <= 256 {
                lookup(&name)
            } else {
                None
            };
            let scalar = scalar.ok_or(EscapeError::Undefined(name))?;
            *input = end;
            return Ok(scalar);
        }
        name.push(ch);
        if ch == '\\' {
            name.push(end.next().ok_or_else(|| eof(at))?);
            at += 1;
        }
        at += 1;
    }
    Err(syntax(
        if name.is_empty() {
            "missing character name"
        } else {
            "missing }, unterminated name"
        },
        3,
    ))
}

fn lookup(name: &str) -> Option<char> {
    if let Some((prefix, digits)) = name.split_once('-') {
        // unicode_names2 implements algorithmic CJK and Hangul names. Unicode 16
        // stores Tangut ideograph names as these two ranges in UnicodeData.txt.
        if prefix.eq_ignore_ascii_case("TANGUT IDEOGRAPH") {
            if digits.len() != 5 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let point = u32::from_str_radix(digits, 16).ok()?;
            return match point {
                0x17000..=0x187f7 | 0x18d00..=0x18d08 => char::from_u32(point),
                _ => None,
            };
        }
        if prefix.eq_ignore_ascii_case("CJK UNIFIED IDEOGRAPH") && digits.starts_with('0') {
            return None;
        }
    }
    let ch = unicode_names2::character(name)?;
    if let Some(canonical) = unicode_names2::name(ch) {
        // Version 1.3 can ignore trailing text after a perfect-hash collision.
        // Unicode 16 NameAliases.txt has one correction alias extending its own name.
        let canonical = canonical.to_string();
        if name.len() > canonical.len()
            && name
                .get(..canonical.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&canonical))
            && !name.eq_ignore_ascii_case("MENDE KIKAKUI SYLLABLE M174 MBOO")
        {
            return None;
        }
    }
    Some(ch)
}
