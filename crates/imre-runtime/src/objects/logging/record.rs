//! A log record owns one native Python value per attribute, including user-supplied extras.
use super::fields::Fields;
use crate::{ObjectOwner, StrOwner};
use std::cell::Cell;
use std::fmt::Write as _;
thread_local! {static FLAGS:Cell<u8> = const {Cell::new(31)};}
use num_traits::ToPrimitive;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::exceptions::Result;
use super::super::words::{NULLABLE, OwnedWord, WordKind};
use super::super::{
    Bytes, Error, Int, Kind, List, Object, Storage, StorageCell, Str, boxed_values, finish, object,
    records, values,
};

pub(in crate::objects) struct Record {
    pub(super) fields: Fields,
}

fn owned(word: usize, kind: u32) -> OwnedWord {
    // SAFETY: callers transfer one owner or an immediate scalar of the given kind.
    unsafe { OwnedWord::from_owned(word, WordKind::new(kind)) }
}

pub(super) fn string(s: &str) -> OwnedWord {
    owned(StrOwner::into_raw(StrOwner::new(Str::from(s))) as usize, 3)
}
fn integer(n: usize) -> OwnedWord {
    owned(Int::from_usize(n).into_raw(), 2)
}
pub(super) fn float(n: f64) -> OwnedWord {
    owned(n.to_bits() as usize, 7)
}

fn exact_text(v: &OwnedWord) -> Option<&str> {
    if v.kind.raw() & !NULLABLE == 3 && v.word != 0 {
        // SAFETY: v owns this exact string for the duration of the borrow.
        Some(unsafe { &*(v.word as *const Str) }.as_str())
    } else {
        None
    }
}

pub(super) fn write_display(out: &mut String, v: &OwnedWord) -> Result<()> {
    // SAFETY: v retains its concrete payload until formatting finishes.
    let (kind, word) = unsafe { boxed_values::concrete(v.kind, v.word) };
    if boxed_values::write_string_value(&mut values::ReprWriter(out), kind, word)
        .map_err(|_| Error::allocation())?
    {
        return Ok(());
    }
    // SAFETY: v keeps the value graph rooted until the entire field is written.
    unsafe { values::try_write_unchecked_repr(&mut values::ReprWriter(out), word, kind) }
        .map_err(|_| Error::allocation())
}

pub(super) fn display(v: &OwnedWord) -> Result<String> {
    let mut out = String::new();
    write_display(&mut out, v)?;
    Ok(out)
}

pub(super) fn owned_string(s: String) -> Result<OwnedWord> {
    StrOwner::try_new(Str::from(s))
        .map(|text| owned(StrOwner::into_raw(text) as usize, 3))
        .map_err(|_| Error::allocation())
}

pub(super) fn tuple_len(v: &OwnedWord) -> Option<usize> {
    if v.kind.raw() & !NULLABLE != 6 || v.word == 0 {
        return None;
    }
    // SAFETY: all kind-six owners have immutable record descriptors.
    let d = unsafe { records::descriptor(v.word as *const usize) };
    // SAFETY: semantics are static; kind one denotes an exact tuple.
    unsafe { d.semantics.as_ref() }
        .filter(|s| s.kind == 1)
        .map(|_| d.length)
}

pub(super) fn tuple_item(v: &OwnedWord, index: usize) -> OwnedWord {
    assert!(tuple_len(v).is_some_and(|len| index < len));
    // SAFETY: the live tuple supplies a descriptor and valid slot at this index.
    unsafe {
        let data = v.word as *const usize;
        let k = WordKind::new(*records::descriptor(data).kinds.add(index));
        k.acquire(k.read_record_field(data.add(index)))
    }
}

pub(super) fn is_dict(v: &OwnedWord) -> bool {
    // SAFETY: a nonzero kind-five word owns an Object, examined without mutation.
    v.kind.raw() & !NULLABLE == 5
        && v.word != 0
        && unsafe {
            object(v.word as *const Object)
                .0
                .with(|s| matches!(s, Storage::Dictionary(_)))
        }
}

pub(super) fn dict_item(v: &OwnedWord, key: &str) -> Result<OwnedWord> {
    if !is_dict(v) {
        return Err(Error::new(Kind::TypeError, "format requires a mapping"));
    }
    let key = Str::from(key);
    // SAFETY: v and key retain both operands; lookup returns an independent owner.
    unsafe {
        object(v.word as *const Object)
            .0
            .with(|s| s.dictionary().logging_key(&key))
    }
    .ok_or_else(|| Error::key(string(key.as_str())))
}

