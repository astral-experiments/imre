//! Percent, brace and template formatting over the actual fields of native log records.
use crate::StrOwner;
use std::sync::LazyLock;
use std::time::Duration;

use regex_automata::meta::Regex;
use ruff_python_literal::format::{FormatParseError, FormatPart, FormatString, FromTemplate};

use super::super::exceptions::{Result, string_repr};
use super::super::words::OwnedWord;
use super::super::{Error, Int, Kind, Object, Storage, StorageCell, Str, finish, object};
use super::record::{self, Record, append, display, string, write_display};

static PERCENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)%\(\w+\)[#0+ -]*(\*|\d+)?(\.(\*|\d+))?[diouxefgcrsa%]").unwrap()
});
static FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+|\w+)(\.\w+|\[[^\]]+\])*$").unwrap());
static SPEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(.?[<>=^])?[+ -]?#?0?(\d+|\{\w+\})?[,_]?(\.(\d+|\{\w+\}))?[bcdefgnosx%]?$")
        .unwrap()
});

#[derive(Clone, Copy)]
enum Style {
    Percent,
    Brace,
    Dollar,
}

pub(in crate::objects) struct Formatter {
    style: Style,
    fmt: StrOwner,
    date: Option<StrOwner>,
    defaults: Option<crate::ObjectOwner>,
    utc: bool,
    default_time: StrOwner,
    default_msec: Option<StrOwner>,
}

fn error(message: impl Into<Str>) -> Error {
    Error::new(Kind::ValueError, message)
}
fn invalid(message: &str) -> Error {
    error(format!("invalid format: {message}"))
}

fn braces(fmt: &str) -> Result<Vec<FormatPart>> {
    FormatString::from_raw_str(fmt)
        .map(|s| s.format_parts)
        .map_err(|e| {
            invalid(match e {
                FormatParseError::UnknownConversion => "expected ':' after conversion specifier",
                FormatParseError::MissingStartBracket => "Single '}' encountered in format string",
                FormatParseError::UnmatchedBracket | FormatParseError::MissingRightBracket
                    if fmt.ends_with('{') =>
                {
                    "Single '{' encountered in format string"
                }
                FormatParseError::UnmatchedBracket | FormatParseError::MissingRightBracket => {
                    "expected '}' before end of string"
                }
                _ => "unexpected '{' in field name",
            })
        })
}

enum Dollar<'a> {
    Text(&'a str),
    Field(&'a str),
}

/// Template names use ASCII identifiers, even when the surrounding text is Unicode.
fn dollars(mut fmt: &str) -> Result<Vec<Dollar<'_>>> {
    let mut tokens = Vec::new();
    while let Some((literal, tail)) = fmt.split_once('$') {
        tokens.push(Dollar::Text(literal));
        if let Some(rest) = tail.strip_prefix('$') {
            tokens.push(Dollar::Text("$"));
            fmt = rest;
            continue;
        }
        let braced = tail.starts_with('{');
        let name = if braced { &tail[1..] } else { tail };
        let len = name
            .bytes()
            .enumerate()
            .take_while(|&(i, b)| {
                b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
            })
            .count();
        if len == 0 || (braced && !name[len..].starts_with('}')) {
            return Err(invalid("bare '$' not allowed"));
        }
        tokens.push(Dollar::Field(&name[..len]));
        fmt = &name[len + usize::from(braced)..];
    }
    tokens.push(Dollar::Text(fmt));
    Ok(tokens)
}

