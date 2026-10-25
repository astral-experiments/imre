//! A bounded or unbounded ring of owned words. Removing an element transfers
//! its owner out of the storage borrow before destruction can run.

use std::collections::VecDeque;

use super::mappings::IterationError;
use super::words::WordKind;
use super::*;

pub(super) struct Deque {
    items: VecDeque<OwnedWord>,
    kind: WordKind,
    maxlen: Option<usize>,
    state: usize,
}

impl Deque {
    /// Compare one pair of elements and resume after the child comparison.
    /// The traversal retains its deque roots; neither storage borrow escapes.
    pub(super) fn compare_entry(&self, other: &Self, position: usize) -> values::Comparison {
        if self.items.len() != other.items.len() {
            return values::Comparison::Done(false);
        }
        if position == self.items.len() {
            return values::Comparison::Done(true);
        }
        if self.kind != other.kind {
            return values::Comparison::Done(false);
        }
        values::Comparison::Child {
            left: self.items[position].word,
            right: other.items[position].word,
            kind: self.kind,
            resume: Some(position + 1),
        }
    }
}

impl Storage {
    fn deque(&self) -> &Deque {
        let Self::Deque(deque) = self else {
            fail("SystemError", "expected deque storage")
        };
        deque
    }

    fn deque_mut(&mut self) -> &mut Deque {
        let Self::Deque(deque) = self else {
            fail("SystemError", "expected deque storage")
        };
        deque
    }
}

/// # Safety
/// A nonzero limit is a rooted tagged Python integer. Zero represents None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_new(kind: u32, limit: usize) -> *const Object {
    let maxlen = if limit == 0 {
        None
    } else {
        // SAFETY: the compiler supplies a rooted integer for this conversion.
        let number = unsafe { Int::borrow(limit) };
        let Some(n) = number
            .as_small()
            .map(|n| n as isize)
            .or_else(|| number.big().to_isize())
        else {
            return Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
            .returning(std::ptr::null());
        };
        if n < 0 {
            return Error::new(Kind::ValueError, "maxlen must be non-negative")
                .returning(std::ptr::null());
        }
        Some(n as usize)
    };
    let deque = Deque {
        items: VecDeque::new(),
        kind: WordKind::new(kind),
        maxlen,
        state: 0,
    };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Deque(Box::new(
        deque,
    ))))))
}

/// # Safety
/// The caller roots the deque and a word of its exact element kind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_append(
    value: *const Object,
    word: usize,
    left: u32,
    extending: u32,
) {
    // SAFETY: acquisition cannot run user code; only independent owners leave
    // the closure, and evicted owners are dropped after storage access ends.
    let result = unsafe {
        object(value).0.with_mut(|storage| {
            let deque = storage.deque_mut();
            if deque.maxlen == Some(0) {
                if extending == 0 {
                    deque.state = deque.state.wrapping_add(1);
                }
                return Ok(None);
            }
            let removed = if deque.maxlen == Some(deque.items.len()) {
                if left != 0 {
                    deque.items.pop_back()
                } else {
                    deque.items.pop_front()
                }
            } else {
                deque
                    .items
                    .try_reserve(1)
                    .map_err(|_| Error::new(Kind::MemoryError, ""))?;
                None
            };
            let item = deque.kind.acquire(word);
            if left != 0 {
                deque.items.push_front(item);
            } else {
                deque.items.push_back(item);
            }
            deque.state = deque.state.wrapping_add(1);
            Ok(removed)
        })
    };
    drop(finish(result, None));
}

/// # Safety
/// The caller roots the source deque during the shallow copy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_copy(value: *const Object) -> *const Object {
    // SAFETY: reserve before cloning, and only independent owners escape.
    let copy = unsafe {
        object(value).0.with(|storage| {
            let source = storage.deque();
            let mut items = VecDeque::new();
            items
                .try_reserve(source.items.len())
                .map_err(|_| Error::new(Kind::MemoryError, ""))?;
            items.extend(source.items.iter().cloned());
            Ok(Deque {
                items,
                kind: source.kind,
                maxlen: source.maxlen,
                state: 0,
            })
        })
    };
    finish(
        copy.map(|deque| {
            Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Deque(Box::new(
                deque,
            ))))))
        }),
        std::ptr::null(),
    )
}