/// A bracketed formatter item is a decimal index or the literal spelling of a key.
pub(super) fn brace_item(value: &OwnedWord, key: &str) -> Result<OwnedWord> {
    if key.is_empty() {
        return Err(Error::new(
            Kind::ValueError,
            "Empty attribute in format string",
        ));
    }
    let index = super::super::stdlib::format_index(key)?;
    let concrete;
    let value = if value.kind.base() == 9 && value.word != 0 {
        let (word, kind) = super::super::unions::member(value.word);
        // SAFETY: the union owner roots its concrete member throughout acquisition.
        concrete = unsafe { kind.acquire(word) };
        &concrete
    } else {
        value
    };
    let unsupported = |kind| {
        Error::new(
            Kind::TypeError,
            format!("'{kind}' object is not subscriptable"),
        )
    };
    if value.kind.raw() & NULLABLE != 0 && value.word == 0 {
        return Err(unsupported("NoneType"));
    }
    if is_dict(value) {
        let lookup = index.map_or_else(|| string(key), integer);
        // SAFETY: value owns the map and the lookup key stays rooted throughout the call.
        if let Some(item) = unsafe {
            object(value.word as *const Object)
                .0
                .with_mut(|s| s.dictionary_mut().subscript_value(&lookup))
        }? {
            return Ok(item);
        }
        let mut message = String::from("Formatting field not found in record: ");
        // SAFETY: lookup owns the integer or string key throughout the diagnostic.
        unsafe {
            values::try_write_unchecked_repr(
                &mut values::ReprWriter(&mut message),
                lookup.word,
                lookup.kind,
            )
        }
        .map_err(|_| Error::allocation())?;
        return Err(Error::new(Kind::ValueError, message));
    }
    let index_for = |kind| {
        index.ok_or_else(|| {
            Error::new(
                Kind::TypeError,
                match kind {
                    "string" => "string indices must be integers, not 'str'".to_owned(),
                    _ => format!("{kind} indices must be integers or slices, not str"),
                },
            )
        })
    };
    let (kind, item) = if let Some(len) = tuple_len(value) {
        let at = index_for("tuple")?;
        ("tuple", (at < len).then(|| tuple_item(value, at)))
    } else {
        // SAFETY: each exact descriptor identifies the rooted field's storage.
        unsafe {
            match value.kind.base() {
                3 => {
                    let text = &*(value.word as *const Str);
                    (
                        "string",
                        text.chars()
                            .nth(index_for("string")?)
                            .map(|ch| string(ch.encode_utf8(&mut [0; 4]))),
                    )
                }
                4 => {
                    let at = index_for("list")?;
                    (
                        "list",
                        (&*(value.word as *const List))
                            .0
                            .with(|list| list.get(at).map(|n| owned(n.clone().into_raw(), 2))),
                    )
                }
                5 => object(value.word as *const Object).0.with(|storage| {
                    let Storage::Sequence(_) = storage else {
                        return Err(unsupported("object"));
                    };
                    let words = storage.sequence()?;
                    Ok((
                        "list",
                        words
                            .values
                            .get(index_for("list")?)
                            .map(|&word| words.kind.acquire(word)),
                    ))
                })?,
                8 => (
                    "bytes",
                    (&*(value.word as *const Bytes))
                        .get(index_for("byte")?)
                        .map(|&b| integer(usize::from(b))),
                ),
                kind => {
                    return Err(unsupported(match kind {
                        0 => "NoneType",
                        1 => "bool",
                        2 => "int",
                        7 => "float",
                        _ => "object",
                    }));
                }
            }
        }
    };
    item.ok_or_else(|| Error::new(Kind::IndexError, format!("{kind} index out of range")))
}

pub(super) fn truth(v: &OwnedWord) -> bool {
    match v.kind.raw() {
        0 => false,
        1 => v.word != 0,
        // SAFETY: the discriminant establishes a rooted native integer.
        2 => unsafe { Int::borrow(v.word) }.as_small() != Some(0),
        3 => exact_text(v).is_some_and(|s| !s.is_empty()),
        7 => f64::from_bits(v.word as u64) != 0.0,
        _ if tuple_len(v).is_some() => tuple_len(v) != Some(0),
        _ if is_dict(v) => {
            // SAFETY: the kind and variant were checked, and v roots the dictionary.
            let n = unsafe { super::super::mappings::imre_dict_len(v.word as *const Object) };
            Int(n).as_small() != Some(0)
        }
        _ => v.word != 0,
    }
}

fn nanos() -> i128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

fn milliseconds(ct: i128) -> (f64, f64) {
    let created = ct as f64 / 1e9;
    let msecs = ct.rem_euclid(1_000_000_000) / 1_000_000;
    (
        created,
        if msecs == 999 && created as i128 != ct.div_euclid(1_000_000_000) {
            0.0
        } else {
            msecs as f64
        },
    )
}