impl Formatter {
    fn new(
        fmt: Option<StrOwner>,
        date: Option<StrOwner>,
        style: &str,
        validate: bool,
        defaults: Option<crate::ObjectOwner>,
    ) -> Result<Self> {
        let (style, default) = match style {
            "%" => (Style::Percent, "%(message)s"),
            "{" => (Style::Brace, "{message}"),
            "$" => (Style::Dollar, "${message}"),
            _ => return Err(error("Style must be one of: %,{,$")),
        };
        let fmt = fmt
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| StrOwner::new(Str::from(default)));
        let value = Self {
            style,
            fmt,
            date,
            defaults,
            utc: false,
            default_time: StrOwner::new(Str::from("%Y-%m-%d %H:%M:%S")),
            default_msec: Some(StrOwner::new(Str::from("%s,%03d"))),
        };
        if validate {
            value.validate()?;
        }
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        let has_fields = match self.style {
            Style::Percent => {
                if !PERCENT.is_match(self.fmt.as_str()) {
                    return Err(error(format!(
                        "Invalid format '{}' for '%' style",
                        self.fmt
                    )));
                }
                true
            }
            Style::Dollar => dollars(self.fmt.as_str())?
                .iter()
                .any(|t| matches!(t, Dollar::Field(_))),
            Style::Brace => {
                let mut any = false;
                for part in braces(self.fmt.as_str())? {
                    if let FormatPart::Field {
                        field_name,
                        conversion_spec,
                        format_spec,
                    } = part
                    {
                        if !field_name.is_empty() {
                            if !FIELD.is_match(field_name.as_str()) {
                                return Err(invalid(&format!(
                                    "invalid field name/expression: {}",
                                    string_repr(&field_name)
                                )));
                            }
                            any = true;
                        }
                        if let Some(c) = conversion_spec
                            && !"rsa".contains(c)
                        {
                            return Err(invalid(&format!(
                                "invalid conversion: {}",
                                string_repr(&c.to_string())
                            )));
                        }
                        if !format_spec.is_empty() && !SPEC.is_match(format_spec.as_str()) {
                            return Err(invalid(&format!(
                                "bad specifier: {}",
                                string_repr(&format_spec)
                            )));
                        }
                    }
                }
                any
            }
        };
        if has_fields {
            Ok(())
        } else {
            Err(invalid("no fields"))
        }
    }

    fn uses_time(&self) -> bool {
        match self.style {
            Style::Percent => self.fmt.contains("%(asctime)"),
            Style::Brace => self.fmt.contains("{asctime"),
            Style::Dollar => self.fmt.contains("$asctime") || self.fmt.contains("${asctime}"),
        }
    }

    fn get(&self, r: &Record, key: &str) -> Result<OwnedWord> {
        if let Some(v) = r.fields.get(key) {
            return Ok(v);
        }
        if let Some(defaults) = &self.defaults {
            // SAFETY: the formatter retains this string-key map; lookup acquires a field owner.
            if let Some(v) = unsafe {
                defaults
                    .0
                    .with(|s| s.dictionary().logging_key(&Str::from(key)))
            } {
                return Ok(v);
            }
        }
        Err(error(format!(
            "Formatting field not found in record: {}",
            string_repr(key)
        )))
    }

    fn render(&self, r: &Record) -> Result<String> {
        let mut out = String::new();
        match self.style {
            Style::Dollar => {
                for part in dollars(self.fmt.as_str())? {
                    match part {
                        Dollar::Text(s) => append(&mut out, s)?,
                        Dollar::Field(key) => write_display(&mut out, &self.get(r, key)?)?,
                    }
                }
            }
            Style::Percent => {
                let mut tail = self.fmt.as_str();
                while let Some((literal, rest)) = tail.split_once('%') {
                    append(&mut out, literal)?;
                    if let Some(rest) = rest.strip_prefix('%') {
                        append(&mut out, "%")?;
                        tail = rest;
                        continue;
                    }
                    let rest = rest
                        .strip_prefix('(')
                        .ok_or_else(|| Error::new(Kind::TypeError, "format requires a mapping"))?;
                    let (key, rest) = record::mapping_key(rest)?;
                    let (text, rest) = record::percent_value(rest, &self.get(r, key)?)?;
                    append(&mut out, &text)?;
                    tail = rest;
                }
                append(&mut out, tail)?;
            }
            Style::Brace => out = self.brace_fields(r, self.fmt.as_str(), 0)?,
        }
        Ok(out)
    }

    fn brace_fields(&self, r: &Record, fmt: &str, depth: u8) -> Result<String> {
        if depth > 2 {
            return Err(error("Max string recursion exceeded"));
        }
        let mut out = String::new();
        for part in braces(fmt)? {
            match part {
                FormatPart::Literal(s) => append(&mut out, &s)?,
                FormatPart::Field {
                    field_name,
                    conversion_spec,
                    format_spec,
                } => {
                    let stem = field_name.find(['[', '.']).unwrap_or(field_name.len());
                    super::super::stdlib::check_keyword_field(&field_name[..stem])?;
                    let mut v = self.get(r, &field_name[..stem])?;
                    let mut parts = &field_name[stem..];
                    while !parts.is_empty() {
                        if let Some(rest) = parts.strip_prefix('[') {
                            let (sub, rest) = rest
                                .split_once(']')
                                .ok_or_else(|| error("Missing ']' in format string"))?;
                            v = record::brace_item(&v, sub)?;
                            parts = rest;
                        } else {
                            return Err(Error::new(
                                Kind::AttributeError,
                                format!(
                                    "object has no attribute '{}'",
                                    parts.trim_start_matches('.')
                                ),
                            ));
                        }
                    }
                    if let Some(c) = conversion_spec {
                        v = record::owned_string(record::percent_value(&c.to_string(), &v)?.0)?;
                    }
                    let spec = if format_spec.contains('{') {
                        self.brace_fields(r, &format_spec, depth + 1)?
                    } else {
                        format_spec
                    };
                    append(&mut out, &format_specifier(&v, &spec, "int")?)?;
                }
            }
        }
        Ok(out)
    }

    fn time(&self, r: &Record, date: Option<&str>) -> Result<String> {
        use num_traits::ToPrimitive;
        let created = self.get(r, "created")?;
        let seconds = match created.kind.raw() {
            // SAFETY: a null destination asks time() to return the current epoch seconds.
            0 => unsafe { libc::time(std::ptr::null_mut()) },
            1 => i64::from(created.word != 0),
            // SAFETY: the record field owns this exact integer.
            2 => unsafe { Int::borrow(created.word) }
                .big()
                .to_i64()
                .ok_or_else(super::timed::overflow)?,
            7 => super::timed::float_seconds(f64::from_bits(created.word as u64))?,
            _ => return Err(Error::new(Kind::TypeError, "expected a number")),
        };
        let clock = super::timed::civil(seconds, self.utc)?;
        // Python passes a full C-int year from struct_time back to strftime.
        if clock.tm_year.checked_add(1900).is_none() {
            return Err(Error::new(Kind::OverflowError, "year out of range"));
        }
        let custom = date.filter(|s| !s.is_empty());
        let pattern = custom.unwrap_or(self.default_time.as_str());
        let timestamp = super::format_timestamp(Some(pattern), &clock, Duration::ZERO);
        let Some(msec) = self
            .default_msec
            .as_ref()
            .filter(|s| !s.is_empty() && custom.is_none())
        else {
            return Ok(timestamp);
        };
        let mut input = msec.as_str();
        let mut out = String::new();
        let mut operands = [string(&timestamp), self.get(r, "msecs")?].into_iter();
        while let Some((s, rest)) = input.split_once('%') {
            out.push_str(s);
            if let Some(rest) = rest.strip_prefix('%') {
                out.push('%');
                input = rest;
                continue;
            }
            let value = operands.next().ok_or_else(|| {
                Error::new(Kind::TypeError, "not enough arguments for format string")
            })?;
            let (s, rest) = record::percent_value(rest, &value)?;
            out.push_str(&s);
            input = rest;
        }
        if operands.next().is_some() {
            return Err(Error::new(
                Kind::TypeError,
                "not all arguments converted during string formatting",
            ));
        }
        out.push_str(input);
        Ok(out)
    }

    fn format(&self, r: &mut Record) -> Result<String> {
        let message = r.message()?;
        r.fields.insert("message".into(), message)?;
        if self.uses_time() {
            let time = self.time(r, self.date.as_deref().map(Str::as_str))?;
            r.fields.insert("asctime".into(), string(&time))?;
        }
        let mut text = self.render(r)?;
        if r.fields.get("exc_info").as_ref().is_some_and(record::truth)
            && !r.fields.get("exc_text").as_ref().is_some_and(record::truth)
        {
            let info = &r.fields.field("exc_info")?;
            if record::tuple_len(info) == Some(3)
                && (0..3).all(|i| record::tuple_item(info, i).kind.raw() == 0)
            {
                r.fields
                    .insert("exc_text".into(), string("NoneType: None"))?;
            } else {
                return Err(Error::new(
                    Kind::NotImplementedError,
                    "native logging requires traceback capture to format uncached exception information",
                ));
            }
        }
        for key in ["exc_text", "stack_info"] {
            if let Some(v) = r.fields.get(key).filter(record::truth) {
                if !text.ends_with('\n') {
                    append(&mut text, "\n")?;
                }
                write_display(&mut text, &v)?;
            }
        }
        Ok(text)
    }
}

