//! Owning, type-erased views of native Python scalars, sequences, and eligible dataclasses.
use super::exceptions::{Error, Rendered, finish_display, rendered};
use super::words::WordKind;
use super::{Bytes, Object, Storage, StorageCell, Str, object, records, values};
use crate::Int;
use crate::StrOwner;

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

pub(super) fn word_text(kind: WordKind, word: usize, repr: bool) -> Rendered {
    if kind.raw() == 3 && !repr {
        // SAFETY: the owning field roots the exact string.
        return Ok(unsafe { StrOwner::acquire_word(word) });
    }
    rendered(word_repr(kind, word)?.into())
}

/// Render directly into the buffer used by a short-lived consumer such as stderr.
pub(super) fn word_repr(kind: WordKind, word: usize) -> Result<String, std::fmt::Error> {
    let mut result = String::new();
    // SAFETY: callers retain this exact value throughout structural formatting.
    unsafe { values::try_write_repr(&mut values::ReprWriter(&mut result), word, kind) }?;
    Ok(result)
}

/// Transfer the formatted string only after all visited fields and callbacks succeed.
pub(super) unsafe fn checked_text(kind: WordKind, word: usize, repr: bool) -> *const Str {
    // SAFETY: all descendants stay rooted in the original immutable boxed value.
    unsafe {
        if !repr
            && kind.raw() == 5
            && object(word as *const Object)
                .0
                .with(|s| matches!(s, Storage::Exception(_)))
        {
            // The storage borrow ends before a user-defined __str__ can be called.
            return super::exceptions::imre_exception_message(word as *const Object);
        }
        finish_display(word_text(kind, word, repr))
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
    crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(StorageCell::new(
        Storage::ObjectValue(boxed),
    ))))
}

/// Borrow the payload of an erased instance with the requested exact class.
///
/// # Safety
/// The caller roots the erased value for the lifetime of the returned borrow.
/// The requested descriptor is immutable compiler-emitted class metadata.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_object_record_payload(
    value: *const Object,
    expected: *const records::RecordDescriptor,
) -> *const usize {
    // SAFETY: the rooted object keeps the candidate payload alive.
    unsafe {
        (*value).0.with(|s| {
            if let Storage::ObjectValue(v) = s
                && v.kind.raw() == 6
            {
                let ptr = v.word as *const usize;
                if records::descriptor(ptr).same_class(&*expected) {
                    return ptr;
                }
            }
            std::ptr::null()
        })
    }
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
            6 => {
                let layout = records::descriptor(word as *const usize);
                layout.class_semantics().is_none().then_some(layout.length)
            }
            8 => Some((&*(word as *const Bytes)).len()),
            _ => None,
        }
    }
}

/// Observable Python length, including containers whose members are not sequence indices.
pub(super) fn python_length(kind: WordKind, word: usize) -> Option<usize> {
    length(kind, word).or_else(|| {
        if kind.raw() != 5 {
            return None;
        }
        // SAFETY: this rooted object stays live for the brief storage read.
        unsafe {
            object(word as *const Object)
                .0
                .with(Storage::mapping_or_set_len)
        }
    })
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
            6 if length(kind, word).is_some() => 2,
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
                        if pending.try_reserve(1).is_err() {
                            return Error::memory_error().returning(false);
                        }
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
