//! Shared by typed and boxed string.Formatter cursors. Delimiters are ASCII.
use std::ops::Range;

pub(super) type Result<T> = std::result::Result<T, &'static str>;

pub(super) struct Markup {
    pub(super) literal: Range<usize>,
    pub(super) field: Option<Range<usize>>,
    pub(super) spec: Option<Range<usize>>,
    pub(super) conversion: Option<Range<usize>>,
}

impl Markup {
    fn literal(span: Range<usize>) -> Self {
        Self {
            literal: span,
            field: None,
            spec: None,
            conversion: None,
        }
    }
}

fn bump(text: &str, pos: &mut usize) -> Option<char> {
    let c = text[*pos..].chars().next()?;
    *pos += c.len_utf8();
    Some(c)
}

/// A doubled brace ends the current literal row. A field and its preceding
/// literal form one row, even when the literal or the field name is empty.
pub(super) fn markup(text: &str, pos: &mut usize) -> Result<Option<Markup>> {
    let begin = *pos;
    if begin == text.len() {
        return Ok(None);
    }
    let Some(offset) = text[begin..].find(['{', '}']) else {
        *pos = text.len();
        return Ok(Some(Markup::literal(begin..*pos)));
    };
    let brace = begin + offset;
    let kind = text.as_bytes()[brace];
    *pos = brace + 1;
    if text.as_bytes().get(*pos) == Some(&kind) {
        *pos += 1;
        return Ok(Some(Markup::literal(begin..brace + 1)));
    }
    if kind == b'}' {
        return Err("Single '}' encountered in format string");
    }
    if *pos == text.len() {
        return Err("Single '{' encountered in format string");
    }
    let mut row = Markup::literal(begin..brace);
    let field = *pos;
    let delimiter = loop {
        let index = *pos;
        match bump(text, pos) {
            Some('{') => return Err("unexpected '{' in field name"),
            Some('[') => {
                // Delimiters inside a bracketed item do not close the field.
                *pos = text[*pos..]
                    .find(']')
                    .map_or(text.len(), |end| *pos + end + 1);
            }
            Some(c @ ('!' | ':' | '}')) => {
                row.field = Some(field..index);
                row.spec = Some(index..index);
                break c;
            }
            None => return Err("expected '}' before end of string"),
            _ => {}
        }
    };
    if delimiter == '}' {
        return Ok(Some(row));
    }
    if delimiter == '!' {
        let start = *pos;
        let conversion =
            bump(text, pos).ok_or("end of string while looking for conversion specifier")?;
        if conversion != '\0' {
            row.conversion = Some(start..*pos);
        }
        match bump(text, pos) {
            Some('}') => return Ok(Some(row)),
            Some(':') | None => {}
            _ => return Err("expected ':' after conversion specifier"),
        }
    }
    let spec = *pos;
    let mut depth = 1usize;
    while let Some(offset) = text[*pos..].find(['{', '}']) {
        let index = *pos + offset;
        *pos = index + 1;
        if text.as_bytes()[index] == b'{' {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                row.spec = Some(spec..index);
                return Ok(Some(row));
            }
        }
    }
    *pos = text.len();
    Err("unmatched '{' in format spec")
}

pub(super) enum Key {
    Integer(usize),
    Text(Range<usize>),
}

fn key(text: &str, span: Range<usize>) -> Result<Key> {
    if !span.is_empty() {
        let mut value = 0isize;
        for c in text[span.clone()].chars() {
            let Some(digit) = crate::integer_text::decimal_digit(c) else {
                return Ok(Key::Text(span));
            };
            value = value
                .checked_mul(10)
                .and_then(|n| n.checked_add(isize::from(digit)))
                .ok_or("Too many decimal digits in format string")?;
        }
        return Ok(Key::Integer(value as usize));
    }
    Ok(Key::Text(span))
}

pub(super) fn first(text: &str) -> Result<(Key, usize)> {
    let end = text.find(['.', '[']).unwrap_or(text.len());
    Ok((key(text, 0..end)?, end))
}

/// On a malformed segment the cursor stays at the position reached by that failure.
pub(super) fn field(text: &str, pos: &mut usize) -> Result<Option<(bool, Key)>> {
    let Some(c) = bump(text, pos) else {
        return Ok(None);
    };
    let start = *pos;
    let (end, attribute) = match c {
        '.' => {
            *pos += text[start..].find(['.', '[']).unwrap_or(text.len() - start);
            (*pos, true)
        }
        '[' => {
            let Some(offset) = text[start..].find(']') else {
                *pos = text.len();
                return Err("Missing ']' in format string");
            };
            *pos = start + offset + 1;
            (start + offset, false)
        }
        _ => return Err("Only '.' or '[' may follow ']' in format field specifier"),
    };
    if start == end {
        return Err("Empty attribute in format string");
    }
    let item = if attribute {
        Key::Text(start..end)
    } else {
        key(text, start..end)?
    };
    Ok(Some((attribute, item)))
}
