//! Lazy, string-valued CSV rows. A pull retains only the current field and row.

use crate::StrOwner;
use std::ptr;

use super::{Error, Int, Kind, Object, Storage, StorageCell, Str, Words, finish, object};

type Result<T> = std::result::Result<T, Error>;

enum Lines {
    List(crate::ObjectOwner, usize),
    File(crate::ObjectOwner),
    Text(StrOwner, usize),
    End,
}

impl Lines {
    /// The checked row parser consumes the borrowed text before the source can advance again.
    fn read_line(&mut self, parse: impl FnOnce(&str) -> Result<bool>) -> Result<Option<bool>> {
        let parsed = match self {
            Self::List(lines, index) => {
                // SAFETY: the list owns each exact str, and parsing cannot reenter the list.
                unsafe {
                    lines.0.with(|s| -> Result<_> {
                        Ok(s.sequence()?.values.get(*index).map(|&word| {
                            *index += 1;
                            parse(&*(word as *const Str))
                        }))
                    })
                }?
            }
            Self::File(file) => {
                // SAFETY: this rooted file is distinct from its CSV reader; reading cannot reenter.
                let line = unsafe { file.0.with_mut(|s| s.file_mut().readline()) }?;
                return (!line.is_empty()).then(|| parse(&line)).transpose();
            }
            Self::Text(text, byte) => {
                let rest = &text[*byte..];
                let width = rest.ceil_char_boundary(1);
                (width != 0).then(|| {
                    *byte += width;
                    parse(&rest[..width])
                })
            }
            Self::End => return Ok(None),
        };
        if parsed.is_none() {
            *self = Self::End;
        }
        parsed.transpose()
    }
}

pub(crate) struct Dialect {
    delimiter: char,
    quote: Option<char>,
    escape: Option<char>,
    doublequote: bool,
    skip_space: bool,
    strict: bool,
}

impl Dialect {
    pub(crate) fn new(delimiter: u32, quote: u32, escape: u32, flags: u32) -> Self {
        Self {
            delimiter: char::from_u32(delimiter).unwrap(),
            quote: char::from_u32(quote),
            escape: char::from_u32(escape),
            doublequote: flags & 1 != 0,
            skip_space: flags & 2 != 0,
            strict: flags & 4 != 0,
        }
    }

