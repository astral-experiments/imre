//! Copy file contents without copying permissions or truncating an alias of the source.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, symlink};
use std::path::Path;
use std::rc::Rc;

use super::super::exceptions::{Error, Kind, finish, push_string_repr};
use super::super::files::{close_file, errno_error, errno_with_paths};
use super::super::{Bytes, Object, Str, bytes, object};

type Result<T> = std::result::Result<T, Error>;

enum Name<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
    Path(Rc<Str>),
}

impl<'a> Name<'a> {
    /// The call retains all three possible handle kinds until the operation finishes.
    unsafe fn from_handle(value: *const (), tag: u32) -> Self {
        // SAFETY: the compiler supplies the tag for the exact live allocation.
        unsafe {
            match tag {
                0 => Self::Text(&*value.cast::<Str>()),
                1 => Self::Bytes(&*value.cast::<Bytes>()),
                2 => Self::Path(object(value.cast()).0.with(|s| s.path_text().clone())),
                _ => unreachable!(),
            }
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Text(text) => Path::new(text),
            Self::Bytes(bytes) => Path::new(OsStr::from_bytes(bytes)),
            Self::Path(text) => Path::new(text.as_str()),
        }
    }

    fn repr(&self, original: bool) -> String {
        let mut result = String::new();
        match self {
            Self::Bytes(value) => bytes::write_repr(&mut result, value),
            Self::Path(text) if original => {
                result.push_str("PosixPath(");
                push_string_repr(text, &mut result);
                result.push(')');
            }
            _ => push_string_repr(self.path().to_str().unwrap(), &mut result),
        }
        result
    }

    fn display(&self) -> String {
        match self {
            Self::Bytes(_) => self.repr(false),
            _ => self.path().display().to_string(),
        }
    }

    fn error(&self, error: io::Error) -> Error {
        errno_with_paths(error, |out| {
            out.push_str(": ");
            out.push_str(&self.repr(false));
        })
    }
}

/// Match nested Python file context managers: open the source first, close the
/// destination first, and let an explicit close error supersede the copy error.
fn copy_data(src: &Name<'_>, dst: &Name<'_>) -> Result<()> {
    let mut input = File::open(src.path()).map_err(|e| src.error(e))?;
    let result = (|| {
        if input.metadata().map_err(errno_error)?.is_dir() {
            return Err(src.error(io::Error::from_raw_os_error(libc::EISDIR)));
        }
        let mut output = File::create(dst.path()).map_err(|e| {
            if e.kind() == io::ErrorKind::IsADirectory && !dst.path().exists() {
                Error::new(
                    Kind::FileNotFoundError,
                    format!("Directory does not exist: {}", dst.display()),
                )
                .with_cause(dst.error(e))
            } else {
                dst.error(e)
            }
        })?;
        let copied = io::copy(&mut input, &mut output).map_err(errno_error);
        close_file(output)
            .map_err(errno_error)
            .and(copied)
            .map(|_| ())
    })();
    close_file(input).map_err(errno_error).and(result)
}

fn copyfile(src: &Name<'_>, dst: &Name<'_>, follow: bool) -> Result<()> {
    if [src, dst]
        .iter()
        .any(|name| name.path().as_os_str().as_bytes().contains(&0))
    {
        return Err(Error::new(Kind::ValueError, "embedded null byte"));
    }
    let source = fs::metadata(src.path()).ok();
    let target = fs::metadata(dst.path()).ok();
    if let (Some(source), Some(target)) = (&source, &target)
        && source.dev() == target.dev()
        && source.ino() == target.ino()
    {
        return Err(Error::new(
            Kind::SameFileError,
            format!(
                "{} and {} are the same file",
                src.repr(true),
                dst.repr(true)
            ),
        ));
    }
    for (name, stat) in [(src, source), (dst, target)] {
        if stat.is_some_and(|s| s.file_type().is_fifo()) {
            return Err(Error::new(
                Kind::SpecialFileError,
                format!("`{}` is a named pipe", name.display()),
            ));
        }
    }
    if !follow && src.path().is_symlink() {
        let target = fs::read_link(src.path()).map_err(|e| src.error(e))?;
        return symlink(&target, dst.path()).map_err(|e| {
            errno_with_paths(e, |message| {
                message.push_str(": ");
                if matches!(src, Name::Bytes(_)) {
                    bytes::write_repr(message, target.as_os_str().as_bytes());
                } else {
                    push_string_repr(&target.to_string_lossy(), message);
                }
                message.push_str(" -> ");
                message.push_str(&dst.repr(false));
            })
        });
    }
    copy_data(src, dst)
}

/// # Safety
/// The two handles are rooted and their tags are 0 (str), 1 (bytes), or 2 (Path).
/// A successful call transfers a new reference to the *original* destination.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_copyfile(
    src: *const (),
    src_tag: u32,
    dst: *const (),
    dst_tag: u32,
    follow: u32,
) -> *const () {
    // SAFETY: the compiler roots the operands throughout the copy and provides their exact tags.
    let result = unsafe {
        copyfile(
            &Name::from_handle(src, src_tag),
            &Name::from_handle(dst, dst_tag),
            follow != 0,
        )
    };
    finish(
        result.map(|()| {
            // SAFETY: acquire exactly one result reference before the caller releases its operands.
            unsafe {
                match dst_tag {
                    0 => Rc::increment_strong_count(dst.cast::<Str>()),
                    1 => Rc::increment_strong_count(dst.cast::<Bytes>()),
                    2 => Rc::increment_strong_count(dst.cast::<Object>()),
                    _ => unreachable!(),
                }
            }
            dst
        }),
        std::ptr::null(),
    )
}
