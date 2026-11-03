//! A checked enumerate over split text whose body is guarded by substring membership.
use super::words::WordKind;
use super::{Error, Int, Object, OwnedWord, Storage, StorageCell, Str, StrOwner, owned_line};

pub(super) struct MatchingFields {
    source: StrOwner,
    needle: StrOwner,
    separator: u8,
    index: Int,
    offset: usize,
}

impl MatchingFields {
    pub(super) fn next(&mut self) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, Error> {
        let offset = self.offset;
        let Some(text) = self.source.as_bytes().get(offset..) else {
            return Ok(None);
        };
        let hit = memchr::memmem::find(text, self.needle.as_bytes());
        // Yield the final field even on a miss: Python keeps both loop targets bound.
        let prefix = &text[..hit.unwrap_or(text.len())];
        // After the first yield, the offset has already crossed one separator.
        let skipped =
            memchr::memchr_iter(self.separator, prefix).count() + usize::from(offset != 0);
        let begin = memchr::memrchr(self.separator, prefix).map_or(0, |last| last + 1);
        let end = hit
            .and_then(|pos| {
                let after = pos + self.needle.len();
                memchr::memchr(self.separator, &text[after..]).map(|delta| after + delta)
            })
            .unwrap_or(text.len());
        let field = owned_line(&self.source, offset + begin..offset + end)?;
        self.index = crate::integer_binary(&self.index, &Int::from_usize(skipped), 0)?;
        // Past the final field, the cursor is one byte beyond the source.
        self.offset = offset + end + 1;
        // SAFETY: each word transfers an owned value of the declared concrete kind.
        unsafe {
            Ok(Some((
                OwnedWord::from_owned(self.index.clone().into_raw(), WordKind::new(2)),
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
        index: start,
        offset: 0,
    };
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::MatchingFields(fields),
    ))))
}
