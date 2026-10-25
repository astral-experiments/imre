//! Preserve OS error fields as values; a rendered error is not a source of errno.
use super::{
    Error, Kind, Object, Payload, Result, Str, Unicode16Repr, finish, object, push_string_repr,
};
#[cfg(test)]
use crate::Bytes;
use crate::StrOwner;
use ruff_python_literal::escape::{Escape, UnicodeEscape};
use std::ffi::OsString;
use std::fmt::Write;
use std::os::unix::ffi::OsStringExt;

#[derive(Clone)]
pub(in crate::objects) enum Filename {
    Str(StrOwner),
    Bytes(crate::BytesOwner),
    Path(crate::ObjectOwner),
    /// A POSIX name decoded with surrogateescape, outside Imre's scalar str domain.
    SurrogateEscaped(crate::BytesOwner),
}

impl Filename {
    pub(in crate::objects) fn from_fs_text(path: OsString) -> Self {
        match path.into_string() {
            Ok(s) => Self::Str(StrOwner::new(Str::from(s))),
            Err(raw) => Self::SurrogateEscaped(crate::BytesOwner::new(raw.into_vec())),
        }
    }

    fn append_repr(&self, output: &mut String) {
        match self {
            Self::Str(s) => push_string_repr(s, output),
            Self::Bytes(b) => crate::objects::bytes::write_repr(output, b),
            Self::SurrogateEscaped(raw) => push_fs_repr(raw, output),
            Self::Path(p) => {
                // SAFETY: the exception roots this immutable path while rendering it.
                unsafe {
                    crate::objects::values::write_repr(
                        output,
                        crate::ObjectOwner::as_ptr(p) as usize,
                        crate::objects::words::WordKind::new(5),
                    );
                }
            }
        }
    }

    fn union_word(&self) -> Result<usize> {
        Ok(match self {
            Self::Str(s) => StrOwner::into_raw(StrOwner::clone(s)) as usize,
            Self::Bytes(b) => crate::BytesOwner::into_raw(crate::BytesOwner::clone(b)) as usize | 1,
            Self::Path(p) => {
                crate::ObjectOwner::into_raw(crate::ObjectOwner::clone(p)) as usize | 6
            }
            Self::SurrogateEscaped(_) => {
                return Err(Error::new(
                    Kind::UnicodeDecodeError,
                    "filesystem path contains surrogate-escaped bytes",
                ));
            }
        })
    }
}

