use super::*;
use crate::StrOwner;

/// Report a noncallable copy_function only when a leaf actually needs copying.
///
/// # Safety
/// The native caller keeps the message string rooted through invocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_noncallable(
    message: *const Str,
    _: *const (),
    _: *const (),
) -> u8 {
    // SAFETY: the frontend supplied the exact diagnostic as the callback context.
    Error::new(Kind::TypeError, unsafe { &*message }.clone()).returning(0)
}

pub(super) fn handle(name: &Name) -> *const () {
    match name {
        Name::Text(s) | Name::Path(s) => StrOwner::as_ptr(s).cast(),
        Name::Bytes(b) => crate::BytesOwner::as_ptr(b).cast(),
    }
}

#[derive(Clone, Copy)]
pub(in super::super) struct Callback {
    target: *const (),
    context: *const (),
    repr: u32,
    tagged: u32,
}

impl Callback {
    /// The compiler keeps the callable and any bound receiver alive during traversal.
    pub(in super::super) fn new(target: *const (), context: *const (), repr: u32) -> Option<Self> {
        (!target.is_null()).then_some(Self {
            target,
            context,
            repr,
            tagged: 0,
        })
    }

    pub(in super::super) fn tagged(mut self, mask: u32) -> Self {
        self.tagged = mask;
        self
    }

    pub(in super::super) fn call(self, src: &Name, dst: &Name) -> TreeResult<()> {
        self.call_paths(src, dst, None, None)
    }

    /// Move may pass an original pathlib object; a joined child is always a string or bytes.
    pub(in super::super) fn call_paths(
        self,
        src: &Name,
        dst: &Name,
        src_object: Option<*const ()>,
        dst_object: Option<*const ()>,
    ) -> TreeResult<()> {
        let path = |name: &Name, original: Option<*const ()>| match (name, original) {
            (Name::Path(_), Some(object)) => (object, 6usize),
            (Name::Bytes(_), _) => (handle(name), 1),
            _ => (handle(name), 0),
        };
        let (src, s_tag) = path(src, src_object);
        let (dst, d_tag) = path(dst, dst_object);
        // SAFETY: the compiler set these bits from the callable's exact parameter types.
        let word = unsafe {
            match self.tagged {
                0 => self.invoke(src, dst),
                1 => self.invoke(src as usize | s_tag, dst),
                2 => self.invoke(src, dst as usize | d_tag),
                3 => self.invoke(src as usize | s_tag, dst as usize | d_tag),
                _ => unreachable!(),
            }
        };
        if let Some(e) = take_pending() {
            return Err(Failure::Raised(e));
        }
        if word != 0 {
            // SAFETY: copytree and move discard the independently owned return of copy_function.
            drop(unsafe { OwnedWord::from_owned(word, WordKind::new(self.repr)) });
        }
        Ok(())
    }

    unsafe fn invoke<S: Copy, D: Copy>(self, src: S, dst: D) -> usize {
        type Function<S, D, T> = unsafe extern "C" fn(*const (), S, D) -> T;
        // SAFETY: S, D and repr match the exact LLVM calling convention. The
        // three inputs are borrowed. Only a successful result transfers an owner.
        unsafe {
            match self.repr {
                0 => {
                    std::mem::transmute::<*const (), Function<S, D, u8>>(self.target)(
                        self.context,
                        src,
                        dst,
                    );
                    0
                }
                1 => {
                    std::mem::transmute::<*const (), Function<S, D, bool>>(self.target)(
                        self.context,
                        src,
                        dst,
                    );
                    0
                }
                7 => {
                    std::mem::transmute::<*const (), Function<S, D, f64>>(self.target)(
                        self.context,
                        src,
                        dst,
                    );
                    0
                }
                2 | 9 | 10 | 256.. => std::mem::transmute::<*const (), Function<S, D, usize>>(
                    self.target,
                )(self.context, src, dst),
                _ => std::mem::transmute::<*const (), Function<S, D, *const ()>>(self.target)(
                    self.context,
                    src,
                    dst,
                ) as usize,
            }
        }
    }
}