    fn validate(&self, terminator: &str) -> Result<()> {
        let chars = [
            ("delimiter", Some(self.delimiter)),
            ("escapechar", self.escape),
            ("quotechar", self.quote),
        ];
        for (name, value) in chars {
            if matches!(value, Some('\r' | '\n'))
                || name != "delimiter" && self.skip_space && value == Some(' ')
            {
                return Err(Error::new(Kind::ValueError, format!("bad {name} value")));
            }
            if value.is_some_and(|ch| terminator.contains(ch)) {
                return Err(Error::new(
                    Kind::ValueError,
                    format!("bad {name} or lineterminator value"),
                ));
            }
        }
        for (i, (name, value)) in chars.iter().enumerate() {
            for (other, other_value) in &chars[i + 1..] {
                if value.is_some() && value == other_value {
                    return Err(Error::new(
                        Kind::ValueError,
                        format!("bad {name} or {other} value"),
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(super) struct Reader {
    lines: Lines,
    dialect: Dialect,
    line_num: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Record,
    FieldStart,
    Field,
    Quoted,
    AfterQuote,
    EscapeField,
    EscapeQuoted,
    AfterEscapeNewline,
    TrailingNewline,
}

/// A field keeps its UTF-8 storage until a Python surrogate requires code points.
pub(crate) enum Field {
    Text(String),
    Surrogates(Vec<u32>),
}

/// One logical record. Both native object representations consume the same parser.
pub(crate) struct Row<F> {
    save_field: F,
    field: Field,
    chars: usize,
    quoted: bool,
    state: State,
}

#[cold]
fn field_limit_error(limit: i64) -> Error {
    Error::new(
        Kind::CsvError,
        format!("field larger than field limit ({limit})"),
    )
}

impl<F: FnMut(Field, bool) -> Result<()>> Row<F> {
    pub(crate) fn new(save_field: F) -> Self {
        Self {
            save_field,
            field: Field::Text(String::new()),
            chars: 0,
            quoted: false,
            state: State::Record,
        }
    }

    pub(crate) fn line(
        &mut self,
        dialect: &Dialect,
        line: impl Iterator<Item = u32>,
        limit: i64,
    ) -> Result<bool> {
        for ch in line {
            self.token(dialect, Some(ch), limit)?;
        }
        self.token(dialect, None, limit)?;
        Ok(self.state == State::Record)
    }

    pub(crate) fn eof(&mut self, dialect: &Dialect) -> Result<bool> {
        if self.chars == 0 && self.state != State::Quoted {
            return Ok(false);
        }
        if dialect.strict {
            return Err(Error::new(Kind::CsvError, "unexpected end of data"));
        }
        self.save()?;
        Ok(true)
    }

    fn add(&mut self, point: u32, limit: i64) -> Result<()> {
        if self.chars as i64 >= limit {
            return Err(field_limit_error(limit));
        }
        match &mut self.field {
            Field::Text(text) => {
                if let Some(ch) = char::from_u32(point) {
                    text.try_reserve(ch.len_utf8())
                        .map_err(|_| Error::allocation())?;
                    text.push(ch);
                } else {
                    let mut points = Vec::new();
                    points
                        .try_reserve(self.chars + 1)
                        .map_err(|_| Error::allocation())?;
                    points.extend(text.chars().map(u32::from));
                    points.push(point);
                    self.field = Field::Surrogates(points);
                }
            }
            Field::Surrogates(points) => {
                points.try_reserve(1).map_err(|_| Error::allocation())?;
                points.push(point);
            }
        }
        self.chars += 1;
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        (self.save_field)(
            std::mem::replace(&mut self.field, Field::Text(String::new())),
            self.quoted,
        )?;
        self.quoted = false;
        self.chars = 0;
        Ok(())
    }

    /// None is the iterator's end-of-line marker, which need not be a newline.
    fn token(&mut self, dialect: &Dialect, token: Option<u32>, limit: i64) -> Result<()> {
        use State::*;
        let newline = token == Some(u32::from('\n')) || token == Some(u32::from('\r'));
        if self.state == Record {
            if token.is_none() {
                return Ok(());
            }
            if newline {
                self.state = TrailingNewline;
                return Ok(());
            }
            self.state = FieldStart;
        }
        match self.state {
            FieldStart | Field | AfterEscapeNewline | AfterQuote => {
                let Some(ch) = token else {
                    if self.state != AfterEscapeNewline {
                        self.save()?;
                        self.state = Record;
                    }
                    return Ok(());
                };
                if newline {
                    self.save()?;
                    self.state = TrailingNewline;
                } else if self.state == FieldStart && token == dialect.quote.map(u32::from) {
                    self.quoted = true;
                    self.state = Quoted;
                } else if self.state == AfterQuote && token == dialect.quote.map(u32::from) {
                    self.add(ch, limit)?;
                    self.state = Quoted;
                } else if self.state != AfterQuote && token == dialect.escape.map(u32::from) {
                    self.state = EscapeField;
                } else if self.state == FieldStart
                    && dialect.skip_space
                    && token == Some(u32::from(' '))
                {
                    // Leading delimiter spaces are skipped too.
                } else if token == Some(u32::from(dialect.delimiter)) {
                    self.save()?;
                    self.state = FieldStart;
                } else if self.state == AfterQuote && dialect.strict {
                    return Err(Error::new(
                        Kind::CsvError,
                        format!(
                            "'{}' expected after '{}'",
                            dialect.delimiter,
                            dialect.quote.unwrap()
                        ),
                    ));
                } else {
                    self.add(ch, limit)?;
                    if self.state != AfterEscapeNewline {
                        self.state = Field;
                    }
                }
            }
            Quoted => {
                if let Some(ch) = token {
                    if Some(ch) == dialect.escape.map(u32::from) {
                        self.state = EscapeQuoted;
                    } else if Some(ch) == dialect.quote.map(u32::from) {
                        self.state = if dialect.doublequote {
                            AfterQuote
                        } else {
                            Field
                        };
                    } else {
                        self.add(ch, limit)?;
                    }
                }
            }
            EscapeField | EscapeQuoted => {
                self.add(token.unwrap_or(u32::from('\n')), limit)?;
                self.state = if self.state == EscapeQuoted {
                    Quoted
                } else if newline {
                    AfterEscapeNewline
                } else {
                    Field
                };
            }
            TrailingNewline => {
                if token.is_none() {
                    self.state = Record;
                } else if !newline {
                    return Err(Error::new(
                        Kind::CsvError,
                        "new-line character seen in unquoted field - do you need to open the file with newline=''?",
                    ));
                }
            }
            Record => unreachable!(),
        }
        Ok(())
    }
}

impl Reader {
    fn next(&mut self) -> Result<Option<crate::ObjectOwner>> {
        let mut fields = Words::new(3);
        let mut row = Row::new(|field, _| {
            let Field::Text(text) = field else {
                return Err(Error::new(
                    Kind::ValueError,
                    "typed CSV input must be UTF-8",
                ));
            };
            fields
                .values
                .try_reserve(1)
                .map_err(|_| Error::allocation())?;
            fields
                .values
                .push(StrOwner::into_raw(StrOwner::new(Str::from(text))) as usize);
            Ok(())
        });
        loop {
            let Some(complete) = self.lines.read_line(|line| {
                self.line_num += 1;
                row.line(&self.dialect, line.chars().map(u32::from), 131_072)
            })?
            else {
                if !row.eof(&self.dialect)? {
                    return Ok(None);
                }
                break;
            };
            if complete {
                break;
            }
        }
        Ok(Some(fields.into_object()))
    }
}

/// Create a cursor without reading the first line.
///
/// # Safety
/// Source is a rooted list[str], file or str matching kind (0, 1 or 2).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_csv_reader(
    source: *const Object,
    kind: u32,
    delimiter: u32,
    quote: u32,
    escape: u32,
    flags: u32,
) -> *const Object {
    // SAFETY: the default dialect borrows only the checked source.
    unsafe {
        create(
            source,
            kind,
            Dialect::new(delimiter, quote, escape, flags),
            Ok("\r\n"),
            flags,
        )
    }
}

/// Create a reader with an explicitly evaluated lineterminator.
///
/// # Safety
/// The source matches kind. A nonnull terminator is a rooted str; otherwise
/// bad_type is a rooted str naming the supplied Python type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_csv_reader_terminator(
    source: *const Object,
    kind: u32,
    delimiter: u32,
    quote: u32,
    escape: u32,
    flags: u32,
    terminator: *const Str,
    bad_type: *const Str,
) -> *const Object {
    // SAFETY: the compiler evaluates and roots the option and its diagnostic.
    unsafe {
        let value = terminator
            .as_ref()
            .map(Str::as_str)
            .ok_or_else(|| (*bad_type).as_str());
        create(
            source,
            kind,
            Dialect::new(delimiter, quote, escape, flags),
            value,
            flags,
        )
    }
}

unsafe fn create(
    source: *const Object,
    kind: u32,
    dialect: Dialect,
    terminator: std::result::Result<&str, &str>,
    flags: u32,
) -> *const Object {
    // SAFETY: the checked owner and kind agree; entering checks file closure as iter(file) does.
    let lines = unsafe {
        match kind {
            0 => Lines::List(crate::ObjectOwner::acquire_word(source as usize), 0),
            1 => {
                if super::files::imre_text_enter(source) == 0 {
                    return ptr::null();
                }
                Lines::File(crate::ObjectOwner::acquire_word(source as usize))
            }
            2 => Lines::Text(crate::StrOwner::acquire_word(source as usize), 0),
            _ => unreachable!(),
        }
    };
    // A missing name in the dialect registry wins over any invalid format options.
    if flags & 16 != 0 {
        return finish(
            Err(Error::new(Kind::CsvError, "unknown dialect")),
            ptr::null(),
        );
    }
    finish(
        terminator
            .map_err(|name| {
                Error::new(
                    Kind::TypeError,
                    format!("\"lineterminator\" must be a string, not {name}"),
                )
            })
            .and_then(|terminator| {
                if flags & 8 != 0 && dialect.quote.is_none() {
                    return Err(Error::new(
                        Kind::TypeError,
                        "quotechar must be set if quoting enabled",
                    ));
                }
                dialect.validate(terminator)
            })
            .map(|()| {
                crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
                    Storage::CsvReader(Box::new(Reader {
                        lines,
                        dialect,
                        line_num: 0,
                    })),
                ))))
            }),
        ptr::null(),
    )
}

/// Pull one row. Only explicit next() raises StopIteration on exhaustion.
///
/// # Safety
/// The caller roots a CSV reader throughout a nonreentrant pull.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_csv_next(reader: *const Object, required: u32) -> *const Object {
    // SAFETY: the compiler guarantees an exclusive, nonreentrant cursor mutation.
    let row = unsafe {
        object(reader).0.with_mut(|s| {
            let Storage::CsvReader(r) = s else {
                unreachable!()
            };
            r.next()
        })
    };
    finish(
        row.and_then(|row| match row {
            Some(row) => Ok(crate::ObjectOwner::into_raw(row)),
            None if required == 0 => Ok(ptr::null()),
            None => Err(Error::new(Kind::StopIteration, "")),
        }),
        ptr::null(),
    )
}

/// # Safety
/// The caller roots a CSV reader while observing its physical line count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_csv_line_number(reader: *const Object) -> usize {
    // SAFETY: reading the counter never mutates the reader or calls user code.
    unsafe {
        object(reader).0.with(|s| match s {
            Storage::CsvReader(r) => Int::from_usize(r.line_num).into_raw(),
            _ => unreachable!(),
        })
    }
}
