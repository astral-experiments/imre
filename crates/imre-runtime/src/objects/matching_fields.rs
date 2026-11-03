//! A checked enumerate over split text whose body is guarded by substring membership.
use super::words::WordKind;
use super::{Error, Int, Object, OwnedWord, Storage, StorageCell, Str, StrOwner, owned_line};

pub(super) struct MatchingFields {
    source: StrOwner,
    needle: StrOwner,
    separator: u8,
    start: Int,
    offset: Option<usize>,
    index: usize,
}

impl MatchingFields {
    pub(super) fn next(&mut self) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, Error> {
        let Some(offset) = self.offset else {
            return Ok(None);
        };
        let text = &self.source.as_bytes()[offset..];
        let hit = memchr::memmem::find(text, self.needle.as_bytes());
        // Yield the final field even on a miss: Python keeps both loop targets bound.
        let prefix = &text[..hit.unwrap_or(text.len())];
        let index = self.index + memchr::memchr_iter(self.separator, prefix).count();
        let begin = memchr::memrchr(self.separator, prefix).map_or(0, |last| last + 1);
        let end = hit
            .and_then(|pos| {
                let after = pos + self.needle.len();
                memchr::memchr(self.separator, &text[after..]).map(|delta| after + delta)
            })
            .unwrap_or(text.len());
        let field = owned_line(&self.source, offset + begin..offset + end)?;
        let index_word = crate::integer_binary(&self.start, &Int::from_usize(index), 0)?;
        self.index = index + 1;
        self.offset = (end < text.len()).then_some(offset + end + 1);
        // SAFETY: each word transfers an owned value of the declared concrete kind.
        unsafe {
            Ok(Some((
                OwnedWord::from_owned(index_word.into_raw(), WordKind::new(2)),
                Some(OwnedWord::from_owned(
                    StrOwner::into_raw(field) as usize,
                    WordKind::new(3),
                )),
            )))
        }
    }
}

/// # Safety
/// The caller roots two strings and an integer. The checked needle is nonempty
/// and contains no separator byte. The iterator acquires its own references.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_str_matching_split_iter(
    text: *const Str,
    needle: *const Str,
    start: usize,
    separator: u8,
) -> *const Object {
    // SAFETY: all three operands remain rooted until the iterator has acquired them.
    let (source, needle, start) = unsafe {
        (
            StrOwner::acquire_word(text as usize),
            StrOwner::acquire_word(needle as usize),
            Int::clone(&Int::borrow(start)),
        )
    };
    let fields = MatchingFields {
        source,
        needle,
        separator,
        start,
        offset: Some(0),
        index: 0,
    };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::MatchingFields(fields),
    ))))
}