fn number(v: &OwnedWord) -> Result<f64> {
    use num_traits::ToPrimitive;
    match v.kind.raw() {
        7 => Ok(f64::from_bits(v.word as u64)),
        // SAFETY: a word of kind two is a rooted Python integer.
        2 => unsafe { Int::borrow(v.word) }
            .big()
            .to_f64()
            .ok_or_else(|| Error::new(Kind::OverflowError, "int too large to convert to float")),
        _ => Err(Error::new(Kind::TypeError, "expected a number")),
    }
}

/// Apply Python's alignment after converting the scalar; the field width counts characters.
pub(super) fn format_specifier(
    value: &OwnedWord,
    spec: &str,
    integer_name: &str,
) -> Result<String> {
    if spec.is_empty() {
        return display(value);
    }
    let mut tail = spec;
    let mut align = None;
    let mut fill = ' ';
    let mut chars = tail.chars();
    let first = chars.next();
    let explicit_fill = chars.next().filter(|c| "<^>=".contains(*c));
    if let Some(second) = explicit_fill {
        fill = first.unwrap();
        align = Some(second);
        tail = chars.as_str();
    } else if let Some(first) = first.filter(|c| "<^>=".contains(*c)) {
        align = Some(first);
        tail = &tail[first.len_utf8()..];
    }
    let sign = tail.chars().next().filter(|c| "+ -".contains(*c));
    if sign.is_some() {
        tail = &tail[1..];
    }
    let negative_zero = value.kind.raw() == 2 && tail.starts_with('z');
    if negative_zero {
        tail = &tail[1..];
    }
    let alternate = tail.starts_with('#');
    if alternate {
        tail = &tail[1..];
    }
    if let Some(rest) = tail.strip_prefix('0') {
        tail = rest;
        if explicit_fill.is_none() {
            fill = '0';
        }
        if align.is_none() {
            align = Some(if value.kind.raw() == 3 { '<' } else { '=' });
        }
    }
    let size = tail.bytes().take_while(u8::is_ascii_digit).count();
    let width = if size == 0 {
        0
    } else {
        tail[..size]
            .parse::<isize>()
            .map_err(|_| error("Too many decimal digits in format string"))? as usize
    };
    tail = &tail[size..];
    let group = tail.chars().next().filter(|c| ",_".contains(*c));
    if group.is_some() {
        tail = &tail[1..];
    }
    let precision = if let Some(rest) = tail.strip_prefix('.') {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return Err(error("Format specifier missing precision"));
        }
        tail = &rest[n..];
        Some(&rest[..n])
    } else {
        None
    };
    let numeric = matches!(value.kind.raw(), 1 | 2 | 7);
    let c = match tail {
        "" => {
            if value.kind.raw() == 7 {
                if precision.is_some() { 'g' } else { 's' }
            } else if numeric {
                'd'
            } else {
                's'
            }
        }
        _ if tail.chars().count() == 1 => tail.chars().next().unwrap(),
        _ if value.kind.raw() == 2 => {
            return Err(error(format!(
                "Invalid format specifier '{spec}' for object of type '{integer_name}'"
            )));
        }
        _ => return Err(error(format!("Invalid format specifier '{spec}'"))),
    };
    let integer = value.kind.raw() == 2;
    let integer_code = matches!(c, 'b' | 'o' | 'd' | 'x' | 'X' | 'n' | 'c');
    if integer && integer_code {
        if negative_zero {
            return Err(error(
                "Negative zero coercion (z) not allowed in integer format specifier",
            ));
        }
        if precision.is_some() {
            return Err(error("Precision not allowed in integer format specifier"));
        }
        if let Some(group) = group
            && (matches!(c, 'n' | 'c') || group == ',' && matches!(c, 'x' | 'X' | 'o' | 'b'))
        {
            return Err(error(format!("Cannot specify '{group}' with '{c}'.")));
        }
        if c == 'c' {
            if sign.is_some() || alternate {
                return Err(error(if sign.is_some() {
                    "Sign not allowed with integer format specifier 'c'"
                } else {
                    "Alternate form (#) not allowed with integer format specifier 'c'"
                }));
            }
            // SAFETY: this field holds a rooted native integer.
            let n = unsafe { Int::borrow(value.word) };
            n.to_i64()
                .and_then(|n| std::ffi::c_long::try_from(n).ok())
                .ok_or_else(|| {
                    Error::new(
                        Kind::OverflowError,
                        "Python int too large to convert to C long",
                    )
                })?;
        }
    } else if integer && !"eEfFgG%".contains(c) {
        return Err(error(format!(
            "Unknown format code '{c}' for object of type '{integer_name}'"
        )));
    }
    let flags = format!(
        "{}{}",
        sign.map_or(String::new(), |c| c.to_string()),
        if alternate { "#" } else { "" }
    );
    let precision = precision.map(|p| format!(".{p}")).unwrap_or_default();
    let mut s = if c == '%' {
        let n = number(value)? * 100.0;
        let spec = format!("{flags}{precision}f");
        let temp = record::float(n);
        format!("{}%", record::percent_value(&spec, &temp)?.0)
    } else if integer && matches!(c, 'd' | 'o' | 'x' | 'X' | 'n') {
        // SAFETY: an integer word retains the native Int throughout formatting.
        let number = unsafe { Int::borrow(value.word) };
        let code = if c == 'n' { 'd' } else { c };
        record::percent_integer_field(&format!("{flags}{code}"), [(*number).clone()])?
    } else if c == 'b' {
        if !integer {
            return Err(error("Unknown format code 'b' for this value"));
        }
        // SAFETY: the integer descriptor and its owner were checked above.
        let number = unsafe { Int::borrow(value.word) };
        let number = number.big();
        let sign = if number.sign() == num_bigint::Sign::Minus {
            "-"
        } else {
            match sign {
                Some('+') => "+",
                Some(' ') => " ",
                _ => "",
            }
        };
        let prefix = if alternate { "0b" } else { "" };
        format!("{sign}{prefix}{:b}", number.magnitude())
    } else if c == 'n' {
        record::percent_value(
            &format!(
                "{flags}{precision}{}",
                if value.kind.raw() == 7 { 'g' } else { 'd' }
            ),
            value,
        )?
        .0
    } else {
        record::percent_value(&format!("{flags}{precision}{c}"), value)?.0
    };
    let sign_len = usize::from(c != 'c' && s.starts_with(['-', '+', ' ']));
    let prefix_len = if integer && alternate && matches!(c, 'b' | 'o' | 'x' | 'X') {
        2
    } else {
        0
    };
    let head = sign_len + prefix_len;
    if let Some(group) = group
        && integer
        && matches!(c, 'd' | 'b' | 'o' | 'x' | 'X')
    {
        let step = if c == 'd' { 3 } else { 4 };
        let digits = s.len() - head;
        let zeros = if fill == '0' && align == Some('=') {
            let target = width.saturating_sub(head);
            target
                .saturating_sub(target.saturating_sub(1) / (step + 1))
                .saturating_sub(digits)
        } else {
            0
        };
        let digits = digits + zeros;
        let mut grouped = String::new();
        let bytes = head
            .checked_add(digits)
            .and_then(|n| n.checked_add((digits - 1) / step))
            .ok_or_else(|| Error::new(Kind::OverflowError, "formatted string is too long"))?;
        grouped
            .try_reserve_exact(bytes)
            .map_err(|_| Error::new(Kind::MemoryError, "formatted string is too long"))?;
        grouped.push_str(&s[..head]);
        for (i, digit) in std::iter::repeat_n(b'0', zeros)
            .chain(s.bytes().skip(head))
            .enumerate()
        {
            if i != 0 && (digits - i).is_multiple_of(step) {
                grouped.push(group);
            }
            grouped.push(char::from(digit));
        }
        s = grouped;
    } else if let Some(group) = group
        && numeric
    {
        let start = s.find(|ch: char| ch.is_ascii_digit()).unwrap_or(s.len());
        let end = s[start..]
            .find(|ch: char| !ch.is_ascii_digit())
            .map_or(s.len(), |i| start + i);
        for i in (start + 1..end).rev() {
            if (end - i) % 3 == 0 {
                s.insert(i, group);
            }
        }
    }
    let count = width.saturating_sub(s.chars().count());
    let align = align.unwrap_or(if numeric { '>' } else { '<' });
    let (left, right) = match align {
        '<' => (0, count),
        '^' => (count / 2, count - count / 2),
        _ => (count, 0),
    };
    let pos = if align == '=' { head } else { 0 };
    let mut out = String::new();
    let bytes = count
        .checked_mul(fill.len_utf8())
        .and_then(|n| n.checked_add(s.len()))
        .ok_or_else(|| Error::new(Kind::OverflowError, "formatted string is too long"))?;
    out.try_reserve_exact(bytes)
        .map_err(|_| Error::new(Kind::MemoryError, "formatted string is too long"))?;
    out.push_str(&s[..pos]);
    out.extend(std::iter::repeat_n(fill, left));
    out.push_str(&s[pos..]);
    out.extend(std::iter::repeat_n(fill, right));
    Ok(out)
}

