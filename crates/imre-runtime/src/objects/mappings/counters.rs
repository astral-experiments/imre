//! Multiset operators on exact keys and integer counts.

use super::*;

const ADD: u32 = 0;
const SUBTRACT: u32 = 1;
const UNION: u32 = 2;
const INTERSECTION: u32 = 3;
const POSITIVE: u32 = 4;
const NEGATIVE: u32 = 5;

/// Both borrowed words are rooted integers. Return an independent count.
unsafe fn combine(a: usize, b: usize, op: u32) -> Int {
    // SAFETY: exact integer operations cannot call back into the source mappings.
    unsafe {
        match op {
            ADD | SUBTRACT => Int(crate::imre_int_binary(a, b, op)),
            UNION | INTERSECTION => {
                let smaller = crate::imre_int_compare(a, b) < 0;
                let word = if smaller == (op == INTERSECTION) {
                    a
                } else {
                    b
                };
                (*Int::borrow(word)).clone()
            }
            POSITIVE => (*Int::borrow(a)).clone(),
            NEGATIVE => Int(crate::imre_int_binary(
                Int::small(0).unwrap().0,
                a,
                SUBTRACT,
            )),
            _ => unreachable!("unknown Counter operator"),
        }
    }
}

/// Read both inputs in insertion order, emitting only positive result counts.
unsafe fn fresh(left: &Mapping, right: &Mapping, op: u32) -> Result<*const Object, Error> {
    // SAFETY: the constructor transfers its fresh strong reference.
    let result = unsafe { crate::ObjectOwner::from_raw(imre_counter_new(left.tables.key_kind())) };
    let zero = Int::small(0).unwrap().0;
    let append = |key, a, b| {
        // SAFETY: immutable input tables root the borrowed words; result is fresh.
        unsafe {
            let count = combine(a, b, op);
            if crate::imre_int_compare(count.0, zero) > 0 {
                dict_set(crate::ObjectOwner::as_ptr(&result), key, count.0)?;
            }
        }
        Ok::<(), Error>(())
    };
    left.tables.try_for_each(|key, count| {
        let other = if op < POSITIVE {
            // SAFETY: the two inputs share a key representation and remain rooted.
            unsafe { right.tables.lookup(key) }
        } else {
            None
        };
        append(key, count, other.unwrap_or(zero))
    })?;
    if matches!(op, ADD | SUBTRACT | UNION) {
        right.tables.try_for_each(|key, count| {
            // SAFETY: both inputs remain immutable and root these keys.
            if unsafe { left.tables.lookup(key) }.is_none() {
                append(key, zero, count)?;
            }
            Ok::<(), Error>(())
        })?;
    }
    Ok(crate::ObjectOwner::into_raw(result))
}

/// Mutate the existing table so aliases and active dictionary cursors survive.
unsafe fn inplace(
    left: *const Object,
    right: *const Object,
    op: u32,
) -> Result<*const Object, Error> {
    let (source, other) = if op == INTERSECTION {
        (left, right)
    } else {
        (right, left)
    };
    let zero = Int::small(0).unwrap().0;
    // SAFETY: the scan never changes the source's keys. In the aliasing case
    // the old count stays rooted until the new count has been computed.
    unsafe {
        let mut position = 0;
        while let Some((next, key, word)) = object(source)
            .0
            .with(|s| s.dictionary().tables.next(position))
        {
            position = next;
            let found = Int(imre_dict_get_default(other, key.word, zero));
            let (a, b) = if op == INTERSECTION {
                (word, found.0)
            } else {
                (found.0, word)
            };
            if op == UNION && crate::imre_int_compare(a, b) >= 0 {
                continue;
            }
            let count = combine(a, b, op);
            dict_set(left, key.word, count.0)?;
        }
        // Deletion may compact the table. Register a cursor so it tracks the
        // next surviving entry across each compaction, including external cursors.
        let cursor = object(left)
            .0
            .with_mut(|s| s.dictionary_mut().tables.iterator_position());
        while let Some((next, key, count)) = object(left)
            .0
            .with(|s| s.dictionary().tables.next(cursor.get()))
        {
            cursor.set(next);
            if crate::imre_int_compare(count, zero) <= 0 {
                imre_dict_delete(left, key.word);
            }
        }
        crate::ObjectOwner::increment_strong_count(left);
    }
    Ok(left)
}

/// Computes a fresh or in-place multiset result, dropping nonpositive counts.
///
/// # Safety
/// Left roots a Counter, right an integer-valued mapping with the same key kind.
/// The compiler passes left for both arguments of a unary operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_counter_math(
    left: *const Object,
    right: *const Object,
    op: u32,
    mutate: u32,
) -> *const Object {
    // SAFETY: fresh writes to a distinct table. Do not reenter the storage cell
    // when both inputs alias. The in-place path snapshots entries before writes.
    let result = unsafe {
        if mutate != 0 {
            inplace(left, right, op)
        } else if left == right {
            object(left)
                .0
                .with(|s| fresh(s.dictionary(), s.dictionary(), op))
        } else {
            object(left).0.with(|a| {
                object(right)
                    .0
                    .with(|b| fresh(a.dictionary(), b.dictionary(), op))
            })
        }
    };
    result.unwrap_or_else(|error| {
        error.raise();
        std::ptr::null()
    })
}
