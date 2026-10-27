//! Iterators keep their input owners even when other consumers share the cursor.

use super::*;
use words::WordKind;

/// A finite or unbounded cursor that retains the same element across pulls.
#[derive(Clone)]
pub(crate) struct Repeat<T> {
    pub(crate) value: T,
    pub(crate) remaining: Option<usize>,
}

impl<T: Clone> Repeat<T> {
    pub(crate) fn new(value: T, times: Option<&Int>) -> crate::Result<Self> {
        let remaining = times.map(ssize).transpose()?.map(|n| n.max(0) as usize);
        Ok(Self { value, remaining })
    }

    pub(crate) fn next(&mut self) -> Option<T> {
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.checked_sub(1)?;
        }
        Some(self.value.clone())
    }
}

fn ssize(value: &Int) -> Result<i64, Error> {
    value
        .as_small()
        .or_else(|| value.big().to_i64())
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })
}

/// # Safety
/// The caller roots the iterator and the canonical integer default.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cursor_length_hint(cursor: *const Object, default: usize) -> usize {
    // SAFETY: both arguments remain independently rooted during this read.
    unsafe {
        let default = Int::borrow(default);
        if let Err(error) = ssize(&default) {
            return error.returning(0);
        }
        let remaining = object(cursor).0.with(|storage| match storage {
            Storage::Repeat(repeat) => repeat.remaining.map(Int::from_usize),
            Storage::IteratorAdapter(state) => match state.as_ref() {
                Adapter::Progression(progression) => Some(progression.remaining()),
                _ => None,
            },
            _ => None,
        });
        if let Some(remaining) = remaining {
            if let Err(error) = ssize(&remaining) {
                return error.returning(0);
            }
            remaining.into_raw()
        } else {
            (*default).clone().into_raw()
        }
    }
}

/// # Safety
/// The caller roots a value of the exact supplied kind. A nonzero `times`
/// is a rooted canonical integer; zero means that the argument was omitted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_repeat(kind: u32, value: usize, times: usize) -> *const Object {
    // SAFETY: a nonzero count is rooted by the caller throughout conversion.
    let times = (times != 0).then(|| unsafe { Int::borrow(times) });
    // SAFETY: the kind describes the caller's rooted argument.
    let value = unsafe { WordKind::new(kind).acquire(value) };
    match Repeat::new(value, times.as_deref()) {
        Ok(repeat) => crate::ObjectOwner::into_raw(crate::ObjectOwner::new(Object(
            StorageCell::new(Storage::Repeat(repeat)),
        ))),
        Err(error) => error.returning(std::ptr::null()),
    }
}

mod count;
pub(crate) use count::CountValue;
mod counter_elements;
mod sources;
mod tee;
use std::cell::{Cell, RefCell};

pub(super) enum Adapter {
    CounterElements(RefCell<counter_elements::Elements>),
    Sequence(sources::Sequence),
    Progression(sources::Progression),
    Count {
        current: RefCell<CountValue>,
        step: CountValue,
    },
    Slice(Slice),
    Tee(tee::Fork),
    Cycle(Cycle),
    PathWalk(paths::Walker),
    Format(format_cursors::Cursor),
}

pub(super) fn adapter(state: Adapter) -> crate::ObjectOwner {
    crate::ObjectOwner::new(Object(StorageCell::new(Storage::IteratorAdapter(Rc::new(
        state,
    )))))
}

fn next_item(source: &Object) -> Result<Option<OwnedWord>, Error> {
    mappings::pull(source).map(|next| {
        next.map(|(item, value)| {
            debug_assert!(value.is_none());
            item
        })
    })
}

type ItemResult = Result<Option<OwnedWord>, Error>;

enum Advance {
    Pull(crate::ObjectOwner),
    Return(ItemResult),
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let mut source = self.take_source();
        while let Some(cursor) = source {
            let Ok(cursor) = crate::ObjectOwner::try_unwrap(cursor) else {
                break;
            };
            let Storage::IteratorAdapter(state) = cursor.0.into_inner() else {
                break;
            };
            let Ok(mut state) = Rc::try_unwrap(state) else {
                break;
            };
            // Detach the next owner before this adapter's fields are destroyed.
            source = state.take_source();
        }
    }
}

impl Adapter {
    fn take_source(&mut self) -> Option<crate::ObjectOwner> {
        match self {
            Self::Tee(fork) => fork.take_source(),
            Self::Slice(slice) => slice.source.get_mut().take(),
            Self::Cycle(cycle) => cycle.source.get_mut().take(),
            _ => None,
        }
    }

