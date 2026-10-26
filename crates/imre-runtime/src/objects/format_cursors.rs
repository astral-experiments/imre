//! Independent cursors for the two grammars used by string.Formatter.
use super::{
    Error, Int, Kind, Object, OwnedWord, Str, finish, itertools, literal_string, records, unions,
    words::WordKind,
};
use crate::StrOwner;
use crate::format_grammar as grammar;
use records::RecordDescriptor;
use std::{cell::Cell, ops::Range};

enum Grammar {
    Markup,
    Field,
}
pub(super) struct Cursor {
    input: StrOwner,
    position: Cell<usize>,
    grammar: Grammar,
    row: &'static RecordDescriptor,
}

/// Allocates before acquiring the field owners, which transfer into the declared tuple layout.
fn tuple<const N: usize>(
    layout: &'static RecordDescriptor,
    fields: impl FnOnce() -> [usize; N],
) -> Result<OwnedWord, Error> {
    debug_assert_eq!(layout.length, N);
    // SAFETY: tuple metadata is immutable and lives for the whole program.
    let record = unsafe { records::allocate_record(layout) };
    if record.is_null() {
        return Err(Error::allocation());
    }
    for (i, word) in fields().into_iter().enumerate() {
        // SAFETY: the producer supplies exactly its declared, owned field word.
        unsafe {
            let kind = WordKind::new(*layout.kinds.add(i));
            OwnedWord::from_owned(word, kind).initialize_record_field(record.add(i));
        }
    }
    // SAFETY: the new tuple transfers its one record reference.
    Ok(unsafe { OwnedWord::from_owned(record as usize, WordKind::new(6)) })
}

fn substring(input: &StrOwner, span: Range<usize>) -> usize {
    let s = if span.start == 0 && span.end == input.len() {
        StrOwner::clone(input)
    } else {
        literal_string(&input[span])
    };
    StrOwner::into_raw(s) as usize
}

fn key(input: &StrOwner, key: grammar::Key) -> usize {
    match key {
        grammar::Key::Integer(n) => unions::integer(Int::from_usize(n)),
        grammar::Key::Text(span) => substring(input, span),
    }
}
fn syntax(message: &'static str) -> Error {
    Error::new(Kind::ValueError, message)
}

impl Cursor {
    fn create(
        input: StrOwner,
        position: usize,
        grammar: Grammar,
        row: &'static RecordDescriptor,
    ) -> crate::ObjectOwner {
        itertools::adapter(itertools::Adapter::Format(Self {
            input,
            position: Cell::new(position),
            grammar,
            row,
        }))
    }
    pub(super) fn name(&self) -> &'static str {
        match self.grammar {
            Grammar::Markup => "formatteriterator",
            Grammar::Field => "fieldnameiterator",
        }
    }
    pub(super) fn next(&self) -> Result<Option<OwnedWord>, Error> {
        let mut pos = self.position.get();
        let result = self.read_at(&mut pos);
        self.position.set(pos);
        result
    }
    fn read_at(&self, pos: &mut usize) -> Result<Option<OwnedWord>, Error> {
        let input = &self.input;
        match self.grammar {
            Grammar::Markup => grammar::markup(input, pos)
                .map_err(syntax)?
                .map(|row| {
                    tuple(self.row, || {
                        let part = |s: Option<Range<usize>>| s.map_or(0, |s| substring(input, s));
                        [
                            substring(input, row.literal),
                            part(row.field),
                            part(row.spec),
                            part(row.conversion),
                        ]
                    })
                })
                .transpose(),
            Grammar::Field => grammar::field(input, pos)
                .map_err(syntax)?
                .map(|(attr, item)| tuple(self.row, || [usize::from(attr), key(input, item)]))
                .transpose(),
        }
    }
}

/// # Safety
/// The caller roots the string and supplies the program's immutable parser tuple layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_parser(
    input: *const Str,
    row: &'static RecordDescriptor,
) -> *const Object {
    // SAFETY: the caller roots the input; the cursor acquires another reference.
    let input = unsafe { StrOwner::acquire_word(input as usize) };
    crate::ObjectOwner::into_raw(Cursor::create(input, 0, Grammar::Markup, row))
}

/// # Safety
/// The caller roots the string and supplies program-lifetime descriptors for
/// the (first key, cursor) tuple and (is attribute, key) rows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_formatter_field_name_split(
    input: *const Str,
    outer: &'static RecordDescriptor,
    row: &'static RecordDescriptor,
) -> *mut usize {
    // SAFETY: the caller roots the input; the cursor acquires another reference.
    let input = unsafe { StrOwner::acquire_word(input as usize) };
    let value = grammar::first(&input)
        .map_err(syntax)
        .and_then(|(first, pos)| {
            tuple(outer, || {
                [
                    key(&input, first),
                    crate::ObjectOwner::into_raw(Cursor::create(input, pos, Grammar::Field, row))
                        as usize,
                ]
            })
        })
        .map(|value| value.into_word() as *mut usize);
    finish(value, std::ptr::null_mut())
}