impl Record {
    fn new(
        [name, level, path, line, msg, mut args, exc, func, stack]: [OwnedWord; 9],
    ) -> Result<Self> {
        static START: OnceLock<i128> = OnceLock::new();
        let start = *START.get_or_init(nanos);
        let ct = nanos();
        let (created, msecs) = milliseconds(ct);
        if tuple_len(&args) == Some(1) {
            let item = tuple_item(&args, 0);
            if is_dict(&item) && truth(&item) {
                args = item;
            }
        }
        let (filename, module) = if let Some(path) = exact_text(&path) {
            let base = path.rsplit('/').next().unwrap();
            // POSIX splitext preserves leading dots and strips only the last suffix.
            let cut = base
                .rfind('.')
                .filter(|&i| base[..i].chars().any(|ch| ch != '.'))
                .unwrap_or(base.len());
            (string(base), string(&base[..cut]))
        } else {
            (path.clone(), string("Unknown module"))
        };
        let unknown_level = || -> Result<OwnedWord> {
            let mut out = String::from("Level ");
            write_display(&mut out, &level)?;
            owned_string(out)
        };
        let levelname = match level.kind.raw() {
            2 => {
                // SAFETY: level owns this Int until lookup finishes.
                let number = unsafe { Int::borrow(level.word) };
                owned(
                    StrOwner::into_raw(super::levels::level_name(&number)) as usize,
                    3,
                )
            }
            3 => {
                // SAFETY: level is a rooted exact string.
                let name = unsafe { &*(level.word as *const Str) };
                match super::levels::number_for(name) {
                    Some(n) => owned(n.into_raw(), 2),
                    None => unknown_level()?,
                }
            }
            _ => unknown_level()?,
        };
        // SAFETY: pthread_self has no preconditions and does not transfer ownership.
        let thread = unsafe { libc::pthread_self() } as usize;
        let flags = FLAGS.get();
        let fields = [
            ("name", name),
            ("msg", msg),
            ("args", args),
            ("levelname", levelname),
            ("levelno", level),
            ("pathname", path),
            ("filename", filename),
            ("module", module),
            ("exc_info", exc),
            ("exc_text", owned(0, 0)),
            ("stack_info", stack),
            ("lineno", line),
            ("funcName", func),
            ("created", float(created)),
            ("msecs", float(msecs)),
            ("relativeCreated", float((ct - start) as f64 / 1e6)),
            (
                "thread",
                if flags & 1 != 0 {
                    integer(thread)
                } else {
                    owned(0, 0)
                },
            ),
            (
                "threadName",
                if flags & 1 != 0 {
                    string("MainThread")
                } else {
                    owned(0, 0)
                },
            ),
            (
                "processName",
                if flags & 4 != 0 {
                    string("MainProcess")
                } else {
                    owned(0, 0)
                },
            ),
            (
                "process",
                if flags & 2 != 0 {
                    integer(std::process::id() as usize)
                } else {
                    owned(0, 0)
                },
            ),
            ("taskName", owned(0, 0)),
        ];
        Ok(Self {
            fields: Fields::new(fields),
        })
    }

    fn repr(&self) -> Result<String> {
        let mut out = String::from("<LogRecord: ");
        for (field, after) in [
            ("name", ", "),
            ("levelno", ", "),
            ("pathname", ", "),
            ("lineno", ", \""),
            ("msg", "\">"),
        ] {
            write_display(&mut out, &self.fields.field(field)?)?;
            append(&mut out, after)?;
        }
        Ok(out)
    }

    pub(super) fn message(&self) -> Result<OwnedWord> {
        let msg = &self.fields.field("msg")?;
        let args = &self.fields.field("args")?;
        if !truth(args) {
            return if exact_text(msg).is_some() {
                Ok(msg.clone())
            } else {
                display(msg).and_then(owned_string)
            };
        }
        let rendered;
        let fmt = if let Some(s) = exact_text(msg) {
            s
        } else {
            rendered = display(msg)?;
            &rendered
        };
        format_arguments(fmt, args).and_then(owned_string)
    }
}

/// Percent formatting is shared by log messages and ordinary string expressions. Callers
/// decide whether an empty argument tuple bypasses formatting (only LogRecord does that).
fn format_arguments(msg: &str, args: &OwnedWord) -> Result<String> {
    let tuple = tuple_len(args);
    let positional = (0..tuple.unwrap_or(1)).map(|i| {
        if tuple.is_some() {
            tuple_item(args, i)
        } else {
            args.clone()
        }
    });
    format_operands(msg, positional, is_dict(args).then_some(args))
}

fn format_operands(
    msg: &str,
    mut positional: impl ExactSizeIterator<Item = OwnedWord>,
    mapping: Option<&OwnedWord>,
) -> Result<String> {
    let mut out = String::new();
    let mut input = msg;
    while let Some((literal, rest)) = input.split_once('%') {
        append(&mut out, literal)?;
        if let Some(rest) = rest.strip_prefix('%') {
            append(&mut out, "%")?;
            input = rest;
            continue;
        }
        let (piece, tail) = if let Some(rest) = rest.strip_prefix('(') {
            let Some(mapping) = mapping else {
                return Err(Error::new(Kind::TypeError, "format requires a mapping"));
            };
            let (key, rest) = mapping_key(rest)?;
            // A named conversion replaces the sole positional mapping with
            // the looked-up value. Later named conversions can still use args.
            let _ = positional.next();
            percent_value_in(
                rest,
                &mut std::iter::once(dict_item(mapping, key)?),
                Some(msg),
            )?
        } else {
            percent_value_in(rest, &mut positional, Some(msg))?
        };
        if out.is_empty() {
            out = piece;
        } else {
            append(&mut out, &piece)?;
        }
        input = tail;
    }
    if mapping.is_none() && positional.len() != 0 {
        return Err(Error::new(
            Kind::TypeError,
            "not all arguments converted during string formatting",
        ));
    }
    append(&mut out, input)?;
    Ok(out)
}

