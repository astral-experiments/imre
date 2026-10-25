//! Copy file contents without copying permissions or truncating an alias of the source.

use crate::StrOwner;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileTypeExt, MetadataExt, symlink};
use std::path::Path;

use super::super::exceptions::{Error, Filename, Kind, finish, push_string_repr};
use super::super::files::{close_file, errno_error, errno_with_filenames};
use super::super::{Bytes, Object, Str, bytes, paths};

type Result<T> = std::result::Result<T, Error>;

enum Name {
    Text(StrOwner),
    Bytes(crate::BytesOwner),
    Path(StrOwner),
}

impl Name {
    /// The call retains all three possible handle kinds until the operation finishes.
    unsafe fn from_handle(value: *const (), tag: u32) -> Self {
        // SAFETY: the compiler supplies the tag for the exact live allocation.
        unsafe {
            match tag {
                0 => Self::Text(crate::StrOwner::acquire_word(value as usize)),
                1 => Self::Bytes(crate::acquire_bytes(value as usize)),
                2 => Self::Path(StrOwner::from_raw(paths::imre_path_text(value.cast()))),
                _ => unreachable!(),
            }
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Text(text) | Self::Path(text) => Path::new(text.as_str()),
            Self::Bytes(bytes) => Path::new(OsStr::from_bytes(bytes)),
        }
    }

    /// Convert the name when the copy operation reaches its filesystem lookup.
    fn checked_path(&self) -> Result<&Path> {
        let path = self.path();
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(Error::new(Kind::ValueError, "embedded null byte"));
        }
        Ok(path)
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

    fn filename(&self) -> Filename {
        match self {
            Self::Text(s) | Self::Path(s) => Filename::Str(StrOwner::clone(s)),
            Self::Bytes(b) => Filename::Bytes(crate::BytesOwner::clone(b)),
        }
    }

    fn error(&self, error: io::Error) -> Error {
        errno_with_filenames(error, Some(self.filename()), None)
    }
}

/// Match nested Python file context managers: open the source first, close the
/// destination first, and let an explicit close error supersede the copy error.
fn copy_data(src: &Name, dst: &Name) -> Result<()> {
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

fn copyfile(src: &Name, dst: &Name, follow: bool) -> Result<()> {
    let source = fs::metadata(src.checked_path()?).ok();
    let target = fs::metadata(dst.checked_path()?).ok();
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
        return copy_link(src, dst);
    }
    copy_data(src, dst)
}

fn copy_link(src: &Name, dst: &Name) -> Result<()> {
    let target = fs::read_link(src.path()).map_err(|e| src.error(e))?;
    symlink(&target, dst.path()).map_err(|e| {
        let target = if matches!(src, Name::Bytes(_)) {
            Filename::Bytes(crate::BytesOwner::new(target.into_os_string().into_vec()))
        } else {
            Filename::from_fs_text(target.into_os_string())
        };
        errno_with_filenames(e, Some(target), Some(dst.filename()))
    })
}

/// Keep os.path.basename's empty final component for sources ending in a slash.
fn destination(src: &Name, dst: &Name) -> Result<std::path::PathBuf> {
    if matches!(src, Name::Bytes(_)) != matches!(dst, Name::Bytes(_)) {
        return Err(Error::new(
            Kind::TypeError,
            "Can't mix strings and bytes in path components",
        ));
    }
    let name = src
        .path()
        .as_os_str()
        .as_bytes()
        .rsplit(|&b| b == b'/')
        .next()
        .unwrap();
    Ok(dst.path().join(OsStr::from_bytes(name)))
}

impl Name {
    fn joined(&self, path: &Path) -> Result<Name> {
        if matches!(self, Name::Bytes(_)) {
            Ok(Name::Bytes(crate::BytesOwner::new(
                path.as_os_str().as_bytes().to_vec(),
            )))
        } else {
            path.to_str()
                .map(|s| Name::Text(StrOwner::new(Str::from(s))))
                .ok_or_else(|| {
                    Error::new(
                        Kind::UnicodeDecodeError,
                        "filesystem path is not valid UTF-8",
                    )
                })
        }
    }
}

