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
    fn next(&mut self) -> Result<Option<StrOwner>> {
        match self {
            Self::List(lines, index) => {
                // SAFETY: the concrete list and its string stay rooted during this nonreentrant read.
                let value = unsafe {
                    lines.0.with(|s| {
                        s.sequence()
                            .values
                            .get(*index)
                            .map(|&word| crate::StrOwner::acquire_word(word))
                    })
                };
                if value.is_some() {
                    *index += 1;
                } else {
                    *self = Self::End;
                }
                Ok(value)
            }
            Self::File(file) => {
                // SAFETY: this rooted file is distinct from its CSV reader; reading cannot reenter.
                let line = unsafe { file.0.with_mut(|s| s.file_mut().readline()) }?;
                Ok((!line.is_empty()).then(|| StrOwner::new(line)))
            }
            Self::Text(text, byte) => {
                let Some(ch) = text[*byte..].chars().next() else {
                    *self = Self::End;
                    return Ok(None);
                };
                let end = *byte + ch.len_utf8();
                let line = StrOwner::new(Str::new(&text[*byte..end]));
                *byte = end;
                Ok(Some(line))
            }
            Self::End => Ok(None),
        }
    }
}

struct Dialect {
    delimiter: char,
    quote: Option<char>,
    escape: Option<char>,
    doublequote: bool,
    skip_space: bool,
    strict: bool,
}

impl Dialect {
    fn new(delimiter: u32, quote: u32, escape: u32, flags: u32) -> Self {
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

struct Row {
    fields: Words,
    field: String,
    chars: usize,
    state: State,
}

impl Row {
    fn add(&mut self, ch: char) -> Result<()> {
        if self.chars == 131_072 {
            return Err(Error::new(
                Kind::CsvError,
                "field larger than field limit (131072)",
            ));
        }
        self.field
            .try_reserve(ch.len_utf8())
            .map_err(|_| Error::allocation())?;
        self.field.push(ch);
        self.chars += 1;
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        self.fields
            .values
            .try_reserve(1)
            .map_err(|_| Error::allocation())?;
        let value = Str::from(std::mem::take(&mut self.field));
        self.fields
            .values
            .push(StrOwner::into_raw(StrOwner::new(value)) as usize);
        self.chars = 0;
        Ok(())
    }

    /// None is the iterator's end-of-line marker, which need not be a newline.
    fn token(&mut self, dialect: &Dialect, token: Option<char>) -> Result<()> {
        use State::*;
        let newline = matches!(token, Some('\n' | '\r'));
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
                } else if self.state == FieldStart && token == dialect.quote {
                    self.state = Quoted;
                } else if self.state == AfterQuote && token == dialect.quote {
                    self.add(ch)?;
                    self.state = Quoted;
                } else if self.state != AfterQuote && token == dialect.escape {
                    self.state = EscapeField;
                } else if self.state == FieldStart && dialect.skip_space && token == Some(' ') {
                    // Leading delimiter spaces are skipped too.
                } else if token == Some(dialect.delimiter) {
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
                    self.add(ch)?;
                    if self.state != AfterEscapeNewline {
                        self.state = Field;
                    }
                }
            }
            Quoted => {
                if let Some(ch) = token {
                    if Some(ch) == dialect.escape {
                        self.state = EscapeQuoted;
                    } else if Some(ch) == dialect.quote {
                        self.state = if dialect.doublequote {
                            AfterQuote
                        } else {
                            Field
                        };
                    } else {
                        self.add(ch)?;
                    }
                }
            }
            EscapeField | EscapeQuoted => {
                self.add(token.unwrap_or('\n'))?;
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
        let mut row = Row {
            fields: Words::new(3),
            field: String::new(),
            chars: 0,
            state: State::Record,
        };
        loop {
            let Some(line) = self.lines.next()? else {
                if row.chars == 0 && row.state != State::Quoted {
                    return Ok(None);
                }
                if self.dialect.strict {
                    return Err(Error::new(Kind::CsvError, "unexpected end of data"));
                }
                row.save()?;
                break;
            };
            self.line_num += 1;
            for ch in line.chars() {
                row.token(&self.dialect, Some(ch))?;
            }
            row.token(&self.dialect, None)?;
            if row.state == State::Record {
                break;
            }
        }
        Ok(Some(row.fields.into_object()))
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
        )
    }
}

unsafe fn create(
    source: *const Object,
    kind: u32,
    dialect: Dialect,
    terminator: std::result::Result<&str, &str>,
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
    finish(
        terminator
            .map_err(|name| {
                Error::new(
                    Kind::TypeError,
                    format!("\"lineterminator\" must be a string, not {name}"),
                )
            })
            .and_then(|terminator| dialect.validate(terminator))
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