unsafe fn optional_string(word: usize) -> Option<StrOwner> {
    // SAFETY: the caller roots the exact string or passes zero for None.
    (word != 0).then(|| unsafe { StrOwner::acquire_word(word) })
}

unsafe fn optional_object(word: usize) -> Option<crate::ObjectOwner> {
    // SAFETY: callers supply zero or a rooted native object.
    (word != 0).then(|| unsafe { crate::ObjectOwner::acquire_word(word) })
}

fn word(s: Option<StrOwner>) -> usize {
    s.map_or(0, |s| StrOwner::into_raw(s) as usize)
}
fn text(result: Result<String>) -> *const Str {
    finish(
        result.and_then(|s| {
            StrOwner::try_new(Str::from(s))
                .map(StrOwner::into_raw)
                .map_err(|_| Error::allocation())
        }),
        std::ptr::null(),
    )
}

/// # Safety
/// Nonzero string and mapping inputs have their declared native types and are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_new(
    fmt: usize,
    date: usize,
    style: *const Str,
    validate: bool,
    defaults: usize,
) -> *const Object {
    // SAFETY: retain all constructor inputs that may outlive this call.
    let result = unsafe {
        Formatter::new(
            optional_string(fmt),
            optional_string(date),
            (&*style).as_str(),
            validate,
            optional_object(defaults),
        )
    };
    finish(
        result.map(|f| {
            crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                Storage::LogFormatter(Box::new(f)),
            ))))
        }),
        std::ptr::null(),
    )
}

