//! Iterators keep their input owners even when other consumers share the cursor.

use super::*;
use words::WordKind;

pub(super) struct Repeat {
    pub(super) value: OwnedWord,
    pub(super) remaining: Option<usize>,
}

impl Repeat {
    pub(super) fn next(&mut self) -> Option<OwnedWord> {
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
                Adapter::Progression(progression) => progression.remaining(),
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
    let remaining = if times == 0 {
        None
    } else {
        // SAFETY: the caller roots the count throughout conversion.
        let times = unsafe { Int::borrow(times) };
        let times = match ssize(&times) {
            Ok(times) => times,
            Err(error) => return error.returning(std::ptr::null()),
        };
        Some(times.max(0) as usize)
    };
    // SAFETY: the kind describes the caller's rooted argument.
    let value = unsafe { WordKind::new(kind).acquire(value) };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::Repeat(Repeat {
        value,
        remaining,
    })))))
}

mod sources;
use std::cell::{Cell, RefCell};

pub(super) enum Adapter {
    Sequence(sources::Sequence),
    Progression(sources::Progression),
    FloatCount { current: Cell<f64>, step: OwnedWord },
    Slice(Slice),
    Cycle(Cycle),
}

fn adapter(state: Adapter) -> Rc<Object> {
    Rc::new(Object(StorageCell::new(Storage::IteratorAdapter(Rc::new(
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

impl Drop for Adapter {
    fn drop(&mut self) {
        let mut source = self.take_source();
        while let Some(cursor) = source {
            let Ok(cursor) = Rc::try_unwrap(cursor) else {
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
    fn take_source(&mut self) -> Option<Rc<Object>> {
        match self {
            Self::Slice(slice) => slice.source.get_mut().take(),
            Self::Cycle(cycle) => cycle.source.get_mut().take(),
            _ => None,
        }
    }

    pub(super) fn write_repr(&self, output: &mut String, owner: *const Object) {
        use std::fmt::Write;
        let name = match self {
            Self::Progression(count) if count.stop.is_none() => {
                let current = count.current.borrow();
                write!(output, "count({}", current.big()).unwrap();
                if count.step.as_small() != Some(1) {
                    write!(output, ", {}", count.step.big()).unwrap();
                }
                output.push(')');
                return;
            }
            Self::FloatCount { current, step } => {
                write!(output, "count({}", crate::floats::render(current.get())).unwrap();
                if step.kind.raw() != 2 || step.word != Int::small(1).unwrap().0 {
                    output.push_str(", ");
                    // SAFETY: step owns its exact numeric representation.
                    unsafe { values::write_repr(output, step.word, step.kind) };
                }
                output.push(')');
                return;
            }
            Self::Progression(_) => "range_iterator",
            Self::Sequence(_) => "iterator",
            Self::Slice(_) => "itertools.islice",
            Self::Cycle(_) => "itertools.cycle",
        };
        write!(output, "<{name} object at {owner:p}>").unwrap();
    }

    pub(super) fn next(&self) -> Result<Option<OwnedWord>, Error> {
        match self {
            Self::Sequence(seq) => Ok(seq.next()),
            Self::Progression(count) => Ok(count.next()),
            Self::FloatCount { current, step } => {
                // SAFETY: a numeric step stays rooted; binary64 words carry no owner.
                unsafe {
                    let delta = if step.kind.raw() == 2 {
                        crate::floats::int_to_float(&Int::borrow(step.word))?
                    } else {
                        f64::from_bits(step.word as u64)
                    };
                    let old = current.replace(current.get() + delta);
                    Ok(Some(OwnedWord::from_owned(
                        old.to_bits() as usize,
                        WordKind::new(7),
                    )))
                }
            }
            Self::Slice(slice) => slice.next(),
            Self::Cycle(cycle) => cycle.next(),
        }
    }
}

pub(super) struct Slice {
    source: RefCell<Option<Rc<Object>>>,
    position: Cell<usize>,
    next_index: Cell<usize>,
    stop: Option<usize>,
    step: usize,
}

impl Slice {
    fn next(&self) -> Result<Option<OwnedWord>, Error> {
        let Some(source) = self.source.borrow().clone() else {
            return Ok(None);
        };
        let next = (|| {
            while self.position.get() < self.next_index.get() {
                if next_item(&source)?.is_none() {
                    return Ok(None);
                }
                self.position.set(self.position.get() + 1);
            }
            if self.stop.is_some_and(|stop| self.position.get() >= stop) {
                return Ok(None);
            }
            let Some(item) = next_item(&source)? else {
                return Ok(None);
            };
            self.position.set(self.position.get() + 1);
            let next = self.next_index.get().saturating_add(self.step);
            self.next_index
                .set(self.stop.map_or(next, |stop| next.min(stop)));
            Ok(Some(item))
        })();
        if !matches!(next, Ok(Some(_))) {
            self.source.take();
        }
        next
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
    Rc::into_raw(adapter(Adapter::Slice(Slice {
        source: RefCell::new(Some(source)),
        position: Cell::new(0),
        next_index: Cell::new(start as usize),
        stop: stop.map(|n| n as usize),
        step: step as usize,
    })))
}

pub(super) struct Cycle {
    source: RefCell<Option<Rc<Object>>>,
    saved: RefCell<Vec<OwnedWord>>,
    index: Cell<usize>,
}

impl Cycle {
    fn next(&self) -> Result<Option<OwnedWord>, Error> {
        let source = self.source.borrow().clone();
        if let Some(source) = source {
            if let Some(item) = next_item(&source)? {
                let mut saved = self.saved.borrow_mut();
                saved.try_reserve(1).map_err(|_| Error::allocation())?;
                saved.push(item.clone());
                return Ok(Some(item));
            }
            self.source.take();
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
        Ok(item)
    }
}

/// # Safety
/// The caller roots a source of the selected concrete mode.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_cycle(mode: u32, source: usize) -> *const Object {
    // SAFETY: the caller provides the exact representation until acquisition completes.
    let source = unsafe { sources::acquire(mode, source) };
    Rc::into_raw(adapter(Adapter::Cycle(Cycle {
        source: RefCell::new(Some(source)),
        saved: RefCell::new(Vec::new()),
        index: Cell::new(0),
    })))
}

/// # Safety
/// The step is a rooted exact int (kind 2) or an unboxed float (kind 7).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_float_count(start: f64, kind: u32, step: usize) -> *const Object {
    // SAFETY: conversion of an integer step happens at pull time, while its owner is live.
    let step = unsafe { WordKind::new(kind).acquire(step) };
    Rc::into_raw(adapter(Adapter::FloatCount {
        current: Cell::new(start),
        step,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut cursor = Rc::clone(&shared);
        for _ in 0..65_536 {
            cursor = cycle(Some(cursor));
        }
        drop(cursor);
        assert_eq!(Rc::strong_count(&shared), 1);
    }
}
