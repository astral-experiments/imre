//! Predicate cursors pull the ordinary source before deciding whether to return its item.
use super::*;

enum Mode {
    FilterFalse,
    DropWhile,
    TakeWhile,
}

type Test = unsafe extern "C" fn(usize, usize) -> u32;

pub(in crate::objects) struct Predicate {
    pub(super) source: Option<Rc<Object>>,
    function: OwnedWord,
    test: Option<Test>,
    error: &'static str,
    transfer: u32,
    mode: Mode,
    transitioned: Cell<bool>,
}

impl Predicate {
    pub(super) fn name(&self) -> &'static str {
        match self.mode {
            Mode::FilterFalse => "itertools.filterfalse",
            Mode::DropWhile => "itertools.dropwhile",
            Mode::TakeWhile => "itertools.takewhile",
        }
    }

    pub(super) fn advance(&self, input: Option<ItemResult>) -> Advance {
        let pull = || Advance::Pull(Rc::clone(self.source.as_ref().unwrap()));
        let Some(input) = input else {
            return if matches!(self.mode, Mode::TakeWhile) && self.transitioned.get() {
                Advance::Return(Ok(None))
            } else {
                pull()
            };
        };
        let item = match input {
            Ok(Some(item)) => item,
            ended => return Advance::Return(ended),
        };
        // A source can reenter dropwhile before it supplies the outer item.
        if matches!(self.mode, Mode::DropWhile) && self.transitioned.get() {
            return Advance::Return(Ok(Some(item)));
        }
        let Some(test) = self.test else {
            return Advance::Return(Err(Error::new(Kind::TypeError, self.error)));
        };
        let argument = |value: &OwnedWord, bit| {
            if self.transfer & bit != 0 {
                value.clone().into_word()
            } else {
                value.word
            }
        };
        // SAFETY: the compiler's bridge accepts two words and performs both the
        // Python call and its truth test. Resource parameters receive independent owners.
        let truth = unsafe { test(argument(&self.function, 1), argument(&item, 2)) } != 0;
        if let Some(error) = Error::take_raised() {
            return Advance::Return(Err(error));
        }
        match (&self.mode, truth) {
            (Mode::TakeWhile, false) => {
                self.transitioned.set(true);
                Advance::Return(Ok(None))
            }
            (Mode::DropWhile, false) => {
                self.transitioned.set(true);
                Advance::Return(Ok(Some(item)))
            }
            (Mode::TakeWhile, true) | (Mode::FilterFalse, false) => Advance::Return(Ok(Some(item))),
            _ => pull(),
        }
    }
}

/// # Safety
/// The caller roots both Python arguments, describes their exact representations
/// and supplies either a native predicate bridge or a static UTF-8 TypeError message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_predicate_cursor(
    mode: u32,
    source_mode: u32,
    source: usize,
    function_kind: u32,
    function: usize,
    test: Option<Test>,
    error: *const u8,
    length: usize,
    transfer: u32,
) -> *const Object {
    // SAFETY: both arguments have been evaluated before the source iterator is acquired.
    let source = match unsafe { sources::try_acquire(source_mode, source) } {
        Ok(source) => source,
        Err(error) => return error.returning(std::ptr::null()),
    };
    let error = if length == 0 {
        ""
    } else {
        // SAFETY: emitted error strings are immutable globals in the executable.
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(error, length)) }
    };
    Rc::into_raw(adapter(Adapter::Predicate(Predicate {
        source: Some(source),
        // SAFETY: the compiler supplies the exact kind of the rooted function argument.
        function: unsafe { WordKind::new(function_kind).acquire(function) },
        test,
        error,
        transfer,
        mode: match mode {
            0 => Mode::FilterFalse,
            1 => Mode::DropWhile,
            2 => Mode::TakeWhile,
            _ => unreachable!("checked predicate mode"),
        },
        transitioned: Cell::new(false),
    })))
}
