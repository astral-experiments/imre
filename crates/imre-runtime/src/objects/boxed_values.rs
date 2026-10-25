//! Owning, type-erased views of native Python scalars, tuples and typed lists.
use super::exceptions::{Error, Kind};
use super::words::WordKind;
use super::{Bytes, Object, Storage, StorageCell, Str, acquire_rc, object, records, values};
use crate::Int;
use std::rc::Rc;

// A tuple may store an optional scalar in its nullable representation. Views
// expose the contained Python value, not the slot's storage tag.
/// # Safety
/// The word is rooted and has the native representation given by its tag.
pub(super) unsafe fn concrete(kind: WordKind, word: usize) -> (WordKind, usize) {
    if kind.raw() & crate::objects::words::NULLABLE == 0 {
        if kind.raw() == 5 {
            // SAFETY: kind 5 denotes a rooted native object supplied by a typed caller.
            let projected = unsafe {
                object(word as *const Object)
                    .0
                    .with(super::exceptions::projected_word)
            };
            return projected.unwrap_or((kind, word));
        }
        return (kind, word);
    }
    if word == 0 {
        return (WordKind::new(0), 0);
    }
    let base = WordKind::new(kind.raw() & !crate::objects::words::NULLABLE);
    let value = match base.raw() {
        1 => word - 1,
        // SAFETY: a present nullable float points to its rooted boxed f64.
        7 => unsafe { (*(word as *const f64)).to_bits() as usize },
        _ => word,
    };
    // SAFETY: a present option owns the concrete representation extracted above.
    unsafe { concrete(base, value) }
}

/// Reads one statically described field; this works for both inline and shared record storage.
pub(super) unsafe fn tuple_item(tuple: usize, index: usize) -> (WordKind, usize) {
    let ptr = tuple as *const usize;
    // SAFETY: the caller roots a tuple and proves index is in its descriptor.
    unsafe {
        let kind = WordKind::new(*records::descriptor(ptr).kinds.add(index));
        concrete(kind, kind.read_record_field(ptr.add(index)))
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
        // A sibling cursor per ancestor keeps scratch space proportional to depth.
        let mut pending = smallvec::SmallVec::<[(WordKind, usize, usize, usize); 4]>::new();
        let mut next = (kind, word);
        loop {
            let (k, w) = next;
            if k.raw() == 2 {
                let n = Int::borrow(w);
                if n.as_small().is_none() && n.big().magnitude() >= &*LIMIT {
                    return Error::new(Kind::ValueError, "Exceeds the limit (4300 digits) for integer string conversion; use sys.set_int_max_str_digits() to increase the limit")
                        .returning(std::ptr::null());
                }
            } else if matches!(k.raw(), 4..=6)
                && let Some(n) = length(k, w)
                && n != 0
            {
                if n > 1 {
                    pending.push((k, w, 1, n));
                }
                next = sequence_item(k, w, 0);
                continue;
            }
            let Some((k, w, i, n)) = pending.pop() else {
                break;
            };
            if i + 1 < n {
                pending.push((k, w, i + 1, n));
            }
            next = sequence_item(k, w, i);
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
    // SAFETY: the caller supplies the rooted word with this exact representation.
    let (kind, word) = unsafe { concrete(WordKind::new(kind), word) };
    // SAFETY: normalization preserves the rooted value supplied by the ABI.
    let boxed = unsafe { kind.acquire(word) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::ObjectValue(
        boxed,
    )))))
}

pub(super) fn length(kind: WordKind, word: usize) -> Option<usize> {
    // SAFETY: each admitted exact type is read through its own representation.
    unsafe {
        match kind.raw() {
            3 => Some((&*(word as *const Str)).chars().count()),
            4 => Some((&*(word as *const crate::List)).0.with(Vec::len)),
            5 => object(word as *const Object).0.with(|s| match s {
                Storage::Sequence(v) => Some(v.resolved().values.len()),
                _ => None,
            }),
            6 => Some(records::descriptor(word as *const usize).length),
            8 => Some((&*(word as *const Bytes)).len()),
            _ => None,
        }
    }
}

/// # Safety
/// The caller roots a sequence and has checked the element index against its length.
pub(super) unsafe fn sequence_item(kind: WordKind, word: usize, i: usize) -> (WordKind, usize) {
    // SAFETY: each concrete representation retains its element during the read.
    unsafe {
        match kind.raw() {
            4 => (&*(word as *const crate::List))
                .0
                .with(|v| (WordKind::new(2), v[i].0)),
            5 => object(word as *const Object).0.with(|s| {
                let Storage::Sequence(v) = s else {
                    unreachable!("admitted typed list")
                };
                let v = v.resolved();
                concrete(v.kind, v.values[i])
            }),
            6 => tuple_item(word, i),
            _ => unreachable!("list or tuple"),
        }
    }
}

/// Compare nested native sequences independent of their storage specialization.
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
    type Field = (WordKind, usize);
    let mut pending = smallvec::SmallVec::<[(Field, Field, usize, usize); 4]>::new();
    let mut next = (left, right);
    loop {
        // SAFETY: both current fields stay rooted by the original comparison operands.
        let ((ka, wa), (kb, wb)) = unsafe {
            (
                numeric(concrete(next.0.0, next.0.1)),
                numeric(concrete(next.1.0, next.1.1)),
            )
        };
        let sequence = |kind: WordKind, word| match kind.raw() {
            4 => 1,
            5 if length(kind, word).is_some() => 1,
            6 => 2,
            _ => 0,
        };
        let (a, b) = ((ka, wa), (kb, wb));
        if sequence(ka, wa) != 0 && sequence(ka, wa) == sequence(kb, wb) {
            // SAFETY: both words are rooted exact sequences, read within checked bounds.
            unsafe {
                let n = length(ka, wa).unwrap();
                if Some(n) != length(kb, wb) {
                    return false;
                }
                if !(ka == kb && wa == wb) && n != 0 {
                    if n > 1 {
                        pending.push((a, b, 1, n));
                    }
                    next = (sequence_item(ka, wa, 0), sequence_item(kb, wb, 0));
                    continue;
                }
            }
        } else if ka != kb {
            let (integer, float) = match (ka.raw(), kb.raw()) {
                (2, 7) => (wa, wb),
                (7, 2) => (wb, wa),
                _ => return false,
            };
            // SAFETY: the exact int stays rooted throughout the lossless comparison.
            let integer = unsafe { Int::borrow(integer) };
            if crate::floats::compare(&integer, f64::from_bits(float as u64)) != 0 {
                return false;
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
        next = unsafe { (sequence_item(a.0, a.1, i), sequence_item(b.0, b.1, i)) };
    }
}