pub(super) fn append(out: &mut String, text: &str) -> Result<()> {
    values::ReprWriter(out)
        .write_str(text)
        .map_err(|_| Error::allocation())
}

fn percent_text(value: &OwnedWord, conversion: char, precision: Option<usize>) -> Result<String> {
    let end = |s: &str| {
        precision
            .and_then(|n| s.char_indices().nth(n))
            .map_or(s.len(), |(i, _)| i)
    };
    let mut text = String::new();
    if let Some(s) = exact_text(value).filter(|_| conversion == 's') {
        append(&mut text, &s[..end(s)])?;
        return Ok(text);
    }
    // SAFETY: the argument retains its concrete payload throughout representation.
    let (kind, word) = unsafe { boxed_values::concrete(value.kind, value.word) };
    if conversion != 's'
        || !boxed_values::write_string_value(&mut values::ReprWriter(&mut text), kind, word)
            .map_err(|_| Error::allocation())?
    {
        // SAFETY: the original value keeps the normalized payload alive.
        unsafe { values::try_write_unchecked_repr(&mut values::ReprWriter(&mut text), word, kind) }
            .map_err(|_| Error::allocation())?;
    }
    if conversion == 'a' && !text.is_ascii() {
        let mut ascii = String::new();
        let mut out = values::ReprWriter(&mut ascii);
        for ch in text.chars() {
            let code = ch as u32;
            match code {
                0..=127 => out.write_char(ch),
                128..=255 => write!(out, "\\x{code:02x}"),
                256..=65535 => write!(out, "\\u{code:04x}"),
                _ => write!(out, "\\U{code:08x}"),
            }
            .map_err(|_| Error::allocation())?;
        }
        text = ascii;
    }
    text.truncate(end(&text));
    Ok(text)
}

/// Find the closing delimiter after "%(", allowing balanced parentheses in the key.
pub(super) fn mapping_key(rest: &str) -> Result<(&str, &str)> {
    let mut nested = 0usize;
    for (i, b) in rest.bytes().enumerate() {
        match b {
            b'(' => nested += 1,
            b')' if nested == 0 => return Ok((&rest[..i], &rest[i + 1..])),
            b')' => nested -= 1,
            _ => {}
        }
    }
    Err(Error::new(Kind::ValueError, "incomplete format key"))
}

/// Reserve before filling a user-provided field width so it can raise a Python exception.
fn padding(n: usize, c: char) -> Result<String> {
    let mut text = String::new();
    text.try_reserve_exact(n)
        .map_err(|_| Error::new(Kind::MemoryError, "formatted string is too long"))?;
    text.extend(std::iter::repeat_n(c, n));
    Ok(text)
}

/// Formats a single percent conversion and returns the remainder of the original format.
pub(super) fn percent_value<'a>(fmt: &'a str, value: &OwnedWord) -> Result<(String, &'a str)> {
    percent_value_in(fmt, &mut std::iter::once(value.clone()), None)
}

fn unpack_object(value: OwnedWord) -> OwnedWord {
    if value.kind.raw() == 5 && value.word != 0 {
        // SAFETY: a kind-five owner roots this object; clone a boxed field inside the borrow.
        if let Some(field) = unsafe {
            object(value.word as *const Object).0.with(|s| match s {
                Storage::ObjectValue(field) => Some(field.clone()),
                _ => None,
            })
        } {
            return field;
        }
    }
    value
}

/// Fetch a format operand at the point CPython consumes its width, precision or value.
fn next_argument(args: &mut impl Iterator<Item = OwnedWord>) -> Result<OwnedWord> {
    args.next()
        .map(unpack_object)
        .ok_or_else(|| Error::new(Kind::TypeError, "not enough arguments for format string"))
}

/// A '*' consumes one integer before the formatter asks for the next operand.
fn star_integer(args: &mut impl Iterator<Item = OwnedWord>) -> Result<Int> {
    let value = next_argument(args)?;
    match value.kind.raw() {
        1 => Ok(Int::from_usize(value.word)),
        2 => Ok(Int(value.into_word())),
        _ => Err(Error::new(Kind::TypeError, "* wants int")),
    }
}

/// An exact scalar supplies its Python class name in a rejected conversion.
fn scalar_name(kind: WordKind) -> Option<&'static str> {
    Some(match kind.raw() {
        0 => "NoneType",
        1 => "bool",
        2 => "int",
        3 => "str",
        7 => "float",
        8 => "bytes",
        _ => return None,
    })
}