/// # Safety
/// Both arguments are rooted deques of the same element type. A self-extension
/// iterates a shallow snapshot; all other extensions retain the live source.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_extend_source(
    target: *const Object,
    source: *const Object,
) -> *const Object {
    // SAFETY: the compiler roots both handles while the snapshot/owner is acquired.
    unsafe {
        if target == source {
            imre_deque_copy(source)
        } else {
            Rc::into_raw(acquire_rc::<Object>(source as usize))
        }
    }
}

/// # Safety
/// The deque is rooted. A successful call transfers one exact-kind element.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_pop(value: *const Object, left: u32) -> usize {
    // SAFETY: only the detached owner escapes mutation.
    let item = unsafe {
        object(value).0.with_mut(|storage| {
            let deque = storage.deque_mut();
            let item = if left != 0 {
                deque.items.pop_front()
            } else {
                deque.items.pop_back()
            };
            if item.is_some() {
                deque.state = deque.state.wrapping_add(1);
            }
            item
        })
    };
    item.map_or_else(
        || Error::new(Kind::IndexError, "pop from an empty deque").returning(0),
        OwnedWord::into_word,
    )
}

/// # Safety
/// The caller roots this deque. Destruction follows detachment of the old ring.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_clear(value: *const Object) {
    // SAFETY: no element is released inside this storage borrow.
    let removed = unsafe {
        object(value).0.with_mut(|storage| {
            let deque = storage.deque_mut();
            if !deque.items.is_empty() {
                deque.state = deque.state.wrapping_add(1);
            }
            std::mem::take(&mut deque.items)
        })
    };
    drop(removed);
}

/// # Safety
/// The caller roots this deque while its length is read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_len(value: *const Object) -> usize {
    // SAFETY: only the copied length escapes.
    Int::from_usize(unsafe { object(value).0.with(|storage| storage.deque().items.len()) })
        .into_raw()
}

/// # Safety
/// The caller roots this deque. The result is an owned optional integer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_maxlen(value: *const Object) -> usize {
    // SAFETY: only the copied bound escapes.
    unsafe { object(value).0.with(|storage| storage.deque().maxlen) }
        .map_or(0, |n| Int::from_usize(n).into_raw())
}

pub(super) struct DequeIterator {
    owner: Rc<Object>,
    state: usize,
    remaining: usize,
    reverse: bool,
}

impl DequeIterator {
    pub(super) fn next(
        &mut self,
    ) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, IterationError> {
        // SAFETY: a cursor and its independently rooted deque are distinct
        // allocations; cloning a word cannot run a destructor or mutate storage.
        unsafe {
            self.owner.0.with(|storage| {
                let deque = storage.deque();
                if self.reverse && self.remaining == 0 {
                    return Ok(None);
                }
                if self.state != deque.state {
                    self.remaining = 0;
                    return Err(IterationError::DequeMutation);
                }
                if self.remaining == 0 {
                    return Ok(None);
                }
                let index = if self.reverse {
                    self.remaining - 1
                } else {
                    deque.items.len() - self.remaining
                };
                self.remaining -= 1;
                Ok(Some((deque.items[index].clone(), None)))
            })
        }
    }
}

/// # Safety
/// The caller supplies a live deque; the returned cursor acquires its own root.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_iter(value: *const Object, reverse: u32) -> *const Object {
    // SAFETY: the caller roots the exact deque throughout acquisition.
    let owner = unsafe { acquire_rc::<Object>(value as usize) };
    // SAFETY: neither scalar snapshot can refer back into storage.
    let (state, remaining) = unsafe { owner.0.with(|s| (s.deque().state, s.deque().items.len())) };
    let iterator = DequeIterator {
        owner,
        state,
        remaining,
        reverse: reverse != 0,
    };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::DequeIterator(
        iterator,
    )))))
}
