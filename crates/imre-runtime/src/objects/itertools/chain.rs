//! Outer iterations select a new input only after the previous one stops.

use super::*;

enum Modes {
    Uniform(u32),
    Positional(&'static [u8], Cell<usize>),
}

pub(in crate::objects) struct Chain {
    outer: RefCell<Option<Rc<Object>>>,
    active: RefCell<Option<Rc<Object>>>,
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

    pub(super) fn next(&self) -> Result<Option<OwnedWord>, Error> {
        loop {
            // Do not hold storage borrows while polling another iterator.
            let active = self.active.borrow().clone();
            if let Some(active) = active {
                if let Some(item) = next_item(&active)? {
                    return Ok(Some(item));
                }
                self.active.take();
            }
            let Some(outer) = self.outer.borrow().clone() else {
                return Ok(None);
            };
            let inner = match next_item(&outer) {
                Ok(Some(inner)) => inner,
                ended => {
                    self.outer.take();
                    return ended;
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
            let invalid = match mode {
                8 => Some("NoneType"),
                9 => Some("int"),
                10 => Some("bool"),
                11 => Some("float"),
                _ => None,
            };
            if let Some(name) = invalid {
                self.outer.take();
                return Err(Error::new(
                    Kind::TypeError,
                    format!("'{name}' object is not iterable"),
                ));
            }
            // SAFETY: the compiler's mode describes this rooted source word.
            self.active
                .replace(Some(unsafe { sources::acquire(mode, inner.word) }));
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
    Chain::cursor(
        unsafe { sources::acquire(outer, source) },
        Modes::Uniform(inner),
    )
}
