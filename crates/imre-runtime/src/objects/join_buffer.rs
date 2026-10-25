//! Builds a private string list's final join as its elements arrive.

use super::{Object, Storage, StorageCell, Str, exceptions, object};
use crate::{ObjectOwner, StrOwner};
use smallvec::SmallVec;
use std::ptr;

pub(super) struct JoinBuffer {
    separator: StrOwner,
    prefix: Option<StrOwner>,
    // When growing the prefix fails, keep later strings until the original
    // join. An append must not raise just because speculative copying failed.
    rest: SmallVec<[StrOwner; 2]>,
}

impl JoinBuffer {
    fn new(separator: StrOwner) -> Self {
        Self {
            separator,
            prefix: None,
            rest: SmallVec::new(),
        }
    }

    fn push(&mut self, part: StrOwner) -> bool {
        let Some(prefix) = &mut self.prefix else {
            self.prefix = Some(part);
            return true;
        };
        if self.rest.is_empty()
            && let Some(extra) = self.separator.len().checked_add(part.len())
            && let Some(total) = prefix.len().checked_add(extra)
        {
            if let Some(text) = StrOwner::get_mut(prefix) {
                if text.try_reserve(extra).is_ok() {
                    text.push_str(&self.separator);
                    text.push_str(&part);
                    return true;
                }
            } else if let Ok(mut text) = Str::try_with_capacity(total) {
                text.push_str(prefix);
                text.push_str(&self.separator);
                text.push_str(&part);
                if let Ok(owner) = StrOwner::try_new(text) {
                    *prefix = owner;
                    return true;
                }
            }
        }
        // Like ordinary list.append, only growing the list of owners is fallible here.
        if self.rest.try_reserve(1).is_err() {
            return false;
        }
        self.rest.push(part);
        true
    }

    fn finish(&mut self) -> Option<StrOwner> {
        let Some(prefix) = self.prefix.take() else {
            return Some(crate::objects::text::owned_string(""));
        };
        if self.rest.is_empty() {
            return Some(prefix);
        }
        let length = self.rest.iter().try_fold(prefix.len(), |size, part| {
            size.checked_add(self.separator.len())?
                .checked_add(part.len())
        })?;
        let mut text = Str::try_with_capacity(length).ok()?;
        text.push_str(&prefix);
        for part in &self.rest {
            text.push_str(&self.separator);
            text.push_str(part);
        }
        StrOwner::try_new(text).ok()
    }
}

/// Allocates a private collector for a statically known separator.
///
/// # Safety
/// The caller roots the string. The returned object owns one reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_join_buffer_new(separator: *const Str) -> *const Object {
    // SAFETY: the compiler supplies a rooted exact string.
    let separator = unsafe { StrOwner::acquire_word(separator as usize) };
    let buffer = allocator_api2::boxed::Box::try_new(JoinBuffer::new(separator))
        .ok()
        .and_then(|buffer| {
            ObjectOwner::try_new(Object(StorageCell::new(Storage::JoinBuffer(buffer)))).ok()
        });
    buffer.map_or_else(
        || {
            exceptions::sequence_allocation_failed();
            ptr::null()
        },
        ObjectOwner::into_raw,
    )
}

/// Appends one already-evaluated string. Earlier elements are not observable.
///
/// # Safety
/// Both arguments are rooted; the buffer must come from imre_join_buffer_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_join_buffer_append(buffer: *const Object, part: *const Str) -> i32 {
    // SAFETY: only the compiler can create this private buffer. No user code runs here.
    let ok = unsafe {
        object(buffer).0.with_mut(|storage| {
            let Storage::JoinBuffer(buffer) = storage else {
                unreachable!()
            };
            buffer.push(StrOwner::acquire_word(part as usize))
        })
    };
    if !ok {
        exceptions::sequence_allocation_failed();
    }
    i32::from(ok)
}

/// Transfers the joined string at the single final use of the collector.
///
/// # Safety
/// The rooted buffer must have no further uses; its ordinary destructor still runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_join_buffer_finish(buffer: *const Object) -> *const Str {
    // SAFETY: no references escape except the independently owned joined string.
    let joined = unsafe {
        object(buffer).0.with_mut(|storage| {
            let Storage::JoinBuffer(buffer) = storage else {
                unreachable!()
            };
            buffer.finish()
        })
    };
    joined.map_or_else(
        || {
            exceptions::sequence_allocation_failed();
            ptr::null()
        },
        StrOwner::into_raw,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_keeps_its_owner_and_later_appends_cannot_mutate_an_alias() {
        let text = StrOwner::new(Str::new("🐍"));
        let mut buf = JoinBuffer::new(StrOwner::new(Str::new("|")));
        assert!(buf.push(text.clone()));
        assert!(StrOwner::ptr_eq(&buf.finish().unwrap(), &text));
        let mut buf = JoinBuffer::new(StrOwner::new(Str::new("|")));
        for part in [&text, &crate::objects::text::owned_string(""), &text] {
            assert!(buf.push(part.clone()));
        }
        assert_eq!(text.as_str(), "🐍");
        assert_eq!(buf.finish().unwrap().as_str(), "🐍||🐍");
    }

    #[test]
    fn final_join_can_retry_a_prefix_with_later_empty_and_unicode_strings() {
        let mut buf = JoinBuffer::new(StrOwner::new(Str::new("::")));
        assert!(buf.push(StrOwner::new(Str::new("a"))));
        assert!(buf.push(crate::objects::text::owned_string("")));
        // This is the first part that didn't fit in the speculative prefix.
        buf.rest.push(StrOwner::new(Str::new("雪")));
        assert!(buf.push(crate::objects::text::owned_string("")));
        assert_eq!(buf.finish().unwrap().as_str(), "a::::雪::");
    }
}