unsafe fn with<T>(f: *const Object, read: impl FnOnce(&Formatter) -> T) -> T {
    // SAFETY: the caller roots the exact formatter; no reentrant operation runs under the borrow.
    unsafe {
        object(f).0.with(|s| match s {
            Storage::LogFormatter(f) => read(f),
            _ => super::super::fail("SystemError", "expected Formatter"),
        })
    }
}
unsafe fn change(f: *const Object, write: impl FnOnce(&mut Formatter)) {
    // SAFETY: both the receiver and inputs are rooted; no user code is invoked during mutation.
    unsafe {
        object(f).0.with_mut(|s| match s {
            Storage::LogFormatter(f) => write(f),
            _ => super::super::fail("SystemError", "expected Formatter"),
        })
    }
}

/// # Safety
/// The formatter is rooted. The return value owns its own reference to the format.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_text(f: *const Object) -> *const Str {
    // SAFETY: the formatter owns the cloned string throughout this read.
    unsafe { with(f, |f| StrOwner::into_raw(f.fmt.clone())) }
}
/// # Safety
/// The formatter is rooted; nonzero results have independent string ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_date(f: *const Object) -> usize {
    // SAFETY: the formatter owns the optional date string.
    unsafe { with(f, |f| word(f.date.clone())) }
}
/// # Safety
/// The formatter stays rooted; the returned string has an independent owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_default_time(f: *const Object) -> *const Str {
    // SAFETY: the formatter owns the cloned string throughout this read.
    unsafe { with(f, |f| StrOwner::into_raw(f.default_time.clone())) }
}
/// # Safety
/// The formatter is rooted; a nonzero return transfers an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_default_msec(f: *const Object) -> usize {
    // SAFETY: the formatter owns the optional millisecond format.
    unsafe { with(f, |f| word(f.default_msec.clone())) }
}
/// # Safety
/// The formatter is rooted and holds an immutable format during this read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_uses_time(f: *const Object) -> bool {
    // SAFETY: inspecting the format invokes no callbacks.
    unsafe { with(f, Formatter::uses_time) }
}
/// # Safety
/// Distinct formatter and record objects remain rooted for the entire operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_format(f: *const Object, r: *const Object) -> *const Str {
    // SAFETY: only the record is mutated; its fields have independent native owners.
    text(unsafe {
        with(f, |f| {
            object(r).0.with_mut(|s| match s {
                Storage::LogRecord(r) => f.format(r),
                _ => unreachable!(),
            })
        })
    })
}
/// # Safety
/// The formatter and record stay rooted as fields and defaults are rendered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_message(f: *const Object, r: *const Object) -> *const Str {
    // SAFETY: both borrows are read-only and invoke no Python callbacks.
    text(unsafe { with(f, |f| record::with_record(r, |r| f.render(r))) })
}
/// # Safety
/// The formatter, record and optional exact date format stay alive for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_time(
    f: *const Object,
    r: *const Object,
    date: usize,
) -> *const Str {
    // SAFETY: all three arguments are borrowed; text owns the result independently.
    text(unsafe {
        with(f, |f| {
            record::with_record(r, |r| {
                f.time(r, (date as *const Str).as_ref().map(Str::as_str))
            })
        })
    })
}
/// # Safety
/// The formatter is rooted as the native timezone converter is replaced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_utc(f: *const Object, utc: bool) {
    // SAFETY: this nonreentrant write changes one scalar.
    unsafe { change(f, |f| f.utc = utc) }
}
/// # Safety
/// The optional string and formatter are rooted; the formatter takes its own owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_set_date(f: *const Object, date: usize) {
    // SAFETY: acquire the exact input before replacing the old owner.
    unsafe {
        let date = optional_string(date);
        change(f, |f| f.date = date)
    }
}
/// # Safety
/// Both the formatter and exact string are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_set_default_time(f: *const Object, s: *const Str) {
    // SAFETY: acquire the input before replacing the previous reference.
    unsafe {
        let s = crate::StrOwner::acquire_word(s as usize);
        change(f, |f| f.default_time = s)
    }
}
/// # Safety
/// The formatter and optional exact string are rooted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_set_default_msec(f: *const Object, s: usize) {
    // SAFETY: acquire the input before replacing the previous reference.
    unsafe {
        let s = optional_string(s);
        change(f, |f| f.default_msec = s)
    }
}
/// # Safety
/// Both arguments are rooted; the returned stack text has independent ownership.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_stack(_f: *const Object, s: *const Str) -> *const Str {
    // SAFETY: acquire the exact string, which the default formatter returns unchanged.
    StrOwner::into_raw(unsafe { crate::StrOwner::acquire_word(s as usize) })
}
