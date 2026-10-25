//! Counter.most_common keeps at most the requested number of entries. Keys
//! never participate in comparisons, and equal counts retain insertion order.

use super::*;
use crate::objects::Words;
use crate::objects::records::{RecordDescriptor, allocate_record};
use num_traits::{Signed, ToPrimitive};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

struct Entry {
    key: OwnedWord,
    count: OwnedWord,
    position: usize,
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // SAFETY: both entries own exact integer count words. The lowest count
        // and latest tie rank highest in the heap, so the worst entry is on top.
        unsafe { crate::imre_int_compare(other.count.word, self.count.word) }
            .cmp(&0)
            .then_with(|| self.position.cmp(&other.position))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for Entry {}

/// Return fresh (key, count) tuples. A zero limit word denotes None; unlike a
/// slice index, an oversized positive limit simply requests all existing keys.
///
/// # Safety
/// The caller roots an exact Counter and the optional tagged integer. The
/// static tuple descriptor describes that Counter's key and an integer count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_counter_most_common(
    counter: *const Object,
    limit: usize,
    descriptor: *const RecordDescriptor,
) -> *const Object {
    // SAFETY: the receiver stays rooted while the native table supplies borrowed
    // words. Selected entries acquire their own owners before the table borrow ends.
    let result = unsafe {
        object(counter).0.with(|storage| {
            let mapping = storage.dictionary();
            let length = mapping.tables.len();
            let n = if limit == 0 {
                length
            } else {
                let n = Int::borrow(limit);
                if let Some(small) = n.as_small() {
                    usize::try_from(small.max(0)).unwrap_or(length).min(length)
                } else if n.big().is_negative() {
                    0
                } else {
                    n.big().to_usize().unwrap_or(length).min(length)
                }
            };
            let mut heap = BinaryHeap::<Entry>::new();
            heap.try_reserve(n).ok()?;
            if n > 0 {
                let key_kind = WordKind::new(mapping.tables.key_kind());
                let mut position = 0;
                mapping.tables.all(|key, count| {
                    let entry = || Entry {
                        key: key_kind.acquire(key),
                        count: mapping.value_kind.acquire(count),
                        position,
                    };
                    if heap.len() < n {
                        heap.push(entry());
                    } else if crate::imre_int_compare(count, heap.peek().unwrap().count.word) > 0 {
                        *heap.peek_mut().unwrap() = entry();
                    }
                    position += 1;
                    true
                });
            }
            let mut result = Words::new(6);
            result.values.try_reserve(n).ok()?;
            for entry in heap.into_sorted_vec() {
                let tuple = std::ptr::NonNull::new(allocate_record(descriptor))?.as_ptr();
                // Transfer the owners to their zeroed record slots. Store references
                // as pointers, matching the field representation in the LLVM descriptor.
                let kind = entry.key.kind.raw();
                let key = entry.key.into_word();
                if matches!(kind, 3..=6 | 8) {
                    tuple.cast::<*const ()>().write(key as *const ());
                } else {
                    tuple.write(key);
                }
                tuple.add(1).write(entry.count.into_word());
                result.values.push(tuple as usize);
            }
            Some(crate::ObjectOwner::into_raw(result.into_object()))
        })
    };
    result.unwrap_or_else(|| Error::allocation().returning(std::ptr::null()))
}
