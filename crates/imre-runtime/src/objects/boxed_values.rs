//! Owning, type-erased views of native Python scalar and tuple values.
use super::exceptions::{Error, Kind};
use super::words::WordKind;
use super::{Object, Storage, StorageCell, Str, acquire_rc, records, values};
use crate::Int;
use std::rc::Rc;

/// Reads one statically described field; this works for both inline and shared record storage.
pub(super) unsafe fn tuple_item(tuple: usize, index: usize) -> (WordKind, usize) {
    let ptr = tuple as *const usize;
    // SAFETY: the caller roots a tuple and proves index is in its descriptor.
    unsafe {
        let kind = WordKind::new(*records::descriptor(ptr).kinds.add(index));
        (kind, kind.read_record_field(ptr.add(index)))
    }
}

pub(super) fn word_text(kind: WordKind, word: usize, repr: bool) -> Rc<Str> {
    if kind.raw() == 3 && !repr {
        // SAFETY: the owning field roots the exact string.
        return unsafe { acquire_rc(word) };
    }
    let mut result = String::new();
    // SAFETY: only concrete, representable field types reach this routine.
    unsafe {
        values::write_repr(&mut result, word, kind);
    }
    Rc::new(result.into())
}

/// The default CPython decimal conversion limit is tested only when a selected
/// scalar or tuple is actually formatted. Compare magnitudes before converting,
/// so a huge input cannot allocate an unbounded decimal scratch string.
pub(super) unsafe fn checked_text(kind: WordKind, word: usize, repr: bool) -> *const Str {
    static LIMIT: std::sync::LazyLock<num_bigint::BigUint> =
        std::sync::LazyLock::new(|| num_bigint::BigUint::from(10u8).pow(4300));
    // SAFETY: all descendants stay rooted in the original immutable boxed value.
    unsafe {
        let mut pending = Vec::new();
        let mut next = (kind, word);
        loop {
            let (k, w) = next;
            if k.raw() == 2 {
                let n = Int::borrow(w);
                if n.as_small().is_none() && n.big().magnitude() >= &*LIMIT {
                    return Error::new(Kind::ValueError, "Exceeds the limit (4300 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit")
                        .returning(std::ptr::null());
                }
            } else if k.raw() == 6 {
                let n = records::descriptor(w as *const usize).length;
                pending.extend((0..n).map(|i| tuple_item(w, i)));
            }
            let Some(value) = pending.pop() else { break };
            next = value;
        }
        Rc::into_raw(word_text(kind, word, repr))
    }
}

/// Project a concrete Python scalar or tuple into an owning object value.
///
/// # Safety
/// The caller roots the word with the indicated concrete representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_box_value(kind: u32, word: usize) -> *const Object {
    // SAFETY: the ABI supplies the exact representation and a live owner.
    let boxed = unsafe { WordKind::new(kind).acquire(word) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ObjectValue(
        boxed,
    )))))
}

/// Compare nested fixed tuples by their members, even across distinct native descriptors.
/// Python's bool/int equivalence applies to leaves at every depth.
pub(super) unsafe fn equal(left: (WordKind, usize), right: (WordKind, usize)) -> bool {
    let numeric = |(k, w): (WordKind, usize)| {
        if k.raw() == 1 {
            (WordKind::new(2), (w << 1) | 1)
        } else {
            (k, w)
        }
    };
    // Only unvisited siblings occupy the worklist. A wide tuple costs no
    // auxiliary space, and ordinary scalar comparisons do not allocate.
    let mut pending = smallvec::SmallVec::<[(usize, usize, usize, usize); 4]>::new();
    let mut next = (left, right);
    loop {
        let ((ka, wa), (kb, wb)) = (numeric(next.0), numeric(next.1));
        if ka != kb {
            return false;
        }
        if ka.raw() == 6 && wa != wb {
            // SAFETY: both words are rooted tuple payloads with static descriptors.
            unsafe {
                let n = records::descriptor(wa as *const usize).length;
                if n != records::descriptor(wb as *const usize).length {
                    return false;
                }
                if n != 0 {
                    if n > 1 {
                        pending.push((wa, wb, 1, n));
                    }
                    next = (tuple_item(wa, 0), tuple_item(wb, 0));
                    continue;
                }
            }
        } else {
            // SAFETY: a matching concrete scalar or identical rooted tuple.
            if !unsafe { values::equal(wa, wb, ka) } {
                return false;
            }
        }
        let Some((a, b, i, n)) = pending.pop() else {
            return true;
        };
        if i + 1 < n {
            pending.push((a, b, i + 1, n));
        }
        // SAFETY: the outer exception or comparison operand roots every tuple.
        next = unsafe { (tuple_item(a, i), tuple_item(b, i)) };
    }
}
