//! StringIO positions count Unicode characters, including when a write overwrites
//! existing data or pads a gap after seeking past the end.

use super::*;
use crate::objects::{bytes::native_size, exceptions::string_repr, os::c_int};
use num_traits::ToPrimitive;

#[derive(Clone, Copy)]
enum Newline {
    Universal,
    Preserve,
    Lf,
    Cr,
    CrLf,
}

pub(super) struct TextMemory {
    data: Vec<char>,
    pos: usize,
    newline: Newline,
}

impl TextMemory {
    /// Translate each write independently, then overwrite by character index.
    pub(super) fn write(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let newline = self.newline;
        let translated = || {
            let mut chars = text.chars().peekable();
            let mut pending_lf = false;
            std::iter::from_fn(move || {
                if std::mem::take(&mut pending_lf) {
                    return Some('\n');
                }
                Some(match (newline, chars.next()?) {
                    (Newline::Universal, '\r') => {
                        chars.next_if_eq(&'\n');
                        '\n'
                    }
                    (Newline::Cr, '\n') => '\r',
                    (Newline::CrLf, '\n') => {
                        pending_lf = true;
                        '\r'
                    }
                    (_, c) => c,
                })
            })
        };
        let end = self
            .pos
            .checked_add(translated().count())
            .filter(|&end| end <= isize::MAX as usize)
            .ok_or_else(|| Error::new(Kind::OverflowError, "new position too large"))?;
        if end > self.data.len() {
            self.data
                .try_reserve(end - self.data.len())
                .map_err(|_| Error::allocation())?;
            self.data.resize(end, '\0');
        }
        for (slot, c) in self.data[self.pos..end].iter_mut().zip(translated()) {
            *slot = c;
        }
        self.pos = end;
        Ok(())
    }

    /// Encode only the requested slice; every element is a valid Unicode scalar.
    fn text(data: &[char]) -> Result<Str> {
        let mut text = String::new();
        let bytes = data.iter().map(|c| c.len_utf8()).sum();
        text.try_reserve_exact(bytes)
            .map_err(|_| Error::allocation())?;
        text.extend(data);
        Ok(Str::from(text))
    }

    /// Find the next configured line ending and cap the result in characters.
    pub(super) fn read(&mut self, size: Option<usize>, line: bool) -> Result<Str> {
        let remaining = self.data.get(self.pos..).unwrap_or_default();
        let mut n = remaining.len().min(size.unwrap_or(usize::MAX));
        if line {
            for (i, &c) in remaining[..n].iter().enumerate() {
                let delimiter = match self.newline {
                    Newline::Preserve if c == '\r' => {
                        1 + usize::from(remaining.get(i + 1) == Some(&'\n'))
                    }
                    Newline::Preserve | Newline::Lf | Newline::Universal if c == '\n' => 1,
                    Newline::Cr if c == '\r' => 1,
                    Newline::CrLf if c == '\r' && remaining.get(i + 1) == Some(&'\n') => 2,
                    _ => continue,
                };
                n = n.min(i + delimiter);
                break;
            }
        }
        let result = Self::text(&remaining[..n])?;
        self.pos += n;
        Ok(result)
    }
}

impl FileState {
    /// Borrow a live StringIO. Its closed-operation diagnostic has no period.
    pub(super) fn text_memory(&mut self) -> Result<&mut TextMemory> {
        match &mut self.stream {
            Some(Stream::TextMemory(data)) => Ok(data),
            None => Err(Error::new(Kind::ValueError, "I/O operation on closed file")),
            _ => unreachable!("the caller has an exact StringIO"),
        }
    }
}

/// Create a real memory-backed text stream. A missing initial value and None both
/// start empty; an omitted newline arrives from the compiler as a present LF.
///
/// # Safety
/// Each nonzero word points to a rooted native string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io(initial: usize, newline: usize) -> *const Object {
    // SAFETY: the caller retains both optional strings through construction.
    let (initial, newline) = unsafe {
        (
            (initial as *const Str).as_ref(),
            (newline as *const Str).as_ref(),
        )
    };
    let result = (|| {
        let newline = match newline.map(Str::as_str) {
            None => Newline::Universal,
            Some("") => Newline::Preserve,
            Some("\n") => Newline::Lf,
            Some("\r") => Newline::Cr,
            Some("\r\n") => Newline::CrLf,
            Some(other) => {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("illegal newline value: {}", string_repr(other)),
                ));
            }
        };
        let mut memory = TextMemory {
            data: Vec::new(),
            pos: 0,
            newline,
        };
        if let Some(initial) = initial {
            memory.write(initial)?;
            memory.pos = 0;
        }
        Ok(Rc::into_raw(
            FileState {
                fd: None,
                name: None,
                kind: FileKind::TextMemory,
                stream: Some(Stream::TextMemory(memory)),
            }
            .into_object(),
        ))
    })();
    finish(result, std::ptr::null())
}