/// Render POSIX surrogateescape without replacing a failed syscall with a decoding error.
fn push_fs_repr(raw: &[u8], out: &mut String) {
    let quote = if raw.contains(&b'\'') && !raw.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for chunk in raw.utf8_chunks() {
        let mut text = chunk.valid();
        // Escape both delimiters according to the choice for the complete filename.
        // Quote-free spans use the same Unicode printability table as ordinary str repr.
        while let Some(index) = text.find(['\'', '"']) {
            UnicodeEscape::new_repr(&text[..index])
                .write_body(&mut Unicode16Repr(out))
                .unwrap();
            let ch = char::from(text.as_bytes()[index]);
            if ch == quote {
                out.push('\\');
            }
            out.push(ch);
            text = &text[index + 1..];
        }
        UnicodeEscape::new_repr(text)
            .write_body(&mut Unicode16Repr(out))
            .unwrap();
        for byte in chunk.invalid() {
            write!(out, "\\udc{byte:02x}").unwrap();
        }
    }
    out.push(quote);
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

impl Error {
    /// Assign a filename after constructing a one-argument OSError, as rmtree's
    /// safe descriptor walker does when it detects a symlink.
    pub(in crate::objects) fn os_message_with_filename(arg: &str, filename: Filename) -> Self {
        let paths = Box::new(Filenames {
            first: filename,
            second: None,
        });
        let mut message = String::from("[Errno None] None");
        paths.append_message(&mut message);
        Self {
            kind: Kind::OSError,
            payload: Some(Payload::Arguments(Box::new(super::BuiltinArguments {
                message: StrOwner::new(message.into()),
                args: std::iter::once(Self::string_arg(arg)).collect(),
                os_paths: Some(paths),
            }))),
            cause: None,
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
            if !exception.kind.matches(Kind::OSError)
                || field == 4 && exception.kind != Kind::BlockingIOError
            {
                let name = [
                    "errno",
                    "strerror",
                    "filename",
                    "filename2",
                    "characters_written",
                ][field as usize];
                return Err(Error::new(
                    Kind::AttributeError,
                    format!("'{}' object has no attribute '{name}'", exception.name()),
                ));
            }
            let Payload::Arguments(arguments) = &exception.payload else {
                return if field == 4 {
                    Err(Error::new(Kind::AttributeError, "characters_written"))
                } else {
                    Ok(0)
                };
            };
            // Buffered write errors retain a third repr argument: the accepted byte count.
            // A filename is a separate original path owner, never a parsed message.
            Ok(match field {
                0 | 1
                    if arguments.args.len() == 2
                        || exception.kind == Kind::BlockingIOError && arguments.args.len() == 3 =>
                {
                    arguments.args[field as usize].clone().into_word()
                }
                0 | 1 => 0,
                2 => arguments
                    .os_paths
                    .as_ref()
                    .map_or(Ok(0), |p| p.first.union_word())?,
                3 => arguments
                    .os_paths
                    .as_ref()
                    .and_then(|p| p.second.as_ref())
                    .map_or(Ok(0), Filename::union_word)?,
                4 => match arguments.args.get(2) {
                    Some(word) if word.kind.raw() == 2 => word.clone().into_word(),
                    _ => return Err(Error::new(Kind::AttributeError, "characters_written")),
                },
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
    fn blocked_writes_match_os_error_and_expose_the_accepted_count() {
        let error = Error::blocked_write(libc::EAGAIN, 17);
        assert!(error.kind.matches(Kind::OSError));
        error.raise();
        // SAFETY: take transfers the raised exception, which stays live through the field read.
        unsafe {
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            assert_eq!(
                imre_os_error_field(crate::ObjectOwner::as_ptr(&error), 4),
                crate::Int::small(17).unwrap().into_raw()
            );
        }
    }

    #[test]
    fn projected_paths_keep_the_original_owners_after_the_exception_drops() {
        let a = StrOwner::new(Str::from("before"));
        let b = crate::BytesOwner::new(vec![0xff]);
        Error::from_errno(
            Kind::FileNotFoundError,
            libc::ENOENT,
            "missing",
            Some(Filename::Str(StrOwner::clone(&a))),
            Some(Filename::Bytes(crate::BytesOwner::clone(&b))),
        )
        .raise();
        // SAFETY: take transfers the pending exception; each field returns an
        // owned word, with the str/bytes union tag removed before recovering its owner.
        unsafe {
            let error = crate::ObjectOwner::from_raw(imre_error_take());
            let message =
                StrOwner::from_raw(imre_exception_message(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(message.as_str(), "[Errno 2] missing: 'before' -> b'\\xff'");
            let repr = StrOwner::from_raw(imre_exception_repr(crate::ObjectOwner::as_ptr(&error)));
            assert_eq!(repr.as_str(), "FileNotFoundError(2, 'missing')");
            assert_eq!(
                imre_os_error_field(crate::ObjectOwner::as_ptr(&error), 0),
                5
            );
            let reason = StrOwner::from_raw(imre_os_error_field(
                crate::ObjectOwner::as_ptr(&error),
                1,
            ) as *const Str);
            assert_eq!(reason.as_str(), "missing");
            let first = StrOwner::from_raw(imre_os_error_field(
                crate::ObjectOwner::as_ptr(&error),
                2,
            ) as *const Str);
            let second = imre_os_error_field(crate::ObjectOwner::as_ptr(&error), 3);
            assert_eq!(second & 7, 1);
            let second = crate::BytesOwner::from_raw((second & !7) as *const Bytes);
            drop(error);
            assert!(StrOwner::ptr_eq(&a, &first));
            assert!(crate::BytesOwner::ptr_eq(&b, &second));
        }
    }
}
