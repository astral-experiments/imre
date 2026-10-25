//! Copy file contents without copying permissions or truncating an alias of the source.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileTypeExt, MetadataExt, symlink};
use std::path::Path;
use std::rc::Rc;

use super::super::exceptions::{Error, Kind, finish, push_string_repr};
use super::super::files::names::Name;
use super::super::files::{close_file, errno_error, errno_with_paths};
use super::super::{Bytes, Object, Str, bytes};

type Result<T> = std::result::Result<T, Error>;

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
            Ok(Name::Bytes(Rc::new(path.as_os_str().as_bytes().to_vec())))
        } else {
            path.to_str()
                .map(|s| Name::Text(Rc::new(Str::from(s))))
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
    if [src, dst]
        .iter()
        .any(|n| n.path().as_os_str().as_bytes().contains(&0))
    {
        return Err(Error::new(Kind::ValueError, "embedded null byte"));
    }
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
                    Rc::into_raw(Rc::new(value.into_vec())) as usize
                } else {
                    Rc::into_raw(Rc::new(super::super::paths::path_utf8(value)?)) as usize
                });
            }
            // SAFETY: return exactly one new reference of the original destination's type.
            unsafe {
                match dst_tag {
                    0 => Rc::increment_strong_count(dst.cast::<Str>()),
                    1 => Rc::increment_strong_count(dst.cast::<Bytes>()),
                    2 => Rc::increment_strong_count(dst.cast::<Object>()),
                    _ => unreachable!(),
                }
            }
            Ok(dst as usize
                | if matches!(op, 4 | 5) && dst_tag == 2 {
                    6
                } else {
                    0
                })
        }),
        0,
    )
}
