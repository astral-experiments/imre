use super::*;

pub(super) fn handle(name: &Name) -> *const () {
    match name {
        Name::Text(s) | Name::Path(s) => Rc::as_ptr(s).cast(),
        Name::Bytes(b) => Rc::as_ptr(b).cast(),
    }
}

#[derive(Clone, Copy)]
pub(super) struct Callback {
    target: *const (),
    context: *const (),
    repr: u32,
}

impl Callback {
    /// The compiler keeps the callable and any bound receiver alive during traversal.
    pub(super) fn new(target: *const (), context: *const (), repr: u32) -> Option<Self> {
        (!target.is_null()).then_some(Self {
            target,
            context,
            repr,
        })
    }

    pub(super) fn call(self, src: &Name, dst: &Name) -> TreeResult<()> {
        type Function<T> = unsafe extern "C" fn(*const (), *const (), *const ()) -> T;
        let (src, dst) = (handle(src), handle(dst));
        // SAFETY: repr matches the adapter's exact LLVM return convention. All
        // three inputs are borrowed. Only a successful result transfers an owner.
        let word = unsafe {
            match self.repr {
                0 => {
                    std::mem::transmute::<*const (), Function<u8>>(self.target)(
                        self.context,
                        src,
                        dst,
                    );
                    0
                }
                1 => {
                    std::mem::transmute::<*const (), Function<bool>>(self.target)(
                        self.context,
                        src,
                        dst,
                    );
                    0
                }
                7 => {
                    std::mem::transmute::<*const (), Function<f64>>(self.target)(
                        self.context,
                        src,
                        dst,
                    );
                    0
                }
                2 | 9 | 10 | 256.. => std::mem::transmute::<*const (), Function<usize>>(
                    self.target,
                )(self.context, src, dst),
                _ => std::mem::transmute::<*const (), Function<*const ()>>(self.target)(
                    self.context,
                    src,
                    dst,
                ) as usize,
            }
        };
        if let Some(e) = take_pending() {
            return Err(Failure::Raised(e));
        }
        if word != 0 {
            // SAFETY: copytree discards the independently owned return of copy_function.
            drop(unsafe { OwnedWord::from_owned(word, WordKind::new(self.repr)) });
        }
        Ok(())
    }
}