/// The full format supplies the Unicode character offset for invalid conversions.
fn percent_value_in<'a>(
    fmt: &'a str,
    args: &mut impl Iterator<Item = OwnedWord>,
    full: Option<&str>,
) -> Result<(String, &'a str)> {
    let mut chars = fmt.char_indices().peekable();
    let mut flags = String::new();
    while chars.peek().is_some_and(|(_, c)| "#0-+ ".contains(*c)) {
        flags.push(chars.next().unwrap().1);
    }
    let mut width = String::new();
    while chars.peek().is_some_and(|(_, c)| c.is_ascii_digit()) {
        width.push(chars.next().unwrap().1);
    }
    let width = if width.is_empty() && chars.next_if(|(_, c)| *c == '*').is_some() {
        let n = star_integer(args)?.big().to_isize().ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })?;
        if n < 0 {
            flags.push('-');
        }
        // CPython leaves PY_SSIZE_T_MIN without a positive padding width.
        n.checked_abs().unwrap_or(0) as usize
    } else if width.is_empty() {
        0
    } else {
        width
            .parse::<isize>()
            .map_err(|_| Error::new(Kind::ValueError, "width too big"))? as usize
    };
    let precision = if chars.peek().is_some_and(|(_, c)| *c == '.') {
        chars.next();
        let mut n = String::new();
        while chars.peek().is_some_and(|(_, c)| c.is_ascii_digit()) {
            n.push(chars.next().unwrap().1);
        }
        Some(
            if n.is_empty() && chars.next_if(|(_, c)| *c == '*').is_some() {
                star_integer(args)?
                    .big()
                    .to_i32()
                    .ok_or_else(|| {
                        Error::new(
                            Kind::OverflowError,
                            "Python int too large to convert to C int",
                        )
                    })?
                    .max(0) as usize
            } else if n.is_empty() {
                0
            } else {
                n.parse::<i32>()
                    .map_err(|_| Error::new(Kind::ValueError, "precision too big"))?
                    as usize
            },
        )
    } else {
        None
    };
    if chars.peek().is_some_and(|(_, c)| "hlL".contains(*c)) {
        chars.next();
    }
    let (i, c) = chars
        .next()
        .ok_or_else(|| Error::new(Kind::ValueError, "incomplete format"))?;
    let value = &next_argument(args)?;
    if value.kind == WordKind::new(2) && matches!(c, 'd' | 'i' | 'u' | 's' | 'r' | 'a') {
        // SAFETY: the selected operand roots this exact integer.
        let integer = unsafe { Int::borrow(value.word) };
        if integer.as_small().is_none() {
            crate::integer_text::check_decimal(&integer.big())?;
        }
    }
    let mut text = match c {
        's' | 'r' | 'a' => percent_text(value, c, precision)?,
        'd' | 'i' | 'u' | 'x' | 'X' | 'o' => {
            let n = match value.kind.raw() {
                1 => num_bigint::BigInt::from(value.word),
                // SAFETY: this arm has an integer descriptor and a live owner.
                2 => unsafe { Int::borrow(value.word) }.big().into_owned(),
                7 if "diu".contains(c) => {
                    crate::floats::float_to_integer(f64::from_bits(value.word as u64))?
                }
                _ => {
                    let Some(name) = scalar_name(value.kind) else {
                        return Err(Error::new(Kind::TypeError, "format requires a number"));
                    };
                    let required = if "diu".contains(c) {
                        "a real number"
                    } else {
                        "an integer"
                    };
                    return Err(Error::new(
                        Kind::TypeError,
                        format!("%{c} format: {required} is required, not {name}"),
                    ));
                }
            };
            let mut digits = n.to_str_radix(if c == 'o' {
                8
            } else if matches!(c, 'x' | 'X') {
                16
            } else {
                10
            });
            let neg = digits.starts_with('-');
            if neg {
                digits.remove(0);
            }
            if let Some(n) = precision
                && digits.len() < n
            {
                digits = format!("{}{}", padding(n - digits.len(), '0')?, digits);
            }
            if c == 'X' {
                digits.make_ascii_uppercase();
            }
            let prefix = if flags.contains('#') {
                match c {
                    'x' => "0x",
                    'X' => "0X",
                    'o' => "0o",
                    _ => "",
                }
            } else {
                ""
            };
            let sign = if neg {
                "-"
            } else if flags.contains('+') {
                "+"
            } else if flags.contains(' ') {
                " "
            } else {
                ""
            };
            if flags.contains('0') && !flags.contains('-') {
                let n = width.saturating_sub(sign.len() + prefix.len() + digits.len());
                digits = format!("{}{}", padding(n, '0')?, digits);
            }
            format!("{sign}{prefix}{digits}")
        }
        'c' => {
            if let Some(s) = exact_text(value) {
                let count = s.chars().count();
                if count != 1 {
                    return Err(Error::new(
                        Kind::TypeError,
                        format!(
                            "%c requires an int or a unicode character, not a string of length {count}"
                        ),
                    ));
                }
                s.to_owned()
            } else if matches!(value.kind.raw(), 1 | 2) {
                let code = if value.kind.raw() == 1 {
                    Some(value.word as u32)
                } else {
                    // SAFETY: the live integer supplies an exact native word.
                    unsafe { Int::borrow(value.word) }.big().to_u32()
                };
                code.and_then(char::from_u32)
                    .ok_or_else(|| {
                        Error::new(Kind::OverflowError, "%c arg not in range(0x110000)")
                    })?
                    .to_string()
            } else if let Some(name) = scalar_name(value.kind) {
                return Err(Error::new(
                    Kind::TypeError,
                    format!("%c requires an int or a unicode character, not {name}"),
                ));
            } else {
                return Err(Error::new(Kind::TypeError, "%c requires int or char"));
            }
        }
        'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
            let n = match value.kind.raw() {
                7 => f64::from_bits(value.word as u64),
                2 => {
                    // SAFETY: this arm has an integer descriptor and a live owner.
                    let n = unsafe { Int::borrow(value.word) };
                    crate::floats::int_to_float(&n)?
                }
                1 => value.word as f64,
                _ => {
                    let message = scalar_name(value.kind).map_or_else(
                        || "must be real number".to_owned(),
                        |name| format!("must be real number, not {name}"),
                    );
                    return Err(Error::new(Kind::TypeError, message));
                }
            };
            let precision = precision.unwrap_or(6);
            let spec = format!("%{flags}{width}.{precision}{c}");
            let pattern = std::ffi::CString::new(spec).unwrap();
            // SAFETY: pattern is assembled from flags, a decimal precision and one double conversion.
            let len = unsafe { libc::snprintf(std::ptr::null_mut(), 0, pattern.as_ptr(), n) };
            if len < 0 {
                return Err(Error::new(Kind::ValueError, "float formatting failed"));
            }
            let mut bytes = vec![0u8; len as usize + 1];
            // SAFETY: snprintf reported the exact writable capacity including the trailing NUL.
            unsafe { libc::snprintf(bytes.as_mut_ptr().cast(), bytes.len(), pattern.as_ptr(), n) };
            bytes.pop();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => {
            let message = if let Some(full) = full {
                let index = full[..full.len() - fmt.len() + i].chars().count();
                let printable = if c.is_ascii_graphic() || c == ' ' {
                    c
                } else {
                    '?'
                };
                format!(
                    "unsupported format character '{printable}' (0x{:x}) at index {index}",
                    c as u32
                )
            } else {
                format!("unsupported format character '{c}'")
            };
            return Err(Error::new(Kind::ValueError, message));
        }
    };
    let padding = width.saturating_sub(text.chars().count());
    if padding > 0 {
        let mut spaces = self::padding(padding, ' ')?;
        if flags.contains('-') {
            append(&mut text, &spaces)?;
        } else {
            append(&mut spaces, &text)?;
            text = spaces;
        }
    }
    Ok((text, &fmt[i + c.len_utf8()..]))
}