    pub(super) fn write_repr(
        &self,
        output: &mut super::values::ReprWriter<'_>,
        owner: *const Object,
    ) -> std::fmt::Result {
        use std::fmt::Write;
        let name = match self {
            Self::Count { current, step } => {
                write!(output, "count({}", current.borrow())?;
                if !matches!(step, CountValue::Int(n) if n.as_small() == Some(1)) {
                    write!(output, ", {step}")?;
                }
                output.write_char(')')?;
                return Ok(());
            }
            Self::Progression(_) => "range_iterator",
            Self::CounterElements(_) => "itertools.chain",
            Self::Sequence(_) => "iterator",
            Self::Tee(_) => "itertools._tee",
            Self::Slice(_) => "itertools.islice",
            Self::Cycle(_) => "itertools.cycle",
            Self::Format(cursor) => cursor.name(),
            Self::PathWalk(_) => {
                write!(output, "<generator object Path.walk at {owner:p}>")?;
                return Ok(());
            }
        };
        write!(output, "<{name} object at {owner:p}>")
    }

    fn advance(&self, item: Option<ItemResult>) -> Advance {
        match self {
            Self::Tee(fork) => fork.advance(item),
            Self::Slice(slice) => slice.advance(item),
            Self::Cycle(cycle) => cycle.advance(item),
            _ => unreachable!("only composed cursors request source items"),
        }
    }

    fn next_composed(&self) -> Result<Option<OwnedWord>, Error> {
        let mut nested = smallvec::SmallVec::<[Rc<Self>; 8]>::new();
        let mut step = self.advance(None);
        loop {
            let item = match step {
                Advance::Return(result) => {
                    if nested.pop().is_none() {
                        return result;
                    }
                    result
                }
                Advance::Pull(source) => {
                    // SAFETY: only the adapter owner escapes the storage borrow.
                    let composed = unsafe {
                        source.0.with(|storage| match storage {
                            Storage::IteratorAdapter(state)
                                if matches!(
                                    **state,
                                    Self::Tee(_) | Self::Slice(_) | Self::Cycle(_)
                                ) =>
                            {
                                Some(Rc::clone(state))
                            }
                            _ => None,
                        })
                    };
                    if let Some(state) = composed {
                        if nested.try_reserve(1).is_ok() {
                            step = state.advance(None);
                            nested.push(state);
                            continue;
                        }
                        Err(Error::allocation())
                    } else {
                        next_item(&source)
                    }
                }
            };
            step = nested.last().map_or(self, Rc::as_ref).advance(Some(item));
        }
    }

    pub(super) fn next(&self) -> Result<Option<OwnedWord>, Error> {
        match self {
            Self::CounterElements(elements) => elements.borrow_mut().next(),
            Self::Sequence(seq) => seq.next(),
            Self::Progression(count) => Ok(count.next()),
            Self::Count { current, step } => {
                let mut value = current.borrow_mut();
                let next = value.successor(step)?;
                Ok(Some(std::mem::replace(&mut *value, next).into_word()))
            }
            Self::Tee(_) | Self::Slice(_) | Self::Cycle(_) => self.next_composed(),
            Self::PathWalk(walk) => walk.next(),
            Self::Format(cursor) => cursor.next(),
        }
    }
}

pub(super) struct Slice {
    source: RefCell<Option<crate::ObjectOwner>>,
    position: Cell<usize>,
    next_index: Cell<usize>,
    stop: Option<usize>,
    step: usize,
}

impl Slice {
    fn advance(&self, item: Option<ItemResult>) -> Advance {
        if let Some(item) = item {
            let Ok(Some(item)) = item else {
                self.source.take();
                return Advance::Return(item);
            };
            let position = self.position.get();
            self.position.set(position + 1);
            if position >= self.next_index.get() {
                let next = self.next_index.get().saturating_add(self.step);
                self.next_index
                    .set(self.stop.map_or(next, |stop| next.min(stop)));
                return Advance::Return(Ok(Some(item)));
            }
        }
        let position = self.position.get();
        if position >= self.next_index.get() && self.stop.is_some_and(|stop| position >= stop) {
            self.source.take();
        }
        match self.source.borrow().as_ref() {
            Some(source) => Advance::Pull(crate::ObjectOwner::clone(source)),
            None => Advance::Return(Ok(None)),
        }
    }
}

