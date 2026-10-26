//! Acquire concrete sources at the constructor's specified point. Sequence pulls
//! reread the live length; dictionary and set cursors keep their mutation checks.

use super::*;
use std::cell::{Cell, RefCell};

pub(in crate::objects) struct Sequence {
    owner: RefCell<Option<OwnedWord>>,
    index: Cell<usize>,
}

impl Sequence {
    fn new(owner: OwnedWord) -> Self {
        Self {
            owner: RefCell::new(Some(owner)),
            index: Cell::new(0),
        }
    }

    pub(super) fn next(&self) -> Result<Option<OwnedWord>, Error> {
        let index = self.index.get();
        let item = {
            let owner = self.owner.borrow();
            let Some(owner) = owner.as_ref() else {
                return Ok(None);
            };
            // SAFETY: the constructor roots a list or homogeneous tuple of this exact
            // kind. Copy or acquire the element before releasing any storage borrow.
            unsafe {
                match owner.kind.raw() {
                    4 => (&*(owner.word as *const List))
                        .0
                        .with(|values| values.get(index).map(|n| WordKind::new(2).acquire(n.0))),
                    5 => object(owner.word as *const Object).0.with(|storage| {
                        let values = storage.sequence_storage();
                        if let Some(rows) = values.rows() {
                            return (index < rows.len())
                                .then(|| rows.owned_row(index))
                                .transpose();
                        }
                        let values = values.resolved()?;
                        Ok(values.values.get(index).map(|&v| values.kind.acquire(v)))
                    })?,
                    6 => {
                        let tuple = owner.word as *const usize;
                        let layout = records::descriptor(tuple);
                        (index < layout.length).then(|| {
                            let kind = WordKind::new(*layout.kinds.add(index));
                            kind.acquire(kind.read_record_field(tuple.add(index)))
                        })
                    }
                    _ => unreachable!(),
                }
            }
        };
        if item.is_some() {
            self.index.set(index + 1);
        } else {
            self.owner.take();
        }
        Ok(item)
    }
}

/// The compiler supplies a rooted value of the exact selected source form.
pub(super) unsafe fn acquire(mode: u32, source: usize) -> crate::ObjectOwner {
    // SAFETY: all leaf constructors return one independent owner. A cursor
    // input is already an iterator, so its position and identity are shared.
    unsafe {
        let pointer = match mode {
            0 => return crate::ObjectOwner::acquire_word(source),
            1 => sequences::imre_str_iter(source as *const Str),
            2 => bytes::imre_bytes_iter(source as *const Bytes),
            3..=5 => {
                let owner = WordKind::new(mode + 1).acquire(source);
                return adapter(Adapter::Sequence(Sequence::new(owner)));
            }
            6 => mappings::imre_dict_iter(source as *const Object, 0),
            7 => mappings::imre_set_iter(source as *const Object),
            _ => fail("SystemError", "invalid iterable representation"),
        };
        crate::ObjectOwner::from_raw(pointer)
    }
}

pub(in crate::objects) struct Progression {
    pub(super) current: RefCell<Int>,
    pub(super) step: Int,
    pub(super) stop: Option<Int>,
    ascending: bool,
}

impl Progression {
    pub(super) fn remaining(&self) -> Option<Int> {
        let stop = self.stop.as_ref()?;
        let current = self.current.borrow();
        let step = self.step.big();
        let (distance, stride) = if self.ascending {
            (
                stop.big().as_ref() - current.big().as_ref(),
                step.into_owned(),
            )
        } else {
            (current.big().as_ref() - stop.big().as_ref(), -step.as_ref())
        };
        Some(if distance.sign() != num_bigint::Sign::Plus {
            Int::from_usize(0)
        } else {
            Int::new((distance - 1u8) / stride + 1u8)
        })
    }

    pub(super) fn next(&self) -> Option<OwnedWord> {
        let mut current = self.current.borrow_mut();
        // SAFETY: the current value, limit and step remain independently rooted;
        // exact integer comparison and addition cannot reenter or raise.
        unsafe {
            if let Some(stop) = &self.stop {
                let order = crate::imre_int_compare(current.0, stop.0);
                if (self.ascending && order >= 0) || (!self.ascending && order <= 0) {
                    return None;
                }
            }
            let next = Int(crate::imre_int_binary(current.0, self.step.0, 0));
            let old = std::mem::replace(&mut *current, next);
            Some(OwnedWord::from_owned(old.into_raw(), WordKind::new(2)))
        }
    }
}

/// # Safety
/// Arguments are rooted canonical integers. A zero stop means an unbounded count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_progression(start: usize, stop: usize, step: usize) -> *const Object {
    // SAFETY: acquire the integer arguments before returning the independent cursor.
    let (start, stop, step) = unsafe {
        (
            (*Int::borrow(start)).clone(),
            (stop != 0).then(|| (*Int::borrow(stop)).clone()),
            (*Int::borrow(step)).clone(),
        )
    };
    let sign = step.big().sign();
    if stop.is_some() && sign == num_bigint::Sign::NoSign {
        return Error::new(Kind::ValueError, "range() arg 3 must not be zero")
            .returning(std::ptr::null());
    }
    crate::ObjectOwner::into_raw(adapter(Adapter::Progression(Progression {
        current: RefCell::new(start),
        step,
        stop,
        ascending: sign == num_bigint::Sign::Plus,
    })))
}