/// # Safety
/// Every pair is a rooted value and its exact word descriptor. The returned object owns its fields.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn imre_logrecord_new(
    name: usize,
    nk: u32,
    level: usize,
    lk: u32,
    path: usize,
    pk: u32,
    line: usize,
    ik: u32,
    msg: usize,
    mk: u32,
    args: usize,
    ak: u32,
    exc: usize,
    ek: u32,
    func: usize,
    fk: u32,
    stack: usize,
    sk: u32,
) -> *const Object {
    let fields = [
        (name, nk),
        (level, lk),
        (path, pk),
        (line, ik),
        (msg, mk),
        (args, ak),
        (exc, ek),
        (func, fk),
        (stack, sk),
    ];
    // SAFETY: the caller roots each value under the matching descriptor until it has been acquired.
    let record = Record::new(fields.map(|(v, k)| unsafe { WordKind::new(k).acquire(v) }));
    finish(
        record.map(|record| {
            crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                Storage::LogRecord(Box::new(record)),
            ))))
        }),
        std::ptr::null(),
    )
}

/// Accesses a rooted record for the duration of a nonreentrant operation.
pub(super) unsafe fn with_record<T>(record: *const Object, read: impl FnOnce(&Record) -> T) -> T {
    // SAFETY: callers root the record and do not mutate it until read returns.
    unsafe {
        object(record).0.with(|s| match s {
            Storage::LogRecord(r) => read(r),
            _ => super::super::fail("SystemError", "expected a LogRecord"),
        })
    }
}

/// # Safety
/// Record, key and value are rooted, with the supplied exact value descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_put(
    record: *const Object,
    key: *const Str,
    word: usize,
    kind: u32,
) {
    // SAFETY: acquire before mutation; release the displaced owner outside the storage borrow.
    unsafe {
        let value = WordKind::new(kind).acquire(word);
        finish(
            object(record).0.with_mut(|s| match s {
                Storage::LogRecord(r) => r.fields.insert((&*key).to_string(), value),
                _ => unreachable!(),
            }),
            (),
        );
    }
}

/// # Safety
/// The record and a string-key dictionary are rooted independently until the update finishes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_update(record: *const Object, mapping: *const Object) {
    // SAFETY: copy owners before mutating a distinct object; no callback reenters either borrow.
    unsafe {
        let fields = object(mapping)
            .0
            .with(|s| s.dictionary().owned_string_entries());
        finish(
            object(record).0.with_mut(|s| {
                let Storage::LogRecord(r) = s else {
                    unreachable!()
                };
                for (key, value) in fields {
                    r.fields.insert(key, value)?;
                }
                Ok(())
            }),
            (),
        );
    }
}

