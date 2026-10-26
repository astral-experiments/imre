//! A log record owns one native Python value per attribute, including user-supplied extras.
use crate::StrOwner;
use std::cell::Cell;
use std::collections::HashMap;
thread_local! {static FLAGS:Cell<u8> = const {Cell::new(31)};}
use num_traits::ToPrimitive;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::exceptions::Result;
use super::super::words::{NULLABLE, OwnedWord, WordKind};
use super::super::{
    Bytes, Error, Int, Kind, List, Object, Storage, StorageCell, Str, finish, object, records,
    values,
};

pub(in crate::objects) struct Record {
    pub(super) fields: HashMap<String, OwnedWord>,
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

pub(super) fn display(v: &OwnedWord, repr: bool) -> String {
    if !repr && v.kind.raw() & !NULLABLE == 3 && v.word != 0 {
        // SAFETY: v owns this exact string.
        return unsafe { &*(v.word as *const Str) }.to_string();
    }
    let mut out = String::new();
    // SAFETY: v keeps the full value graph alive until formatting returns.
    unsafe { values::write_repr(&mut out, v.word, v.kind) };
    out
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
        let key = index.map_or_else(|| string(key), integer);
        // SAFETY: the checked dictionary and the lookup key remain rooted.
        return unsafe {
            object(value.word as *const Object)
                .0
                .with_mut(|s| s.dictionary_mut().subscript_value(&key))
        }?
        .ok_or_else(|| {
            Error::new(
                Kind::ValueError,
                format!(
                    "Formatting field not found in record: {}",
                    display(&key, true)
                ),
            )
        });
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
                    let words = storage.sequence();
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
        3 => !display(v, false).is_empty(),
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
    fn new([name, level, path, line, msg, mut args, exc, func, stack]: [OwnedWord; 9]) -> Self {
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
        let (filename, module) = if path.kind.raw() == 3 {
            let path = display(&path, false);
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
                super::levels::number_for(name).map_or_else(
                    || string(&format!("Level {name}")),
                    |n| owned(n.into_raw(), 2),
                )
            }
            _ => string(&format!("Level {}", display(&level, false))),
        };
        // SAFETY: pthread_self has no preconditions and does not transfer ownership.
        let thread = unsafe { libc::pthread_self() } as usize;
        let flags = FLAGS.get();
        let fields = [
            ("name", name),
            ("levelname", levelname),
            ("levelno", level),
            ("pathname", path),
            ("lineno", line),
            ("msg", msg),
            ("args", args),
            ("exc_info", exc),
            ("funcName", func),
            ("stack_info", stack),
            ("filename", filename),
            ("module", module),
            ("exc_text", owned(0, 0)),
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
                "process",
                if flags & 2 != 0 {
                    integer(std::process::id() as usize)
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
            ("taskName", owned(0, 0)),
        ];
        Self {
            fields: fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        }
    }

    fn repr(&self) -> String {
        let get = |k: &str| display(&self.fields[k], false);
        format!(
            "<LogRecord: {}, {}, {}, {}, \"{}\">",
            get("name"),
            get("levelno"),
            get("pathname"),
            get("lineno"),
            get("msg")
        )
    }

    pub(super) fn message(&self) -> Result<String> {
        let msg = display(&self.fields["msg"], false);
        let args = &self.fields["args"];
        if !truth(args) {
            return Ok(msg);
        }
        format_arguments(&msg, args)
    }
}

/// Percent formatting is shared by log messages and ordinary string expressions. Callers
/// decide whether an empty argument tuple bypasses formatting (only LogRecord does that).
fn format_arguments(msg: &str, args: &OwnedWord) -> Result<String> {
    let tuple = tuple_len(args);
    let mut positional = (0..tuple.unwrap_or(1)).map(|i| {
        if tuple.is_some() {
            tuple_item(args, i)
        } else {
            args.clone()
        }
    });
    let mut out = String::new();
    let mut input = msg;
    while let Some((literal, rest)) = input.split_once('%') {
        out.push_str(literal);
        if let Some(rest) = rest.strip_prefix('%') {
            out.push('%');
            input = rest;
            continue;
        }
        let (piece, tail) = if let Some(rest) = rest.strip_prefix('(') {
            if !is_dict(args) {
                return Err(Error::new(Kind::TypeError, "format requires a mapping"));
            }
            let (key, rest) = mapping_key(rest)?;
            // A named conversion replaces the sole positional mapping with
            // the looked-up value. Later named conversions can still use args.
            let _ = positional.next();
            percent_value_in(rest, &mut std::iter::once(dict_item(args, key)?), Some(msg))?
        } else {
            percent_value_in(rest, &mut positional, Some(msg))?
        };
        out.push_str(&piece);
        input = tail;
    }
    if !is_dict(args) && positional.len() != 0 {
        return Err(Error::new(
            Kind::TypeError,
            "not all arguments converted during string formatting",
        ));
    }
    out.push_str(input);
    Ok(out)
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

/// Fetch a format operand at the point CPython consumes its width, precision or value.
fn next_argument(args: &mut impl Iterator<Item = OwnedWord>) -> Result<OwnedWord> {
    args.next()
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
        's' | 'r' | 'a' => {
            let mut s = display(value, c != 's');
            if c == 'a' && !s.is_ascii() {
                s = super::super::text::ascii_escape(&s)?;
            }
            if let Some(n) = precision {
                s = s.chars().take(n).collect();
            }
            s
        }
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
            if value.kind.raw() == 3 {
                let s = display(value, false);
                let count = s.chars().count();
                if count != 1 {
                    return Err(Error::new(
                        Kind::TypeError,
                        format!(
                            "%c requires an int or a unicode character, not a string of length {count}"
                        ),
                    ));
                }
                s
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
        let spaces = self::padding(padding, ' ')?;
        if flags.contains('-') {
            text.push_str(&spaces);
        } else {
            text.insert_str(0, &spaces);
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
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::LogRecord(Box::new(record)),
    ))))
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
        let old = object(record).0.with_mut(|s| match s {
            Storage::LogRecord(r) => r.fields.insert((&*key).to_string(), value),
            _ => unreachable!(),
        });
        drop(old);
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
        object(record).0.with_mut(|s| {
            if let Storage::LogRecord(r) = s {
                r.fields.extend(fields)
            }
        });
    }
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
                .and_then(|v| read(v, kind))
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
        result.and_then(|s| crate::checked_owned_str(Str::from(s))),
        std::ptr::null(),
    )
}

/// # Safety
/// The record stays rooted; the returned text has an independent owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_repr(record: *const Object) -> *const Str {
    // SAFETY: display only reads the live record and its rooted fields.
    let text = unsafe { with_record(record, Record::repr) };
    StrOwner::into_raw(StrOwner::new(Str::from(text.as_str())))
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
