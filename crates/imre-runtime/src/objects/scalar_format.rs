//! Shared scalar formatting for native words and registered Python format methods.
use super::{brace_float_field, brace_integer_field, words::WordKind};
use crate::{Error, Int, Kind, Result, Str, StrOwner, format_grammar::FormatPrefix};

struct FixedFloatSpec {
    sign: u8,
    width: usize,
    zero: bool,
    precision: u32,
    percent: bool,
}

impl FixedFloatSpec {
    /// Parse the supported sign, decimal width and bounded precision for f and %.
    fn parse(spec: &str) -> Result<Option<Self>> {
        let Some(prefix) = spec.strip_suffix(['f', '%']) else {
            return Ok(None);
        };
        let (digits, sign) = match prefix.as_bytes().first() {
            Some(&sign @ (b'+' | b' ')) => (&prefix[1..], sign),
            Some(b'-') => (&prefix[1..], 0),
            _ => (prefix, 0),
        };
        let (width, places) = digits.split_once('.').unwrap_or((digits, "6"));
        let Ok(precision) = places.parse::<u32>() else {
            return Ok(None);
        };
        if precision > 1000
            || !width
                .bytes()
                .chain(places.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Ok(None);
        }
        let zero = width.starts_with('0');
        let width = if width.is_empty() {
            0
        } else {
            width.parse::<isize>().map_err(|_| {
                Error::new(Kind::ValueError, "Too many decimal digits in format string")
            })? as usize
        };
        Ok(Some(Self {
            sign,
            width,
            zero,
            precision,
            percent: spec.ends_with('%'),
        }))
    }

    /// Pad the rounded ASCII result, keeping a numeric sign ahead of zero fill.
    fn render(&self, number: f64) -> Result<String> {
        let text =
            crate::objects::format_fixed_float(number, self.precision, self.percent, self.sign)?;
        if self.width <= text.len() {
            return Ok(text.into_string());
        }
        let head = usize::from(self.zero && text.starts_with(['+', '-', ' ']));
        let mut padded = String::new();
        padded
            .try_reserve_exact(self.width)
            .map_err(|_| Error::memory_error())?;
        padded.push_str(&text[..head]);
        padded.extend(std::iter::repeat_n(
            if self.zero { '0' } else { ' ' },
            self.width - text.len(),
        ));
        padded.push_str(&text[head..]);
        Ok(padded)
    }
}

/// Format a finite or nonfinite binary64 value after any Python method dispatch.
pub(crate) fn float_field(spec: &str, value: f64, type_name: &str) -> Result<String> {
    match FixedFloatSpec::parse(spec)? {
        Some(field) => field.render(value),
        None => brace_float_field(spec, value, type_name),
    }
}

/// Integer and boolean formatting share decimal limits and checked float conversion.
pub(crate) fn integer_field(spec: &str, value: Int, type_name: &str) -> Result<String> {
    match FixedFloatSpec::parse(spec)? {
        Some(field) => field.render(crate::floats::int_to_float(&value)?),
        None => brace_integer_field(spec, value, type_name),
    }
}

/// Parse one string specification, retaining the special diagnostic for a surrogate fill.
pub(crate) fn string_field<'a>(
    spec: &'a str,
    scalar_fill: bool,
    type_name: &str,
) -> Result<(FormatPrefix<'a>, Option<usize>)> {
    let field =
        FormatPrefix::parse(spec, '<', true).map_err(|e| Error::new(Kind::ValueError, e))?;
    let precision = field.string_precision(scalar_fill.then_some(spec), type_name)?;
    Ok((field, precision))
}

/// Pad UTF-8 text by character count, reserving its exact byte size before writing.
pub(crate) fn padded_text(text: &str, fill: char, left: usize, right: usize) -> Result<Str> {
    let size = left
        .checked_add(right)
        .and_then(|n| n.checked_mul(fill.len_utf8()))
        .and_then(|padding| padding.checked_add(text.len()))
        .ok_or_else(Error::memory_error)?;
    let mut result = String::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| Error::memory_error())?;
    result.extend(std::iter::repeat_n(fill, left));
    result.push_str(text);
    result.extend(std::iter::repeat_n(fill, right));
    Ok(result.into())
}