/// Expose the record's live, insertion-ordered attribute dictionary.
///
/// # Safety
/// The caller holds a native record until the mapping takes its own strong owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_dictionary(record: *const Object) -> *const Object {
    // SAFETY: no user code runs while the record switches to its dictionary representation.
    let result = unsafe {
        object(record).0.with_mut(|s| {
            let Storage::LogRecord(r) = s else {
                unreachable!()
            };
            r.fields.dictionary()
        })
    };
    finish(result.map(ObjectOwner::into_raw), std::ptr::null())
}

/// Attributes supplied by the pinned LogRecord class and its object base.
fn class_attribute(name: &str) -> bool {
    matches!(
        name,
        "getMessage"
            | "__module__"
            | "__firstlineno__"
            | "__doc__"
            | "__static_attributes__"
            | "__dict__"
            | "__weakref__"
            | "__new__"
            | "__repr__"
            | "__hash__"
            | "__str__"
            | "__getattribute__"
            | "__setattr__"
            | "__delattr__"
            | "__lt__"
            | "__le__"
            | "__eq__"
            | "__ne__"
            | "__gt__"
            | "__ge__"
            | "__init__"
            | "__reduce_ex__"
            | "__reduce__"
            | "__getstate__"
            | "__subclasshook__"
            | "__init_subclass__"
            | "__format__"
            | "__sizeof__"
            | "__dir__"
            | "__class__"
    )
}

/// Test attribute presence on a concrete LogRecord, including inherited descriptors.
///
/// # Safety
/// The caller roots the record and exact string for the duration of the lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_hasattr(record: *const Object, key: *const Str) -> bool {
    // SAFETY: neither lookup calls user code or takes ownership of its inputs.
    unsafe {
        let key = &*key;
        class_attribute(key.as_str()) || with_record(record, |r| r.fields.get_text(key).is_some())
    }
}

/// Return a data attribute as a retained Python value, or the supplied default if absent.
///
/// # Safety
/// The record, key and optional retained-value default are rooted. The result transfers an owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_field_view(
    record: *const Object,
    key: *const Str,
    default: *const Object,
) -> *const Object {
    // SAFETY: both arguments remain rooted until the field has its own owner.
    let (key, field) = unsafe {
        let key = &*key;
        (key, with_record(record, |r| r.fields.get_text(key)))
    };
    match field {
        Some(value) => finish(
            ObjectOwner::try_new(Object(StorageCell::new(Storage::ObjectValue(value))))
                .map(ObjectOwner::into_raw)
                .map_err(|_| Error::memory_error()),
            std::ptr::null(),
        ),
        None if !default.is_null() => {
            // SAFETY: default is a rooted view; transfer one new owner of the same value.
            ObjectOwner::into_raw(unsafe { ObjectOwner::acquire_word(default as usize) })
        }
        None => finish(Err(Fields::missing(key.as_str())), std::ptr::null()),
    }
}

/// Add Logger.makeRecord's fields without replacing existing or formatter-reserved names.
///
/// # Safety
/// The caller holds the record and a string-key dictionary, or zero for None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_extra(record: *const Object, mapping: usize) {
    if mapping == 0 {
        return;
    }
    // SAFETY: acquiring the mapping values ends its borrow before any record is changed.
    let fields = unsafe {
        object(mapping as *const Object)
            .0
            .with(|s| s.dictionary().owned_string_entries())
    };
    // SAFETY: the record, original keys and all values remain rooted through mutation.
    finish(
        unsafe {
            object(record).0.with_mut(|s| {
                let Storage::LogRecord(r) = s else {
                    unreachable!()
                };
                for (key, value) in fields {
                    if matches!(key.as_str(), "message" | "asctime") || r.fields.get(&key).is_some()
                    {
                        return Err(Error::key(string(&format!(
                            "Attempt to overwrite {} in LogRecord",
                            super::super::exceptions::string_repr(&key)
                        ))));
                    }
                    r.fields.insert(key, value)?;
                }
                Ok(())
            })
        },
        (),
    );
}

fn read(value: &OwnedWord, want: u32) -> Result<usize> {
    let kind = value.kind.raw();
    if want == kind {
        return Ok(value.clone().into_word());
    }
    if want & NULLABLE != 0 {
        if kind == 0 {
            return Ok(0);
        }
        if want & !NULLABLE == kind {
            return Ok(match kind {
                1 => value.word + 1,
                7 => Rc::into_raw(Rc::new(f64::from_bits(value.word as u64))) as usize,
                _ => value.clone().into_word(),
            });
        }
    }
    if kind & NULLABLE != 0 && value.word != 0 && kind & !NULLABLE == want {
        return Ok(match want {
            1 => value.word - 1,
            // SAFETY: a present nullable float is one rooted Rc<f64>.
            7 => unsafe { (*(value.word as *const f64)).to_bits() as usize },
            _ => value.clone().into_word(),
        });
    }
    Err(Error::new(
        Kind::TypeError,
        "log record field has a different value type",
    ))
}