fn run(op: u32, src: &Name, dst: &Name, follow: bool) -> Result<Option<std::path::PathBuf>> {
    match op {
        0 => copyfile(src, dst, follow)?,
        1 => metadata::copymode(src, dst, follow)?,
        2 => metadata::copystat(src, dst, follow)?,
        3 => metadata::copyxattr(src, dst, follow)?,
        4 | 5 => {
            let path = dst
                .path()
                .is_dir()
                .then(|| destination(src, dst))
                .transpose()?;
            let joined;
            let dst = if let Some(path) = &path {
                joined = dst.joined(path)?;
                &joined
            } else {
                dst
            };
            copyfile(src, dst, follow)?;
            if op == 4 {
                metadata::copymode(src, dst, follow)?;
            } else {
                metadata::copystat(src, dst, follow)?;
            }
            return Ok(path);
        }
        _ => unreachable!(),
    }
    Ok(None)
}

#[path = "metadata.rs"]
mod metadata;
#[path = "move.rs"]
mod moving;
#[path = "tree.rs"]
mod tree;

/// Transfer one reference of the original path's concrete type to the caller.
unsafe fn retain_handle(value: *const (), tag: u32) -> usize {
    // SAFETY: the compiler roots the live str, bytes, or Path with this tag.
    unsafe {
        match tag {
            0 => StrOwner::increment_strong_count(value.cast::<Str>()),
            1 => crate::retain_bytes(value.cast::<Bytes>()),
            2 => crate::ObjectOwner::increment_strong_count(value.cast::<Object>()),
            _ => unreachable!(),
        }
    }
    value as usize
}

/// # Safety
/// The two handles are rooted and tagged as str, bytes, or Path. For a Path
/// supplied to copy/copy2, a directory produces a str; otherwise tag the
/// original Path as a member of the compiler's str | Path union.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_copy(
    op: u32,
    src: *const (),
    src_tag: u32,
    dst: *const (),
    dst_tag: u32,
    follow: u32,
) -> usize {
    // SAFETY: both handle tags describe their exact, live compiler operands.
    let result = unsafe {
        run(
            op,
            &Name::from_handle(src, src_tag),
            &Name::from_handle(dst, dst_tag),
            follow != 0,
        )
    };
    finish(
        result.and_then(|joined| {
            if matches!(op, 1..=3) {
                return Ok(0);
            }
            if let Some(joined) = joined {
                let value = joined.into_os_string();
                return Ok(if dst_tag == 1 {
                    crate::BytesOwner::into_raw(crate::BytesOwner::new(value.into_vec())) as usize
                } else {
                    StrOwner::into_raw(StrOwner::new(super::super::paths::path_utf8(value)?))
                        as usize
                });
            }
            // SAFETY: return exactly one new reference of the original destination's type.
            Ok(unsafe { retain_handle(dst, dst_tag) }
                | if matches!(op, 4 | 5) && dst_tag == 2 {
                    6
                } else {
                    0
                })
        }),
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_errors_retain_the_original_bytes_filenames() {
        let dir = std::env::temp_dir().join(format!("imre-shutil-{}", getrandom::u64().unwrap()));
        fs::create_dir(&dir).unwrap();
        let source = crate::BytesOwner::new(dir.join("missing").into_os_string().into_vec());
        let destination = crate::BytesOwner::new(dir.join("existing").into_os_string().into_vec());
        fs::write(dir.join("existing"), []).unwrap();
        symlink("missing", dir.join("link")).unwrap();
        let missing = copyfile(
            &Name::Bytes(crate::BytesOwner::clone(&source)),
            &Name::Bytes(crate::BytesOwner::clone(&destination)),
            true,
        )
        .unwrap_err();
        let exists = copyfile(
            &Name::Bytes(crate::BytesOwner::new(
                dir.join("link").into_os_string().into_vec(),
            )),
            &Name::Bytes(crate::BytesOwner::clone(&destination)),
            false,
        )
        .unwrap_err();
        fs::remove_dir_all(dir).unwrap();
        // The calls' temporary Name handles have dropped. Only the exceptions'
        // filename and filename2 fields can retain these two original objects.
        assert_eq!(crate::BytesOwner::strong_count(&source), 2);
        assert_eq!(crate::BytesOwner::strong_count(&destination), 2);
        drop((missing, exists));
        assert_eq!(crate::BytesOwner::strong_count(&source), 1);
        assert_eq!(crate::BytesOwner::strong_count(&destination), 1);
    }
}
