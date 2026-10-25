//! A log record owns one native Python value per attribute, including user-supplied extras.
use std::cell::Cell;
use std::collections::HashMap;
thread_local! {static FLAGS:Cell<u8> = const {Cell::new(15)};}
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::exceptions::Result;
use super::super::words::{NULLABLE, OwnedWord, WordKind};
use super::super::{
    Error, Int, Kind, Object, Storage, StorageCell, Str, finish, object, records, values,
};

pub(in crate::objects) struct Record {
    pub(super) fields: HashMap<String, OwnedWord>,
}

fn owned(word: usize, kind: u32) -> OwnedWord {
    // SAFETY: callers transfer one owner or an immediate scalar of the given kind.
    unsafe { OwnedWord::from_owned(word, WordKind::new(kind)) }
}

fn string(s: &str) -> OwnedWord {
    owned(Rc::into_raw(Rc::new(Str::from(s))) as usize, 3)
}
fn integer(n: usize) -> OwnedWord {
    owned(Int::from_usize(n).into_raw(), 2)
}
fn float(n: f64) -> OwnedWord {
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

fn tuple_len(v: &OwnedWord) -> Option<usize> {
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

fn tuple_item(v: &OwnedWord, index: usize) -> OwnedWord {
    assert!(tuple_len(v).is_some_and(|len| index < len));
    // SAFETY: the live tuple supplies a descriptor and valid slot at this index.
    unsafe {
        let data = v.word as *const usize;
        let k = WordKind::new(*records::descriptor(data).kinds.add(index));
        k.acquire(k.read_record_field(data.add(index)))
    }
}

fn is_dict(v: &OwnedWord) -> bool {
    // SAFETY: a nonzero kind-five word owns an Object, examined without mutation.
    v.kind.raw() & !NULLABLE == 5
        && v.word != 0
        && unsafe {
            object(v.word as *const Object)
                .0
                .with(|s| matches!(s, Storage::Dictionary(_)))
        }
}

fn dict_item(v: &OwnedWord, key: &str) -> Result<OwnedWord> {
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

fn truth(v: &OwnedWord) -> bool {
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
                owned(Rc::into_raw(super::levels::level_name(&number)) as usize, 3)
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
        let tuple = tuple_len(args);
        let mut pos = 0;
        let mut out = String::new();
        let mut input = msg.as_str();
        while let Some((literal, rest)) = input.split_once('%') {
            out.push_str(literal);
            if let Some(rest) = rest.strip_prefix('%') {
                out.push('%');
                input = rest;
                continue;
            }
            let (value, rest) = if let Some(rest) = rest.strip_prefix('(') {
                let (key, rest) = rest
                    .split_once(')')
                    .ok_or_else(|| Error::new(Kind::ValueError, "incomplete format key"))?;
                (dict_item(args, key)?, rest)
            } else {
                if pos >= tuple.unwrap_or(1) {
                    return Err(Error::new(
                        Kind::TypeError,
                        "not enough arguments for format string",
                    ));
                }
                let val = if tuple.is_some() {
                    tuple_item(args, pos)
                } else {
                    args.clone()
                };
                pos += 1;
                (val, rest)
            };
            let (piece, tail) = percent_value(rest, &value)?;
            out.push_str(&piece);
            input = tail;
        }
        if tuple.is_some_and(|n| pos < n) {
            return Err(Error::new(
                Kind::TypeError,
                "not all arguments converted during string formatting",
            ));
        }
        out.push_str(input);
        Ok(out)
    }
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
    let mut chars = fmt.char_indices().peekable();
    let mut flags = String::new();
    while chars.peek().is_some_and(|(_, c)| "#0-+ ".contains(*c)) {
        flags.push(chars.next().unwrap().1);
    }
    let mut width = String::new();
    while chars.peek().is_some_and(|(_, c)| c.is_ascii_digit()) {
        width.push(chars.next().unwrap().1);
    }
    let width = width.parse::<usize>().unwrap_or(0);
    let precision = if chars.peek().is_some_and(|(_, c)| *c == '.') {
        chars.next();
        let mut n = String::new();
        while chars.peek().is_some_and(|(_, c)| c.is_ascii_digit()) {
            n.push(chars.next().unwrap().1);
        }
        Some(n.parse::<usize>().unwrap_or(0))
    } else {
        None
    };
    if width > isize::MAX as usize || precision.is_some_and(|p| p > i32::MAX as usize) {
        return Err(Error::new(
            Kind::OverflowError,
            "formatted string is too long",
        ));
    }
    if chars.peek().is_some_and(|(_, c)| "hlL".contains(*c)) {
        chars.next();
    }
    let (i, c) = chars
        .next()
        .ok_or_else(|| Error::new(Kind::ValueError, "incomplete format"))?;
    let mut text = match c {
        's' | 'r' | 'a' => {
            let mut s = display(value, c != 's');
            if c == 'a' {
                s = s
                    .chars()
                    .map(|c| {
                        if c.is_ascii() {
                            c.to_string()
                        } else if c as u32 <= 255 {
                            format!("\\x{:02x}", c as u32)
                        } else if c as u32 <= 65535 {
                            format!("\\u{:04x}", c as u32)
                        } else {
                            format!("\\U{:08x}", c as u32)
                        }
                    })
                    .collect();
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
                    num_traits::FromPrimitive::from_f64(f64::from_bits(value.word as u64).trunc())
                        .ok_or_else(|| {
                        Error::new(Kind::ValueError, "cannot convert float to integer")
                    })?
                }
                _ => return Err(Error::new(Kind::TypeError, "format requires a number")),
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
            use num_traits::ToPrimitive;
            if value.kind.raw() == 3 {
                let s = display(value, false);
                if s.chars().count() != 1 {
                    return Err(Error::new(Kind::TypeError, "%c requires int or char"));
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
            } else {
                return Err(Error::new(Kind::TypeError, "%c requires int or char"));
            }
        }
        'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
            use num_traits::ToPrimitive;
            let n = match value.kind.raw() {
                7 => f64::from_bits(value.word as u64),
                // SAFETY: this arm has an integer descriptor and a live owner.
                2 => unsafe { Int::borrow(value.word) }
                    .big()
                    .to_f64()
                    .ok_or_else(|| {
                        Error::new(Kind::OverflowError, "int too large to convert to float")
                    })?,
                1 => value.word as f64,
                _ => return Err(Error::new(Kind::TypeError, "must be real number")),
            };
            let precision = precision.unwrap_or(6);
            let spec = format!("%{}.{precision}{c}", flags);
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
            return Err(Error::new(
                Kind::ValueError,
                format!("unsupported format character '{c}'"),
            ));
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
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::LogRecord(
        Box::new(record),
    )))))
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
        result.map(|s| Rc::into_raw(Rc::new(Str::from(s.as_str())))),
        std::ptr::null(),
    )
}

/// # Safety
/// The record stays rooted; the returned text has an independent owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_logrecord_repr(record: *const Object) -> *const Str {
    // SAFETY: display only reads the live record and its rooted fields.
    let text = unsafe { with_record(record, Record::repr) };
    Rc::into_raw(Rc::new(Str::from(text.as_str())))
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
