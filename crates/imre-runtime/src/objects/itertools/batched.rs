//! Gather only the next batch; an input error discards the partial tuple.

use super::*;

pub(in crate::objects) struct Batch {
    source: RefCell<Option<Rc<Object>>>,
    size: usize,
    strict: bool,
}

pub(super) struct Partial {
    source: Rc<Object>,
    items: records::tuples::TupleBuilder,
}

impl Batch {
    pub(super) fn take_source(&mut self) -> Option<Rc<Object>> {
        self.source.get_mut().take()
    }

    pub(super) fn advance(&self, frame: &mut PullFrame, item: Option<ItemResult>) -> Advance {
        let Some(item) = item else {
            let Some(source) = self.source.borrow().clone() else {
                return Advance::Return(Ok(None));
            };
            let items = match records::tuples::TupleBuilder::new(self.size) {
                Ok(items) => items,
                Err(error) => return Advance::Return(Err(error)),
            };
            *frame = PullFrame::Batch(Partial {
                source: Rc::clone(&source),
                items,
            });
            return Advance::Pull(source);
        };
        let PullFrame::Batch(partial) = frame else {
            unreachable!()
        };
        match item {
            Ok(Some(item)) => {
                partial.items.push(item);
                if partial.items.len() < self.size {
                    return Advance::Pull(Rc::clone(&partial.source));
                }
            }
            Ok(None) if partial.items.len() != 0 && !self.strict => {}
            ended => {
                self.source.take();
                return Advance::Return(match ended {
                    Err(error) => Err(error),
                    _ if partial.items.len() == 0 => Ok(None),
                    _ => Err(Error::new(Kind::ValueError, "batched(): incomplete batch")),
                });
            }
        }
        let PullFrame::Batch(partial) = std::mem::take(frame) else {
            unreachable!()
        };
        Advance::Return(partial.items.finish().map(Some))
    }
}

/// # Safety
/// The source and canonical integer size stay rooted throughout acquisition.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_batched(
    mode: u32,
    source: usize,
    size: usize,
    strict: u32,
) -> *const Object {
    // SAFETY: the caller supplies a rooted integer.
    let size = unsafe { Int::borrow(size) };
    let size = match ssize(&size) {
        Ok(size) if size >= 1 => size as usize,
        Ok(_) => {
            return Error::new(Kind::ValueError, "n must be at least one")
                .returning(std::ptr::null());
        }
        Err(error) => return error.returning(std::ptr::null()),
    };
    // SAFETY: validation precedes acquiring the compiler-described source.
    let source = match unsafe { sources::try_acquire(mode, source) } {
        Ok(source) => source,
        Err(error) => return error.returning(std::ptr::null()),
    };
    Rc::into_raw(adapter(Adapter::Batched(Batch {
        source: RefCell::new(Some(source)),
        size,
        strict: strict != 0,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_at_a_time(source: Rc<Object>) -> Rc<Object> {
        adapter(Adapter::Batched(Batch {
            source: RefCell::new(Some(source)),
            size: 1,
            strict: false,
        }))
    }

    #[test]
    fn a_reentrant_batch_pull_keeps_the_outer_partial_tuple() {
        let integer = |n| Int::small(n).unwrap().into_raw();
        // SAFETY: the immediate integers stay valid throughout construction.
        let source = unsafe { Rc::from_raw(sources::imre_progression(integer(0), 0, integer(1))) };
        let batch = Rc::new(Adapter::Batched(Batch {
            source: RefCell::new(Some(source)),
            size: 2,
            strict: false,
        }));
        let cursor = Object(StorageCell::new(Storage::IteratorAdapter(Rc::clone(
            &batch,
        ))));
        let mut outer = PullFrame::Empty;
        let Advance::Pull(first) = batch.advance(&mut outer, None) else {
            panic!()
        };
        let Advance::Pull(second) = batch.advance(&mut outer, Some(next_item(&first))) else {
            panic!()
        };
        // The outer next() has read 0. Reenter it before its next input returns.
        let inner_result = next_item(&cursor).ok().flatten().unwrap();
        let Advance::Return(Ok(Some(outer_result))) =
            batch.advance(&mut outer, Some(next_item(&second)))
        else {
            panic!()
        };
        for (pair, expected) in [(inner_result, [1, 2]), (outer_result, [0, 3])] {
            for (i, value) in expected.into_iter().enumerate() {
                // SAFETY: the rooted two-field tuple contains immediate integers.
                let field = unsafe {
                    records::tuples::imre_tuple_get(pair.word as *const usize, integer(i as i64))
                };
                assert_eq!(field, integer(value));
            }
        }
    }

    #[test]
    fn deep_batch_pull_discards_each_empty_tuple() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let zero = Int::small(0).unwrap().into_raw();
                // SAFETY: the tagged integers are immediate, and repeat returns one owner.
                let mut source = unsafe { Rc::from_raw(imre_repeat(2, zero, zero)) };
                for _ in 0..10_000 {
                    source = one_at_a_time(adapter(Adapter::Slice(Slice {
                        source: RefCell::new(Some(source)),
                        position: Cell::new(0),
                        next_index: Cell::new(0),
                        stop: Some(1),
                        step: 1,
                    })));
                }
                assert!(matches!(next_item(&source), Ok(None)));
                assert!(matches!(next_item(&source), Ok(None)));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn deep_batch_release_preserves_a_shared_source() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let value = Int::small(7).unwrap().into_raw();
                let once = Int::small(1).unwrap().into_raw();
                // SAFETY: both tagged integers are immediate; repeat returns one owner.
                let leaf = unsafe { Rc::from_raw(imre_repeat(2, value, once)) };
                let mut cursor = Rc::clone(&leaf);
                for _ in 0..50_000 {
                    cursor = one_at_a_time(cursor);
                }
                drop(cursor);
                assert_eq!(Rc::strong_count(&leaf), 1);
                assert!(matches!(next_item(&leaf), Ok(Some(item)) if item.word == value));
                assert!(matches!(next_item(&leaf), Ok(None)));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
