//! Outer iterations select a new input only after the previous one stops.

use super::*;

enum Modes {
    Uniform(u32),
    Positional(&'static [u8], Cell<usize>),
}

pub(in crate::objects) struct Chain {
    pub(super) outer: RefCell<Option<Rc<Object>>>,
    pub(super) active: RefCell<Option<Rc<Object>>>,
    modes: Modes,
}

impl Chain {
    fn cursor(outer: Rc<Object>, modes: Modes) -> *const Object {
        Rc::into_raw(adapter(Adapter::Chain(Self {
            outer: RefCell::new(Some(outer)),
            active: RefCell::new(None),
            modes,
        })))
    }

    /// The outer cursor stops on errors; an inner cursor can continue after an error.
    pub(super) fn advance(&self, frame: &mut PullFrame, item: Option<ItemResult>) -> Advance {
        if let Some(item) = item {
            let item = match item {
                Err(error) if error.is_kind(Kind::StopIteration) => Ok(None),
                other => other,
            };
            match frame {
                PullFrame::ChainInner => match item {
                    Ok(None) => {
                        self.active.take();
                    }
                    other => return Advance::Return(other),
                },
                PullFrame::ChainOuter => {
                    let inner = match item {
                        Ok(Some(inner)) => inner,
                        ended => {
                            self.outer.take();
                            return Advance::Return(ended);
                        }
                    };
                    let mode = match &self.modes {
                        Modes::Uniform(mode) => *mode,
                        Modes::Positional(modes, index) => {
                            let i = index.get();
                            index.set(i + 1);
                            u32::from(modes[i])
                        }
                    };
                    // SAFETY: the compiler's mode describes this rooted source word.
                    match unsafe { sources::try_acquire(mode, inner.word) } {
                        Ok(active) => {
                            self.active.replace(Some(Rc::clone(&active)));
                            *frame = PullFrame::ChainInner;
                            return Advance::Pull(active);
                        }
                        Err(error) => {
                            self.outer.take();
                            return Advance::Return(Err(error));
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        let outer = self.outer.borrow();
        let Some(outer) = outer.as_ref() else {
            return Advance::Return(Ok(None));
        };
        if let Some(active) = self.active.borrow().as_ref() {
            *frame = PullFrame::ChainInner;
            Advance::Pull(Rc::clone(active))
        } else {
            *frame = PullFrame::ChainOuter;
            Advance::Pull(Rc::clone(outer))
        }
    }
}

/// # Safety
/// `tuple` is a rooted native tuple. Its fields match the `length` static mode bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chain(
    tuple: usize,
    modes: *const u8,
    length: usize,
) -> *const Object {
    // SAFETY: even an empty array passes a valid static address. The tuple source
    // preserves each field's own kind before its mode is used to acquire it.
    let (outer, modes) = unsafe {
        (
            sources::acquire(5, tuple),
            std::slice::from_raw_parts(modes, length),
        )
    };
    Chain::cursor(outer, Modes::Positional(modes, Cell::new(0)))
}

/// # Safety
/// The rooted outer value has the stated mode; each of its items has the inner mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_chain_from(outer: u32, source: usize, inner: u32) -> *const Object {
    // SAFETY: the caller roots the iterable until this acquisition completes.
    match unsafe { sources::try_acquire(outer, source) } {
        Ok(outer) => Chain::cursor(outer, Modes::Uniform(inner)),
        Err(error) => error.returning(std::ptr::null()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repeat(value: i64, times: i64) -> Rc<Object> {
        // SAFETY: small integers are immediate and the constructor returns an owner.
        unsafe {
            Rc::from_raw(imre_repeat(
                2,
                Int::small(value).unwrap().0,
                Int::small(times).unwrap().0,
            ))
        }
    }

    fn cursor_word(cursor: Rc<Object>) -> OwnedWord {
        // SAFETY: object words own exactly one Rc reference.
        unsafe { OwnedWord::from_owned(Rc::into_raw(cursor) as usize, WordKind::new(5)) }
    }

    fn positional(cursor: Rc<Object>) -> Rc<Object> {
        static MODES: [u8; 1] = [0];
        let mut args = records::tuples::TupleBuilder::new(1).ok().unwrap();
        args.push(cursor_word(cursor));
        let args = args.finish().ok().unwrap();
        // SAFETY: the single tuple field is a rooted iterator and its mode is static.
        unsafe { Rc::from_raw(imre_chain(args.word, MODES.as_ptr(), MODES.len())) }
    }

    #[test]
    fn a_reentrant_outer_pull_installs_its_own_inner_cursor() {
        let outer = repeat(0, 0);
        let first = repeat(11, 1);
        let second = repeat(22, 1);
        let chain = Chain {
            outer: RefCell::new(Some(outer)),
            active: RefCell::new(None),
            modes: Modes::Uniform(0),
        };
        let mut suspended = PullFrame::Empty;
        let mut reentrant = PullFrame::Empty;
        assert!(matches!(
            chain.advance(&mut suspended, None),
            Advance::Pull(_)
        ));
        assert!(matches!(
            chain.advance(&mut reentrant, None),
            Advance::Pull(_)
        ));
        for (frame, source, expected) in [(&mut reentrant, second, 22), (&mut suspended, first, 11)]
        {
            let Advance::Pull(active) =
                chain.advance(frame, Some(Ok(Some(cursor_word(Rc::clone(&source))))))
            else {
                panic!("expected an inner cursor");
            };
            assert!(Rc::ptr_eq(&active, &source));
            assert!(
                matches!(chain.advance(frame, Some(next_item(&active))), Advance::Return(Ok(Some(value))) if value.word == Int::small(expected).unwrap().0)
            );
        }
    }

    #[test]
    fn deep_chains_release_both_owners_and_preserve_a_shared_leaf() {
        std::thread::Builder::new().stack_size(256 * 1024).spawn(|| {
            let leaf = repeat(11, 2);
            for started in [false, true] {
                let mut cursor = Rc::clone(&leaf);
                for _ in 0..16_384 { cursor = positional(cursor); }
                if started {
                    assert!(matches!(next_item(&cursor), Ok(Some(item)) if item.word == Int::small(11).unwrap().0));
                }
                drop(cursor);
                assert_eq!(Rc::strong_count(&leaf), 1);
            }
            assert!(matches!(next_item(&leaf), Ok(Some(item)) if item.word == Int::small(11).unwrap().0));
            assert!(matches!(next_item(&leaf), Ok(None)));
            let mut outer = repeat(0, 0);
            for _ in 0..16_384 {
                // SAFETY: mode zero denotes the live outer cursor.
                outer = unsafe { Rc::from_raw(imre_chain_from(0, Rc::as_ptr(&outer) as usize, 0)) };
            }
            assert!(matches!(next_item(&outer), Ok(None)));
            assert!(matches!(next_item(&outer), Ok(None)));
        }).unwrap().join().unwrap();
    }
}
