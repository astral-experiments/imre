//! StringIO positions count Unicode characters, including when a write overwrites
//! existing data or pads a gap after seeking past the end.

use super::super::records::{RecordDescriptor, allocate_record};
use super::*;
use crate::StrOwner;
use crate::objects::MemoryText;
use crate::objects::{bytes::native_size, c_int, exceptions::string_repr};
use num_traits::ToPrimitive;

const CR: u32 = b'\r' as u32;
const LF: u32 = b'\n' as u32;

#[derive(Clone, Copy)]
enum Newline {
    Universal,
    Preserve,
    Lf,
    Cr,
    CrLf,
}

pub(super) struct TextMemory {
    data: Vec<u32>,
    pos: usize,
    newline: Newline,
    seen: u8,
}

impl TextMemory {
    /// Translate each write independently, then overwrite by character index.
    pub(super) fn write(&mut self, text: &str) -> Result<()> {
        self.write_points(text.chars().map(u32::from))
    }

    fn write_points(&mut self, points: impl Iterator<Item = u32> + Clone) -> Result<()> {
        // Reserve the translated size before modifying the stream, including any gap after a seek.
        let mut previous = 0;
        let mut length = 0;
        for point in points.clone() {
            length += match (self.newline, point) {
                (Newline::Universal, LF) if previous == CR => 0,
                (Newline::CrLf, LF) => 2,
                _ => 1,
            };
            previous = point;
        }
        if length == 0 {
            return Ok(());
        }
        let end = self
            .pos
            .checked_add(length)
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or_else(|| Error::new(Kind::OverflowError, "new position too large"))?;
        if end > self.data.len() {
            self.data
                .try_reserve(end - self.data.len())
                .map_err(|_| Error::allocation())?;
            self.data.resize(end, 0);
        }
        let mut written = self.data[self.pos..end].iter_mut();
        let mut input = points.peekable();
        while let Some(ch) = input.next() {
            let out = match (self.newline, ch) {
                (Newline::Universal | Newline::Preserve, CR) if input.peek() == Some(&LF) => {
                    input.next();
                    self.seen |= 4;
                    if matches!(self.newline, Newline::Preserve) {
                        *written.next().unwrap() = CR;
                    }
                    LF
                }
                (Newline::Universal | Newline::Preserve, CR) => {
                    self.seen |= 1;
                    if matches!(self.newline, Newline::Universal) {
                        LF
                    } else {
                        CR
                    }
                }
                (Newline::Universal | Newline::Preserve, LF) => {
                    self.seen |= 2;
                    LF
                }
                (Newline::Cr, LF) => CR,
                (Newline::CrLf, LF) => {
                    *written.next().unwrap() = CR;
                    LF
                }
                _ => ch,
            };
            *written.next().unwrap() = out;
        }
        self.pos = end;
        Ok(())
    }

    /// Keep the single, fallible UTF-8 allocation for ordinary strings, and preserve
    /// every code point when an unpaired surrogate precludes UTF-8.
    fn snapshot(data: &[u32]) -> Result<MemoryText> {
        let scalars = || data.iter().copied().map(char::from_u32);
        let bytes = scalars().try_fold(0, |length, ch| Some(length + ch?.len_utf8()));
        if let Some(bytes) = bytes {
            let mut text = String::new();
            text.try_reserve_exact(bytes)
                .map_err(|_| Error::allocation())?;
            text.extend(scalars().flatten());
            Ok(MemoryText::Utf8(Str::from(text)))
        } else {
            let mut points = Vec::new();
            points
                .try_reserve_exact(data.len())
                .map_err(|_| Error::allocation())?;
            points.extend_from_slice(data);
            Ok(MemoryText::Surrogates(points))
        }
    }

    fn text(data: &[u32]) -> Result<Str> {
        match Self::snapshot(data)? {
            MemoryText::Utf8(text) => Ok(text),
            MemoryText::Surrogates(_) => Err(Error::new(
                Kind::UnicodeError,
                "surrogate in a scalar-only text result",
            )),
        }
    }

    pub(super) fn read(&mut self, size: Option<usize>, line: bool) -> Result<Str> {
        self.read_with(size, line, Self::text)
    }

