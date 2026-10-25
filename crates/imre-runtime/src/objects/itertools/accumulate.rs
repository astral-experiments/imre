//! A running total is published only after the current addition or callback succeeds.
use super::*;

pub(in crate::objects) struct Accumulate {
    pub(super) source: Option<Rc<Object>>,
    initial: RefCell<Option<OwnedWord>>,
    total: RefCell<Option<OwnedWord>>,
    function: OwnedWord,
    abi: u32,
    item_kind: WordKind,
    transfer: bool,
}

impl Accumulate {
    pub(super) fn advance(&self, input: Option<ItemResult>) -> Advance {
        let Some(input) = input else {
            if let Some(initial) = self.initial.take() {
                self.total.replace(Some(initial.clone()));
                return Advance::Return(Ok(Some(initial)));
            }
            return Advance::Pull(Rc::clone(self.source.as_ref().unwrap()));
        };
        let item = match input {
            Ok(Some(item)) => item,
            ended => return Advance::Return(ended),
        };
        // The source can reenter before supplying this item. The callback can reenter
        // again, so keep its left operand rooted without borrowing the live total.
        let previous = self.total.borrow().clone();
        let next = match previous {
            None => item,
            Some(previous) => match self.combine(&previous, &item) {
                Ok(value) => value,
                Err(error) => return Advance::Return(Err(error)),
            },
        };
        self.total.replace(Some(next.clone()));
        Advance::Return(Ok(Some(next)))
    }

    fn combine(&self, left: &OwnedWord, right: &OwnedWord) -> Result<OwnedWord, Error> {
        if self.abi == 0 {
            let name = match self.function.kind.raw() {
                1 => "bool",
                2 => "int",
                3 => "str",
                7 => "float",
                8 => "bytes",
                _ => unreachable!("checked noncallable"),
            };
            return Err(Error::new(
                Kind::TypeError,
                format!("'{name}' object is not callable"),
            ));
        }
        let parameter = |arg: &OwnedWord| {
            if self.transfer {
                arg.clone().into_word()
            } else {
                arg.word
            }
        };
        let (a, b) = (parameter(left), parameter(right));
        // SAFETY: the compiler passes an existing exact (T, T) -> T callable record.
        // Its first field is the adapter, its second the bound context. The ABI tag
        // matches the LLVM type of T; owning parameters receive separate references.
        let result = unsafe {
            let record = self.function.word as *const *const ();
            let target = *record;
            let context = *record.add(1);
            match self.abi {
                1 => std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(*const (), usize, usize) -> usize,
                >(target)(context, a, b),
                2 => std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(*const (), *const (), *const ()) -> *const (),
                >(target)(context, a as *const (), b as *const ()) as usize,
                3 => std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(*const (), f64, f64) -> f64,
                >(target)(
                    context, f64::from_bits(a as u64), f64::from_bits(b as u64)
                )
                .to_bits() as usize,
                4 => usize::from(std::mem::transmute::<
                    *const (),
                    unsafe extern "C" fn(*const (), bool, bool) -> bool,
                >(target)(context, a != 0, b != 0)),
                _ => unreachable!("checked binary callable ABI"),
            }
        };
        if let Some(error) = Error::take_raised() {
            return Err(error);
        }
        // SAFETY: a successful callable returned one owned word of the declared type.
        Ok(unsafe { OwnedWord::from_owned(result, self.item_kind) })
    }
}

