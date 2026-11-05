//! Shared by typed and boxed string.Formatter cursors. Delimiters are ASCII.
use std::ops::Range;

pub(super) type Result<T> = std::result::Result<T, &'static str>;

/// Consume an optional fill character and alignment at the start of a format specifier.
pub(super) fn alignment(spec: &str) -> (Option<char>, Option<char>, &str) {
    let mut chars = spec.chars();
    let first = chars.next();
    if let Some(align) = chars.next().filter(|c| "<^>=".contains(*c)) {
        (first, Some(align), chars.as_str())
    } else if let Some(align) = first.filter(|c| "<^>=".contains(*c)) {
        (None, Some(align), &spec[1..])
    } else {
        (None, None, spec)
    }
}

/// Read Unicode decimal digits up to the next token, checking the signed field-size limit.
pub(super) fn decimal(tail: &mut &str) -> Result<Option<usize>> {
    let mut value = 0isize;
    let mut end = 0;
    for (i, c) in tail.char_indices() {
        let Some(digit) = crate::integer_text::decimal_digit(c) else {
            break;
        };
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(isize::from(digit)))
            .ok_or("Too many decimal digits in format string")?;
        end = i + c.len_utf8();
    }
    *tail = &tail[end..];
    Ok((end != 0).then_some(value as usize))
}

fn grouping(tail: &mut &str) -> Result<Option<char>> {
    let Some(sep) = tail.chars().next().filter(|c| matches!(c, ',' | '_')) else {
        return Ok(None);
    };
    *tail = &tail[1..];
    if tail.starts_with(if sep == ',' { '_' } else { ',' }) {
        return Err("Cannot specify both ',' and '_'.");
    }
    Ok(Some(sep))
}

/// The digits after a point are optional when a fractional grouping separator follows.
pub(super) fn precision(tail: &mut &str) -> Result<(Option<usize>, Option<char>)> {
    let Some(rest) = tail.strip_prefix('.') else {
        return Ok((None, None));
    };
    *tail = rest;
    let digits = decimal(tail)?;
    let separator = grouping(tail)?;
    if digits.is_none() && separator.is_none() {
        return Err("Format specifier missing precision");
    }
    Ok((digits, separator))
}

/// Common lexical prefix for numeric and string format specifications.
pub(super) struct FormatPrefix<'a> {
    pub(super) fill: char,
    pub(super) align: Option<char>,
    pub(super) sign: Option<char>,
    pub(super) negative_zero: bool,
    pub(super) alternate: bool,
    pub(super) width: usize,
    pub(super) group: Option<char>,
    pub(super) tail: &'a str,
}

impl<'a> FormatPrefix<'a> {
    pub(super) fn parse(spec: &'a str, zero_align: char, parse_z: bool) -> Result<Self> {
        let (mut fill, mut align, mut tail) = alignment(spec);
        let sign = tail.chars().next().filter(|c| matches!(c, '+' | '-' | ' '));
        if sign.is_some() {
            tail = &tail[1..];
        }
        let negative_zero = parse_z && tail.starts_with('z');
        if negative_zero {
            tail = &tail[1..];
        }
        let alternate = tail.starts_with('#');
        if alternate {
            tail = &tail[1..];
        }
        if fill.is_none()
            && let Some(rest) = tail.strip_prefix('0')
        {
            fill = Some('0');
            align.get_or_insert(zero_align);
            tail = rest;
        }
        let width = decimal(&mut tail)?.unwrap_or(0);
        let group = grouping(&mut tail)?;
        Ok(Self {
            fill: fill.unwrap_or(' '),
            align,
            sign,
            negative_zero,
            alternate,
            width,
            group,
            tail,
        })
    }

    /// Read the presentation code before applying the string-specific flag restrictions.
    pub(super) fn string_precision(
        &self,
        spec: Option<&str>,
        type_name: &str,
    ) -> crate::Result<Option<usize>> {
        use crate::{Error, Kind};
        let error = |message| Error::new(Kind::ValueError, message);
        let mut tail = self.tail;
        let (precision, frac_group) = precision(&mut tail).map_err(error)?;
        let mut chars = tail.chars();
        let code = chars.next().unwrap_or('s');
        if chars.next().is_some() {
            let spec = spec.ok_or_else(|| {
                Error::new(
                    Kind::UnicodeError,
                    "operation requires Unicode scalar values",
                )
            })?;
            return Err(Error::new(
                Kind::ValueError,
                format!("Invalid format specifier '{spec}' for object of type '{type_name}'"),
            ));
        }
        let code_name = || {
            if ('!'..='\x7f').contains(&code) {
                code.to_string()
            } else {
                format!("\\x{:x}", code as u32)
            }
        };
        if let Some(sep) = self
            .group
            .filter(|&sep| !"defgEG%F\0".contains(code) && !(sep == '_' && "boxX".contains(code)))
            .or(frac_group.filter(|_| code == 'n'))
        {
            return Err(Error::new(
                Kind::ValueError,
                format!("Cannot specify '{sep}' with '{}'.", code_name()),
            ));
        }
        if code != 's' {
            return Err(Error::new(
                Kind::ValueError,
                format!(
                    "Unknown format code '{}' for object of type '{type_name}'",
                    code_name()
                ),
            ));
        }
        let invalid = match (self.sign, self.negative_zero, self.alternate, self.align) {
            (Some(' '), ..) => "Space not allowed in string format specifier",
            (Some(_), ..) => "Sign not allowed in string format specifier",
            (_, true, ..) => "Negative zero coercion (z) not allowed in string format specifier",
            (_, _, true, _) => "Alternate form (#) not allowed in string format specifier",
            (_, _, _, Some('=')) => "'=' alignment not allowed in string format specifier",
            _ => return Ok(precision),
        };
        Err(error(invalid))
    }
}

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
    let mut tail = &text[span.clone()];
    if let Some(value) = decimal(&mut tail)?
        && tail.is_empty()
    {
        return Ok(Key::Integer(value));
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