    /// Find the next configured line ending and advance only after constructing the result.
    fn read_with<T>(
        &mut self,
        size: Option<usize>,
        line: bool,
        collect: impl FnOnce(&[u32]) -> Result<T>,
    ) -> Result<T> {
        let remaining = self.data.get(self.pos..).unwrap_or_default();
        let mut n = remaining.len().min(size.unwrap_or(usize::MAX));
        if line {
            for (i, &c) in remaining[..n].iter().enumerate() {
                let delimiter = match self.newline {
                    Newline::Preserve if c == CR => {
                        1 + usize::from(remaining.get(i + 1) == Some(&LF))
                    }
                    Newline::Preserve | Newline::Lf | Newline::Universal if c == LF => 1,
                    Newline::Cr if c == CR => 1,
                    Newline::CrLf if c == CR && remaining.get(i + 1) == Some(&LF) => 2,
                    _ => continue,
                };
                n = n.min(i + delimiter);
                break;
            }
        }
        let result = collect(&remaining[..n])?;
        self.pos += n;
        Ok(result)
    }
}

impl FileState {
    /// Create a native memory stream with the same newline rules for both call paths.
    pub(in crate::objects) fn string_io(
        initial: Option<&str>,
        spelling: Option<&str>,
    ) -> Result<crate::ObjectOwner> {
        // The C constructor compares a NUL-terminated spelling, but quotes
        // the entire argument in a ValueError.
        let newline = match spelling.map(|s| s.split('\0').next().unwrap_or_default()) {
            None => Newline::Universal,
            Some("") => Newline::Preserve,
            Some("\n") => Newline::Lf,
            Some("\r") => Newline::Cr,
            Some("\r\n") => Newline::CrLf,
            Some(_) => {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("illegal newline value: {}", string_repr(spelling.unwrap())),
                ));
            }
        };
        let mut memory = TextMemory {
            data: Vec::new(),
            pos: 0,
            newline,
            seen: 0,
        };
        if let Some(initial) = initial {
            memory.write(initial)?;
            memory.pos = 0;
        }
        Ok(FileState {
            fd: None,
            raw: false,
            name: None,
            encoding: None,
            errors: None,
            kind: FileKind::TextMemory,
            stream: Some(Stream::TextMemory(memory)),
        }
        .into_object())
    }

    pub(in crate::objects) fn write_string_io_points(&mut self, points: &[u32]) -> Result<()> {
        self.text_memory()?.write_points(points.iter().copied())
    }

    /// Restore already-translated code points without changing decoder history or position.
    pub(in crate::objects) fn replace_string_io_points(
        &mut self,
        points: impl Iterator<Item = u32> + Clone,
    ) -> Result<()> {
        let data = &mut self.text_memory()?.data;
        data.try_reserve(points.clone().count().saturating_sub(data.len()))
            .map_err(|_| Error::allocation())?;
        data.clear();
        data.extend(points);
        Ok(())
    }

    pub(in crate::objects) fn string_io_points(&mut self) -> Result<MemoryText> {
        TextMemory::snapshot(&self.text_memory()?.data)
    }

    pub(in crate::objects) fn read_string_io_points(
        &mut self,
        size: Option<usize>,
        line: bool,
    ) -> Result<MemoryText> {
        self.text_memory()?
            .read_with(size, line, TextMemory::snapshot)
    }

    pub(in crate::objects) fn read_string_io(
        &mut self,
        size: Option<usize>,
        line: bool,
    ) -> Result<Str> {
        self.text_memory()?.read(size, line)
    }

    pub(in crate::objects) fn string_io_position(&mut self) -> Result<usize> {
        Ok(self.text_memory()?.pos)
    }

    pub(in crate::objects) fn seek_string_io(&mut self, pos: i64, whence: i32) -> Result<usize> {
        let m = self.text_memory()?;
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
        Ok(m.pos)
    }

    pub(in crate::objects) fn truncate_string_io(&mut self, size: Option<isize>) -> Result<usize> {
        let m = self.text_memory()?;
        let n = match size {
            Some(n) => usize::try_from(n)
                .map_err(|_| Error::new(Kind::ValueError, format!("Negative size value {n}")))?,
            None => m.pos,
        };
        m.data.truncate(n);
        Ok(n)
    }

    pub(in crate::objects) fn string_io_newline(&mut self) -> Result<Option<&'static str>> {
        Ok(match self.text_memory()?.newline {
            Newline::Universal => None,
            Newline::Preserve => Some(""),
            Newline::Lf => Some("\n"),
            Newline::Cr => Some("\r"),
            Newline::CrLf => Some("\r\n"),
        })
    }

    pub(in crate::objects) fn string_io_newlines(&mut self) -> Result<u8> {
        Ok(self.text_memory()?.seen)
    }

    pub(in crate::objects) fn string_io_value(&mut self) -> Result<Str> {
        TextMemory::text(&self.text_memory()?.data)
    }

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
    let result = FileState::string_io(initial.map(Str::as_str), newline.map(Str::as_str));
    finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// Return an independent snapshot, leaving the current position untouched.