/// # Safety
/// The mode describes the rooted source. Each nonzero bound is a rooted
/// integer; zero represents an omitted bound or None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_islice(
    mode: u32,
    source: usize,
    start: usize,
    stop: usize,
    step: usize,
) -> *const Object {
    let bound = |word: usize| {
        (word != 0).then(|| {
            // SAFETY: nonzero bounds are rooted canonical integers.
            let n = unsafe { Int::borrow(word) };
            n.as_small().or_else(|| n.big().to_i64()).unwrap_or(-1)
        })
    };
    let start = bound(start).unwrap_or(0);
    let stop = bound(stop);
    let step = bound(step).unwrap_or(1);
    let message = if stop == Some(-1) {
        Some("Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize.")
    } else if start < 0 || stop.is_some_and(|n| n < 0) {
        Some("Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.")
    } else if step < 1 {
        Some("Step for islice() must be a positive integer or None.")
    } else {
        None
    };
    if let Some(message) = message {
        return Error::new(Kind::ValueError, message).returning(std::ptr::null());
    }
    // SAFETY: the caller roots the exact source through validation and acquisition.
    let source = unsafe { sources::acquire(mode, source) };
    crate::ObjectOwner::into_raw(adapter(Adapter::Slice(Slice {
        source: RefCell::new(Some(source)),
        position: Cell::new(0),
        next_index: Cell::new(start as usize),
        stop: stop.map(|n| n as usize),
        step: step as usize,
    })))
}

pub(super) struct Cycle {
    source: RefCell<Option<crate::ObjectOwner>>,
    saved: RefCell<Vec<OwnedWord>>,
    index: Cell<usize>,
}

impl Cycle {
    fn advance(&self, item: Option<ItemResult>) -> Advance {
        match item {
            Some(Ok(Some(item))) => {
                let mut saved = self.saved.borrow_mut();
                if saved.try_reserve(1).is_err() {
                    return Advance::Return(Err(Error::allocation()));
                }
                saved.push(item.clone());
                return Advance::Return(Ok(Some(item)));
            }
            Some(Ok(None)) => {
                self.source.take();
            }
            Some(Err(error)) => return Advance::Return(Err(error)),
            None => {}
        }
        if let Some(source) = self.source.borrow().as_ref() {
            return Advance::Pull(crate::ObjectOwner::clone(source));
        }
        let saved = self.saved.borrow();
        let index = self.index.get();
        let item = saved.get(index).cloned();
        if item.is_some() {
            self.index.set(if index + 1 == saved.len() {
                0
            } else {
                index + 1
            });
        }
        Advance::Return(Ok(item))
    }
}

/// # Safety
/// The caller roots a source of the selected concrete mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cycle(mode: u32, source: usize) -> *const Object {
    // SAFETY: the caller provides the exact representation until acquisition completes.
    let source = unsafe { sources::acquire(mode, source) };
    crate::ObjectOwner::into_raw(adapter(Adapter::Cycle(Cycle {
        source: RefCell::new(Some(source)),
        saved: RefCell::new(Vec::new()),
        index: Cell::new(0),
    })))
}

/// # Safety
/// The step is a rooted exact int (kind 2) or an unboxed float (kind 7).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_float_count(start: f64, kind: u32, step: usize) -> *const Object {
    let step = if kind == 2 {
        // SAFETY: the caller roots its exact integer until acquisition completes.
        CountValue::Int(unsafe { (*Int::borrow(step)).clone() })
    } else {
        CountValue::Float(f64::from_bits(step as u64))
    };
    crate::ObjectOwner::into_raw(adapter(Adapter::Count {
        current: RefCell::new(CountValue::Float(start)),
        step,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advancing_deep_adapters_returns_the_leaf_item() {
        // SAFETY: both immediate integers are rooted canonical integer words.
        let leaf = unsafe { imre_repeat(2, Int::small(11).unwrap().0, Int::small(1).unwrap().0) };
        // SAFETY: the constructor transferred its owner.
        let mut cursor = unsafe { crate::ObjectOwner::from_raw(leaf) };
        for _ in 0..32_768 {
            // SAFETY: mode zero acquires the live iterator cursor.
            let next = unsafe { imre_cycle(0, crate::ObjectOwner::as_ptr(&cursor) as usize) };
            // SAFETY: the constructor transferred a new owner.
            cursor = unsafe { crate::ObjectOwner::from_raw(next) };
        }
        let Ok(Some((item, None))) = mappings::pull(&cursor) else {
            panic!("expected the repeated integer");
        };
        assert_eq!(item.word, Int::small(11).unwrap().0);
    }

    #[test]
    fn releasing_deep_adapters_stops_at_a_shared_cursor() {
        let cycle = |source| {
            adapter(Adapter::Cycle(Cycle {
                source: RefCell::new(source),
                saved: RefCell::default(),
                index: Cell::new(0),
            }))
        };
        let shared = cycle(None);
        let mut cursor = crate::ObjectOwner::clone(&shared);
        for _ in 0..65_536 {
            cursor = cycle(Some(cursor));
        }
        drop(cursor);
        assert_eq!(crate::ObjectOwner::strong_count(&shared), 1);
    }
}