/// Return an independent snapshot, leaving the current position untouched.
///
/// # Safety
/// The caller retains an exact StringIO through this operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_value(file: *const Object) -> *const Str {
    // SAFETY: only independently owned results escape the rooted object.
    let result = unsafe { object(file) }
        .0
        .with_mut(|s| TextMemory::text(&s.file_mut().text_memory()?.data));
    finish(result.map(|s| Rc::into_raw(Rc::new(s))), std::ptr::null())
}

/// Read a bounded number of characters or a single line, validating the size before
/// checking the stream, as the CPython argument converter does.
///
/// # Safety
/// The file and optional tagged integer remain rooted until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_read(
    file: *const Object,
    size: usize,
    line: bool,
) -> *const Str {
    let result = (|| {
        let limit = if size == 0 {
            None
        } else {
            // SAFETY: a nonzero word is a rooted canonical integer.
            let n = unsafe { Int::borrow(size) };
            usize::try_from(native_size(&n)?).ok()
        };
        // SAFETY: the file remains rooted for this synchronous read.
        unsafe { object(file) }
            .0
            .with_mut(|s| s.file_mut().text_memory()?.read(limit, line))
    })();
    finish(result.map(|s| Rc::into_raw(Rc::new(s))), std::ptr::null())
}

/// Seek in characters, without allocating storage for an oversized position.
///
/// # Safety
/// The caller roots the StringIO and both canonical integer words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_seek(
    file: *const Object,
    pos: usize,
    whence: usize,
) -> usize {
    let result = (|| {
        // SAFETY: this position is a rooted exact integer.
        let p = unsafe { Int::borrow(pos) };
        let pos = p.as_small().or_else(|| p.big().to_i64()).ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })?;
        let whence = c_int(whence)?;
        // SAFETY: the rooted file outlives this mutation.
        unsafe { object(file) }.0.with_mut(|s| {
            let m = s.file_mut().text_memory()?;
            m.pos = match (whence, pos) {
                (0, p) if p >= 0 => p as usize,
                (0, _) => {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("Negative seek position {pos}"),
                    ));
                }
                (1, 0) => m.pos,
                (2, 0) => m.data.len(),
                (1 | 2, _) => {
                    return Err(Error::new(
                        Kind::OSError,
                        "Can't do nonzero cur-relative seeks",
                    ));
                }
                _ => {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("Invalid whence ({whence}, should be 0, 1 or 2)"),
                    ));
                }
            };
            Ok(Int::from_usize(m.pos).into_raw())
        })
    })();
    finish(result, 0)
}

/// Return or truncate at the character cursor. Truncating past EOF does not grow
/// the buffer, and truncating before the cursor does not move the cursor.
///
/// # Safety
/// The file and any nonzero optional size are rooted throughout the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_position(
    file: *const Object,
    size: usize,
    truncate: bool,
) -> usize {
    let result = (|| {
        let size = if size == 0 {
            None
        } else {
            // SAFETY: a nonzero size is a rooted canonical integer.
            let n = unsafe { Int::borrow(size) };
            Some(native_size(&n)?)
        };
        // SAFETY: the caller retains the file throughout the borrow.
        unsafe { object(file) }.0.with_mut(|s| {
            let m = s.file_mut().text_memory()?;
            let n = match size {
                Some(n) => usize::try_from(n).map_err(|_| {
                    Error::new(Kind::ValueError, format!("Negative size value {n}"))
                })?,
                None => m.pos,
            };
            if truncate {
                m.data.truncate(n);
            }
            Ok(Int::from_usize(n).into_raw())
        })
    })();
    finish(result, 0)
}
