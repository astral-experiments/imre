//! A bounded or unbounded ring of owned words. Removing an element transfers
//! its owner out of the storage borrow before destruction can run.

use std::collections::VecDeque;

use super::words::WordKind;
use super::*;

pub(super) struct Deque {
    items: VecDeque<OwnedWord>,
    kind: WordKind,
    maxlen: Option<usize>,
    state: usize,
}

impl Deque {
    /// Detach the old elements so their owners are released outside the storage borrow.
    fn take_items(&mut self) -> VecDeque<OwnedWord> {
        if !self.items.is_empty() {
            self.state = self.state.wrapping_add(1);
        }
        std::mem::take(&mut self.items)
    }

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

/// Decode the bound before changing an existing deque. Zero represents None.
unsafe fn maxlen(limit: usize) -> Result<Option<usize>, Error> {
    if limit == 0 {
        return Ok(None);
    }
    // SAFETY: the caller roots the optional integer for the duration of conversion.
    let number = unsafe { Int::borrow(limit) };
    let n = number
        .as_small()
        .map(|n| n as isize)
        .or_else(|| number.big().to_isize())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })?;
    usize::try_from(n)
        .map(Some)
        .map_err(|_| Error::new(Kind::ValueError, "maxlen must be non-negative"))
}

/// # Safety
/// A nonzero limit is a rooted tagged Python integer. Zero represents None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_new(kind: u32, limit: usize) -> *const Object {
    // SAFETY: the caller supplies the rooted optional integer.
    let maxlen = match unsafe { maxlen(limit) } {
        Ok(bound) => bound,
        Err(error) => return error.returning(std::ptr::null()),
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
    let removed = unsafe { object(value).0.with_mut(|s| s.deque_mut().take_items()) };
    drop(removed);
}

/// Replace the bound and clear an existing deque before consuming its new iterable.
///
/// # Safety
/// The caller roots the deque and the optional tagged integer through this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_deque_reset(value: *const Object, limit: usize) {
    // SAFETY: validate the caller's rooted bound before touching the receiver.
    let bound = match unsafe { maxlen(limit) } {
        Ok(bound) => bound,
        Err(error) => return error.returning(()),
    };
    // SAFETY: no element is released until aliases can see the cleared deque and its new bound.
    let removed = unsafe {
        object(value).0.with_mut(|storage| {
            let deque = storage.deque_mut();
            deque.maxlen = bound;
            deque.take_items()
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
    pub(super) fn next(&mut self) -> Result<Option<(OwnedWord, Option<OwnedWord>)>, Error> {
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
                    return Err(Error::new(
                        Kind::RuntimeError,
                        "deque mutated during iteration",
                    ));
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