/// Render a parsed field by Unicode code points. An unchanged result keeps its original owner.
pub(crate) fn utf8_field(
    text: &str,
    field: &FormatPrefix<'_>,
    precision: Option<usize>,
) -> Result<Option<Str>> {
    let end = precision
        .and_then(|n| text.char_indices().nth(n))
        .map_or(text.len(), |(i, _)| i);
    let prefix = &text[..end];
    let margin = if field.width <= prefix.len().div_ceil(4) {
        0
    } else {
        field.width.saturating_sub(prefix.chars().count())
    };
    if margin == 0 && end == text.len() {
        return Ok(None);
    }
    let left = match field.align {
        Some('>') => margin,
        Some('^') => margin / 2,
        _ => 0,
    };
    padded_text(prefix, field.fill, left, margin - left).map(Some)
}

/// The inherited object.__format__ method checks the spec before calling str(value).
pub(crate) fn check_object_spec(spec: &str, type_name: &str) -> Result<()> {
    if spec.is_empty() {
        Ok(())
    } else {
        Err(Error::new(
            Kind::TypeError,
            format!("unsupported format string passed to {type_name}.__format__"),
        ))
    }
}

/// Apply the inherited object.__format__ to a native value with checked str semantics.
///
/// # Safety
/// The compiler proves the value inherits object.__format__ and has a supported
/// Python display. The caller roots the concrete word and spec through the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_format_object(
    kind: u32,
    word: usize,
    spec: *const Str,
) -> *const Str {
    // SAFETY: the caller roots the word, including its payload when nullable.
    let (kind, word) = unsafe { super::boxed_values::concrete(WordKind::new(kind), word) };
    // SAFETY: the caller roots both the native string and its concrete operand.
    let (spec, name) = unsafe {
        (
            (&*spec).as_str(),
            super::exceptions::word_type_name(kind, word),
        )
    };
    if let Err(error) = check_object_spec(spec, &name) {
        return error.returning(std::ptr::null());
    }
    // SAFETY: both inputs remain live through the existing Python str operation.
    unsafe { super::boxed_values::checked_text(kind, word, false) }
}

/// Format one exact native scalar with a runtime-selected UTF-8 specification.
///
/// # Safety
/// The nonnullable word matches the compiler's bool, int, str, float or bytes descriptor.
/// Both inputs stay rooted for the call. The caller owns the returned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_format_scalar(
    kind: u32,
    word: usize,
    spec: *const Str,
) -> *const Str {
    // SAFETY: the compiler supplies a rooted native string.
    let spec = unsafe { &*spec }.as_str();
    crate::finish(
        (|| {
            let text = match WordKind::new(kind).raw() {
                1 if spec.is_empty() => Str::from(if word == 0 { "False" } else { "True" }),
                1 => integer_field(spec, Int::from(word as i64), "bool")?.into(),
                2 => {
                    // SAFETY: kind 2 carries a borrowed tagged integer.
                    let integer = unsafe { Int::borrow(word) };
                    if spec.is_empty() {
                        crate::integer_text::decimal_string(&integer)?
                    } else {
                        integer_field(spec, (*integer).clone(), "int")?.into()
                    }
                }
                3 => {
                    // SAFETY: kind 3 roots this immutable UTF-8 string throughout the call.
                    let value = unsafe { &*(word as *const Str) };
                    let (field, precision) = string_field(spec, true, "str")?;
                    match utf8_field(value, &field, precision)? {
                        Some(text) => text,
                        None => {
                            // SAFETY: transfer an additional owner of the unchanged input.
                            let owner = unsafe { StrOwner::acquire_word(word) };
                            return Ok(StrOwner::into_raw(owner));
                        }
                    }
                }
                7 => {
                    let value = f64::from_bits(word as u64);
                    if spec.is_empty() {
                        crate::floats::render(value)
                    } else {
                        float_field(spec, value, "float")?.into()
                    }
                }
                8 => {
                    check_object_spec(spec, "bytes")?;
                    // SAFETY: kind 8 roots exact immutable bytes for this call.
                    super::bytes::repr(unsafe { &*(word as *const crate::Bytes) })?
                }
                _ => return Err(Error::new(Kind::SystemError, "invalid format operand kind")),
            };
            crate::checked_owned_str(text)
        })(),
        std::ptr::null(),
    )
}
