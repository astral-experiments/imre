//! Preserve OS error fields as values; a rendered error is not a source of errno.
use super::{Error, Kind, Object, Payload, Str, finish, object, push_string_repr};
use crate::Bytes;
use std::rc::Rc;

#[derive(Clone)]
pub(in crate::objects) enum Filename {
    Str(Rc<Str>),
    Bytes(Rc<Bytes>),
    Path(Rc<Object>),
}

impl Filename {
    fn append_repr(&self, output: &mut String) {
        match self {
            Self::Str(s) => push_string_repr(s, output),
            Self::Bytes(b) => crate::objects::bytes::write_repr(output, b),
            Self::Path(p) => {
                // SAFETY: the exception roots this immutable path while rendering it.
                unsafe {
                    crate::objects::values::write_repr(
                        output,
                        Rc::as_ptr(p) as usize,
                        crate::objects::words::WordKind::new(5),
                    );
                }
            }
        }
    }

    fn union_word(&self) -> usize {
        match self {
            Self::Str(s) => Rc::into_raw(Rc::clone(s)) as usize,
            Self::Bytes(b) => Rc::into_raw(Rc::clone(b)) as usize | 1,
            Self::Path(p) => Rc::into_raw(Rc::clone(p)) as usize | 6,
        }
    }
}

#[derive(Clone)]
pub(super) struct Filenames {
    pub(super) first: Filename,
    pub(super) second: Option<Filename>,
}

impl Filenames {
    pub(super) fn append_message(&self, output: &mut String) {
        output.push_str(": ");
        self.first.append_repr(output);
        if let Some(path) = &self.second {
            output.push_str(" -> ");
            path.append_repr(output);
        }
    }
}

/// Return an optional integer, string, or str/bytes union in its owned word.
///
/// # Safety
/// The exception is rooted and field is an index into the four public fields.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_os_error_field(value: *const Object, field: u32) -> usize {
    // SAFETY: the rooted immutable payload retains each projected owner.
    let result = unsafe {
        object(value).0.with(|storage| {
            let exception = storage.exception();
            if !exception.kind.matches(Kind::OSError) {
                let name = ["errno", "strerror", "filename", "filename2"][field as usize];
                return Err(Error::new(
                    Kind::AttributeError,
                    format!("'{}' object has no attribute '{name}'", exception.name()),
                ));
            }
            let Payload::Arguments(arguments) = &exception.payload else {
                return Ok(0);
            };
            // errno and strerror are exactly the two arguments used for repr.
            // A filename is a separate original path owner, never a parsed message.
            Ok(match field {
                0 | 1 => arguments.args[field as usize].clone().into_word(),
                2 => arguments
                    .os_paths
                    .as_ref()
                    .map_or(0, |p| p.first.union_word()),
                3 => arguments
                    .os_paths
                    .as_ref()
                    .and_then(|p| p.second.as_ref())
                    .map_or(0, Filename::union_word),
                _ => unreachable!(),
            })
        })
    };
    finish(result, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::exceptions::{
        imre_error_take, imre_exception_message, imre_exception_repr,
    };

    #[test]
    fn projected_paths_keep_the_original_owners_after_the_exception_drops() {
        let a = Rc::new(Str::from("before"));
        let b = Rc::new(vec![0xff]);
        Error::from_errno(
            Kind::FileNotFoundError,
            libc::ENOENT,
            "missing",
            Some(Filename::Str(Rc::clone(&a))),
            Some(Filename::Bytes(Rc::clone(&b))),
        )
        .raise();
        // SAFETY: take transfers the pending exception; each field returns an
        // owned word, with the str/bytes union tag removed before Rc::from_raw.
        unsafe {
            let error = Rc::from_raw(imre_error_take());
            let message = Rc::from_raw(imre_exception_message(Rc::as_ptr(&error)));
            assert_eq!(message.as_str(), "[Errno 2] missing: 'before' -> b'\\xff'");
            let repr = Rc::from_raw(imre_exception_repr(Rc::as_ptr(&error)));
            assert_eq!(repr.as_str(), "FileNotFoundError(2, 'missing')");
            assert_eq!(imre_os_error_field(Rc::as_ptr(&error), 0), 5);
            let reason = Rc::from_raw(imre_os_error_field(Rc::as_ptr(&error), 1) as *const Str);
            assert_eq!(reason.as_str(), "missing");
            let first = Rc::from_raw(imre_os_error_field(Rc::as_ptr(&error), 2) as *const Str);
            let second = imre_os_error_field(Rc::as_ptr(&error), 3);
            assert_eq!(second & 7, 1);
            let second = Rc::from_raw((second & !7) as *const Bytes);
            drop(error);
            assert!(Rc::ptr_eq(&a, &first));
            assert!(Rc::ptr_eq(&b, &second));
        }
    }
}