/// # Safety
/// The record and key stay rooted. A successful result transfers an owner of exactly `kind`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_read(
    record: *const Object,
    key: *const Str,
    kind: u32,
) -> usize {
    // SAFETY: reading and acquiring a field neither calls user code nor mutates the receiver.
    let result = unsafe {
        with_record(record, |r| {
            let key = (&*key).as_str();
            r.fields
                .get(key)
                .ok_or_else(|| {
                    Error::new(
                        Kind::AttributeError,
                        format!("'LogRecord' object has no attribute '{key}'"),
                    )
                })
                .and_then(|v| read(&v, kind))
        })
    };
    finish(result, 0)
}

/// # Safety
/// The record is rooted until all message arguments have been rendered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_message(record: *const Object) -> *const Str {
    // SAFETY: this only reads owned values from the live record.
    let result = unsafe { with_record(record, Record::message) };
    finish(
        result.map(|text| text.into_word() as *const Str),
        std::ptr::null(),
    )
}

/// # Safety
/// The record stays rooted; the returned text has an independent owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_repr(record: *const Object) -> *const Str {
    // SAFETY: display only reads the live record and its rooted fields.
    let text = unsafe { with_record(record, Record::repr) };
    finish(
        text.and_then(|s| {
            StrOwner::try_new(Str::from(s))
                .map(StrOwner::into_raw)
                .map_err(|_| Error::allocation())
        }),
        std::ptr::null(),
    )
}

/// Reads the current module flag selected by a compiler-emitted mask.
#[unsafe(no_mangle)]
pub extern "C" fn imre_logrecord_flag(mask: usize) -> bool {
    FLAGS.get() & (Int(mask).as_small().unwrap() as u8) != 0
}

/// Changes a flag for subsequently created records; prior records keep their metadata.
#[unsafe(no_mangle)]
pub extern "C" fn imre_logrecord_set_flag(mask: usize, value: bool) {
    let mask = Int(mask).as_small().unwrap() as u8;
    FLAGS.set(if value {
        FLAGS.get() | mask
    } else {
        FLAGS.get() & !mask
    });
}

/// # Safety
/// The format and exact integer remain rooted; a successful return transfers an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_percent_int(fmt: *const Str, integer: usize) -> *const Str {
    // SAFETY: the caller roots both precise input representations for the whole operation.
    unsafe { percent(&*fmt, WordKind::new(2).acquire(integer)) }
}

/// Format an exact text operand using the same conversions as a log message.
///
/// # Safety
/// Both strings are rooted through the call; the returned string is owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_percent_text(fmt: *const Str, text: *const Str) -> *const Str {
    // SAFETY: the caller roots the format and the exact string allocation.
    unsafe { percent(&*fmt, WordKind::new(3).acquire(text as usize)) }
}

/// # Safety
/// The second operand is the rooted sequence carrying the function's positional pack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_percent_pack(fmt: &Str, pack: &Object) -> *const Str {
    // SAFETY: the formatter retains values without mutating the rooted pack.
    let rendered = unsafe {
        pack.0.with(|storage| {
            let sequence = storage.sequence()?;
            let items = sequence.values.iter().map(|&word| {
                // SAFETY: every value is rooted by the pack at its declared element kind.
                sequence.kind.acquire(word)
            });
            format_operands(fmt.as_str(), items, None)
        })
    };
    finish(
        rendered.and_then(|s| crate::checked_owned_str(Str::from(s))),
        std::ptr::null(),
    )
}

/// # Safety
/// The caller roots the format and exact dictionary throughout the conversion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_percent_dict(fmt: &Str, dict: *const Object) -> *const Str {
    // SAFETY: kind five retains the rooted dictionary through named lookups.
    unsafe { percent(fmt, WordKind::new(5).acquire(dict as usize)) }
}

/// Format the positional items of an exact tuple with the log-record formatter.
///
/// # Safety
/// Both arguments are rooted through the call. The tuple has an immutable native
/// descriptor; a successful result transfers an owned string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_percent_tuple(
    fmt: *const Str,
    tuple: *const usize,
) -> *const Str {
    // SAFETY: kind six retains the tuple and its descriptor-typed fields until formatting ends.
    unsafe { percent(&*fmt, WordKind::new(6).acquire(tuple as usize)) }
}

fn percent(fmt: &Str, value: OwnedWord) -> *const Str {
    finish(
        format_arguments(fmt.as_str(), &value).and_then(|s| crate::checked_owned_str(Str::from(s))),
        std::ptr::null(),
    )
}

#[cfg(test)]
mod tests {
    use super::milliseconds;

    #[test]
    fn milliseconds_agree_with_the_rounded_second() {
        assert_eq!(milliseconds(1_677_903_920_999_998_503).1, 999.0);
        assert_eq!(
            milliseconds(1_677_903_920_999_999_900),
            (1_677_903_921.0, 0.0)
        );
    }
}