/// # Safety
/// The compiler supplies rooted source, function and initial value words, their exact
/// representations, and the homogeneous callable ABI (zero for a noncallable argument).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_accumulate(
    mode: u32,
    source: usize,
    function_kind: u32,
    function: usize,
    initial_kind: u32,
    initial: usize,
    item_kind: u32,
    abi: u32,
    transfer: u32,
) -> *const Object {
    // SAFETY: the ordered call binds all arguments before acquiring its iterator.
    let source = match unsafe { sources::try_acquire(mode, source) } {
        Ok(source) => source,
        Err(error) => return error.returning(std::ptr::null()),
    };
    // SAFETY: each operand is rooted independently; None omits the seed.
    unsafe {
        Rc::into_raw(adapter(Adapter::Accumulate(Accumulate {
            source: Some(source),
            initial: RefCell::new(
                (initial_kind != 0).then(|| WordKind::new(initial_kind).acquire(initial)),
            ),
            total: RefCell::new(None),
            function: WordKind::new(function_kind).acquire(function),
            abi,
            item_kind: WordKind::new(item_kind),
            transfer: transfer != 0,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Context {
        cursor: Cell<*const Object>,
        nested: Cell<i64>,
        failure: Option<Rc<Object>>,
    }

    fn integer(value: i64) -> usize {
        Int::small(value).unwrap().into_raw()
    }
    fn value(item: ItemResult) -> i64 {
        let Ok(Some(item)) = item else {
            panic!("expected an integer")
        };
        // SAFETY: every test cursor yields a canonical tagged integer.
        unsafe { Int::borrow(item.word).as_small().unwrap() }
    }

    unsafe extern "C" fn reenter(context: *const (), a: usize, b: usize) -> usize {
        // SAFETY: the test roots this context, both input words, and the cursor for this call.
        unsafe {
            let ctx = &*context.cast::<Context>();
            let (a, b) = (
                Int::borrow(a).as_small().unwrap(),
                Int::borrow(b).as_small().unwrap(),
            );
            if b == 2 {
                ctx.nested.set(value(next_item(&*ctx.cursor.get())));
                if let Some(error) = &ctx.failure {
                    exceptions::imre_exception_raise(Rc::as_ptr(error));
                    return 0;
                }
            }
            integer(10 * a + b)
        }
    }

    #[test]
    fn nested_total_survives_a_raising_outer_callback() {
        let kinds = [0, 0];
        let layout = records::RecordDescriptor {
            length: 2,
            kinds: kinds.as_ptr(),
            semantics: std::ptr::null(),
        };
        // SAFETY: the descriptor and each stack context outlive their records. The record
        // has the exact compiled callable layout and both inputs are borrowed integers.
        unsafe {
            for fails in [false, true] {
                let context = Context {
                    cursor: Cell::new(std::ptr::null()),
                    nested: Cell::new(0),
                    failure: fails.then(|| {
                        Rc::from_raw(exceptions::imre_exception_new(
                            Kind::ValueError as u32,
                            std::ptr::null(),
                        ))
                    }),
                };
                let record = records::imre_record_new(&layout);
                record.cast::<*const ()>().write(reenter as *const ());
                record
                    .add(1)
                    .cast::<*const ()>()
                    .write((&raw const context).cast());
                let callback = OwnedWord::from_owned(record as usize, WordKind::new(6));
                let source = Rc::from_raw(sources::imre_progression(
                    integer(1),
                    integer(5),
                    integer(1),
                ));
                let cursor = Rc::from_raw(imre_accumulate(
                    0,
                    Rc::as_ptr(&source) as usize,
                    6,
                    callback.word,
                    0,
                    0,
                    2,
                    1,
                    0,
                ));
                context.cursor.set(Rc::as_ptr(&cursor));
                assert_eq!(value(next_item(&cursor)), 1);
                let outer = next_item(&cursor);
                assert_eq!(context.nested.get(), 13);
                if let Some(original) = &context.failure {
                    let Err(error) = outer else {
                        panic!("expected callback exception")
                    };
                    error.raise();
                    let raised = exceptions::take_pending().unwrap();
                    assert!(Rc::ptr_eq(original, &raised));
                    assert_eq!(value(next_item(&cursor)), 134);
                } else {
                    assert_eq!(value(outer), 12);
                    assert_eq!(value(next_item(&cursor)), 124);
                }
                assert!(matches!(next_item(&cursor), Ok(None)));
            }
        }
    }
}