///
/// # Safety
/// The caller retains an exact StringIO through this operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_value(file: *const Object) -> *const Str {
    // SAFETY: only independently owned results escape the rooted object.
    let result = unsafe { object(file).0.with_mut(|s| s.file_mut().string_io_value()) };
    finish(
        result.map(|s| StrOwner::into_raw(StrOwner::new(s))),
        std::ptr::null(),
    )
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
        unsafe {
            object(file)
                .0
                .with_mut(|s| s.file_mut().read_string_io(limit, line))
        }
    })();
    finish(
        result.map(|s| StrOwner::into_raw(StrOwner::new(s))),
        std::ptr::null(),
    )
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
        unsafe {
            object(file).0.with_mut(|s| {
                let pos = s.file_mut().seek_string_io(pos, whence)?;
                Ok(Int::from_usize(pos).into_raw())
            })
        }
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
        unsafe {
            object(file).0.with_mut(|s| {
                let state = s.file_mut();
                let n = if truncate {
                    state.truncate_string_io(size)?
                } else {
                    state.string_io_position()?
                };
                Ok(Int::from_usize(n).into_raw())
            })
        }
    })();
    finish(result, 0)
}

/// Return an owned optional union: an untagged string, a tagged tuple, or None.
/// Decoder history is independent of cursor moves, overwrites, and truncation.
///
/// # Safety
/// The caller roots the stream and supplies static tuple descriptors for two
/// and three owned strings respectively.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_newlines(
    file: *const Object,
    pair: *const RecordDescriptor,
    triple: *const RecordDescriptor,
) -> usize {
    // SAFETY: read just the integer mask from this live stream.
    let seen = unsafe {
        object(file)
            .0
            .with_mut(|s| s.file_mut().string_io_newlines())
    };
    finish(
        seen.and_then(|seen| {
            let count = seen.count_ones();
            if count == 0 {
                return Ok(0);
            }
            let mut spellings = [(1, "\r"), (2, "\n"), (4, "\r\n")]
                .into_iter()
                .filter(|(bit, _)| seen & bit != 0)
                .map(|(_, s)| StrOwner::into_raw(StrOwner::new(Str::from(s))));
            if count == 1 {
                return Ok(spellings.next().unwrap() as usize);
            }
            // SAFETY: the caller provides an immutable descriptor with exactly
            // count owned-string fields. Populate them before exposing the tuple.
            let record = unsafe { allocate_record(if count == 2 { pair } else { triple }) };
            if record.is_null() {
                return Err(Error::allocation());
            }
            for (i, word) in spellings.enumerate() {
                // SAFETY: i is within the freshly allocated zeroed payload.
                unsafe { record.add(i).cast::<*const Str>().write(word) };
            }
            Ok(record as usize | 4)
        }),
        0,
    )
}

/// # Safety
/// The caller roots a stream and an optional size hint; the result owns each line.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_readlines(
    file: *const Object,
    size: usize,
) -> *const Object {
    // SAFETY: the hint is checked before accessing the exact rooted stream.
    let result = unsafe {
        (if size == 0 {
            Ok(None)
        } else {
            native_size(&Int::borrow(size)).map(Some)
        })
        .and_then(|hint| {
            object(file).0.with_mut(|storage| {
                let state = storage.file_mut();
                state.stream()?;
                let b = state.text_memory()?;
                let mut lines = super::super::Words::new(3);
                let mut read = 0;
                loop {
                    let before = b.pos;
                    let line = b.read(None, true)?;
                    if line.is_empty() {
                        break;
                    }
                    read += b.pos - before;
                    lines
                        .values
                        .try_reserve(1)
                        .map_err(|_| Error::allocation())?;
                    lines
                        .values
                        .push(StrOwner::into_raw(StrOwner::new(line)) as usize);
                    if hint.is_some_and(|n| n > 0 && read > n as usize) {
                        break;
                    }
                }
                Ok(lines.into_object())
            })
        })
    };
    finish(result.map(crate::ObjectOwner::into_raw), std::ptr::null())
}

/// # Safety
/// The caller roots an exact StringIO. All three capabilities are true while open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_string_io_capable(file: *const Object) -> bool {
    // SAFETY: the check only borrows the live buffer.
    finish(
        unsafe {
            object(file)
                .0
                .with_mut(|s| s.file_mut().text_memory().map(|_| true))
        },
        false,
    )
}
