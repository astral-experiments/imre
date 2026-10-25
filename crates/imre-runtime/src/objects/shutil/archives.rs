//! Archive dispatch. Built-in formats resolve members beneath their supplied root; legacy custom packers use the original change-and-restore protocol.
mod registry;
mod tar_extract;
mod tarball;
mod zip;

use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::Rc;

use super::super::exceptions::{Error, Filename, Kind, Result, finish};
use super::super::files::{errno_error, errno_with_filenames};
use super::super::words::{OwnedWord, WordKind};
use super::super::{Object, Storage, Str, paths, posixpath};
use crate::{Int, ObjectOwner, StrOwner};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Tar,
    Gzip,
    Bzip,
    Xz,
    Zstd,
    Zip,
}

fn path_error(err: io::Error, path: &Path) -> Error {
    if path.as_os_str().as_bytes().contains(&0) {
        return Error::new(Kind::ValueError, "embedded null byte");
    }
    errno_with_filenames(
        err,
        Some(Filename::from_fs_text(path.as_os_str().to_owned())),
        None,
    )
}

fn mkdir_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.is_dir()
    {
        fs::create_dir_all(parent).map_err(|e| path_error(e, parent))?;
    }
    Ok(())
}

fn normalized(name: &str, cwd: Option<&Path>) -> Result<Str> {
    let bytes = posixpath::normalize(
        name.as_bytes(),
        cwd.filter(|_| !Path::new(name).is_absolute())
            .map(|p| p.as_os_str().as_bytes()),
    )
    .map_err(|_| Error::allocation())?;
    let path = String::from_utf8(bytes).map_err(|_| {
        Error::new(
            Kind::UnicodeDecodeError,
            "filesystem path is not valid UTF-8",
        )
    })?;
    Ok(path.into())
}

struct ArchiveRoot {
    text: StrOwner,
    path: Option<ObjectOwner>,
}

struct MakeOptions<'a> {
    root: Option<ArchiveRoot>,
    dir: Option<&'a str>,
    dry: OwnedWord,
    owner: Option<&'a str>,
    group: Option<&'a str>,
    base_class: Option<&'static str>,
}

fn make(base: &str, format: &str, options: MakeOptions<'_>) -> Result<Option<StrOwner>> {
    let MakeOptions {
        root,
        dir,
        dry,
        owner,
        group,
        base_class,
    } = options;
    let (handler, suffix) = registry::archiver(format).ok_or_else(|| {
        Error::new(
            Kind::ValueError,
            format!("unknown archive format '{format}'"),
        )
    })?;
    if let Some(root) = &root {
        let path = Path::new(root.text.as_str());
        if root.text.as_bytes().contains(&0) {
            return Err(Error::new(
                Kind::ValueError,
                "stat: embedded null character in path",
            ));
        }
        if !fs::metadata(path)
            .map_err(|e| errno_with_filenames(e, Some(Filename::Str(root.text.clone())), None))?
            .is_dir()
        {
            // os.stat converts a Path to text; shutil's own raise uses the original operand.
            return Err(Error::from_errno(
                Kind::NotADirectoryError,
                libc::ENOTDIR,
                "Not a directory",
                Some(root.path.as_ref().map_or_else(
                    || Filename::Str(root.text.clone()),
                    |path| Filename::Path(path.clone()),
                )),
                None,
            ));
        }
    }
    // Both bool and int retain their original representation for a registered packer.
    let dry_run = match dry.kind.raw() {
        1 => dry.word != 0,
        // SAFETY: the owner has the checked integer representation.
        2 => unsafe { Int::borrow(dry.word) }.as_small() != Some(0),
        _ => unreachable!(),
    };
    if let registry::Handler::Custom(callback) = handler {
        let mut saved = None;
        let mut base = StrOwner::new(Str::from(base));
        let mut callback_root = None;
        if let Some(root) = &root {
            if callback.supports_root_dir() {
                callback_root = Some(root.text.clone());
            } else {
                let cwd = std::env::current_dir().map_err(errno_error)?;
                base = StrOwner::new(normalized(&base, Some(&cwd))?);
                if !dry_run {
                    std::env::set_current_dir(root.text.as_str())
                        .map_err(|e| path_error(e, Path::new(root.text.as_str())))?;
                }
                saved = Some(cwd);
            }
        }
        let result = callback.pack(
            base,
            StrOwner::new(Str::from(dir.unwrap_or("."))),
            dry,
            owner,
            group,
            callback_root,
        );
        // Restore even on the dry path and while a callback exception is pending.
        if let Some(cwd) = saved {
            std::env::set_current_dir(&cwd).map_err(|e| path_error(e, &cwd))?;
        }
        return result;
    }
    let registry::Handler::Builtin(format) = handler else {
        unreachable!()
    };
    if root.is_none()
        && let Some(class) = base_class
    {
        return Err(Error::new(
            Kind::TypeError,
            format!("unsupported operand type(s) for +: '{class}' and 'str'"),
        ));
    }
    let root = root.as_ref().map(|root| root.text.as_str());
    let result = format!("{base}{suffix}");
    if !dry_run {
        let dir = dir.unwrap_or(".");
        let source = root.map_or_else(|| Path::new(dir).to_owned(), |r| Path::new(r).join(dir));
        let target = Path::new(&result);
        mkdir_parent(target)?;
        let file = File::create(target).map_err(|e| path_error(e, target))?;
        if format == Format::Zip {
            zip::create(file, &source, dir)?;
        } else {
            tarball::create(file, format, &source, dir, owner, group)?;
        }
    }
    let result = if root.is_some() {
        normalized(
            &result,
            Some(&std::env::current_dir().map_err(errno_error)?),
        )?
    } else {
        result.into()
    };
    Ok(Some(StrOwner::new(result)))
}

fn unpack(
    filename: StrOwner,
    dest: Option<StrOwner>,
    format: Option<&str>,
    filter: Option<StrOwner>,
) -> Result<()> {
    // The default is the current directory's absolute path. In particular,
    // getcwd can fail before even an unknown archive format is inspected.
    let cwd = if dest.is_none() {
        Some(std::env::current_dir().map_err(errno_error)?)
    } else {
        None
    };
    let driver = registry::unpacker(format, filename.as_str()).ok_or_else(|| match format {
        Some(name) => Error::new(Kind::ValueError, format!("Unknown unpack format '{name}'")),
        None => Error::new(
            Kind::ShutilReadError,
            format!("Unknown archive format '{filename}'"),
        ),
    })?;
    let format = match driver {
        registry::Handler::Custom(callback) => {
            let dest = match dest {
                Some(s) => s,
                None => StrOwner::new(paths::path_utf8(cwd.unwrap().into_os_string())?),
            };
            return callback.call(filename, dest, filter, format.is_some());
        }
        registry::Handler::Builtin(format) => format,
    };
    let path = Path::new(filename.as_str());
    let dest = cwd.as_deref().unwrap_or_else(|| {
        Path::new(
            dest.as_deref()
                .map(Str::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("."),
        )
    });
    let filter = filter.as_deref().map(Str::as_str);
    if format == Format::Zip {
        if filter.is_some() {
            return Err(Error::new(
                Kind::TypeError,
                "_unpack_zipfile() got an unexpected keyword argument 'filter'",
            ));
        }
        zip::unpack(path, dest)
    } else {
        tarball::unpack(path, dest, filter)
    }
}

/// # Safety
/// The compiler roots all arguments. Bit zero marks a path base; the next bits
/// distinguish an optional text root, an exact path root and a tagged str/Path root.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_make_archive(
    base: *const (),
    path_flags: u32,
    format: *const Str,
    root: usize,
    dir: usize,
    dry: usize,
    dry_kind: u32,
    owner: usize,
    group: usize,
) -> *const Str {
    // SAFETY: all handles are guaranteed live until the operation returns.
    let result = unsafe {
        let owned;
        let (base, base_class) = if path_flags & 1 != 0 {
            let path = &*base.cast::<Object>();
            owned = StrOwner::from_raw(paths::imre_path_text(path));
            let class = path.0.with(|storage| match storage {
                Storage::PurePath(p) => p.class(),
                Storage::Path { .. } => "PosixPath",
                _ => unreachable!(),
            });
            (&*owned, Some(class))
        } else {
            (&*base.cast::<Str>(), None)
        };
        let text = |w: usize| (w as *const Str).as_ref().map(Str::as_str);
        let root = (root != 0).then(|| {
            let kind = path_flags >> 1;
            if kind == 1 || (kind == 2 && root & 7 == 6) {
                let ptr = if kind == 2 { root & !7 } else { root };
                ArchiveRoot {
                    text: StrOwner::from_raw(paths::imre_path_text(ptr as *const Object)),
                    path: Some(ObjectOwner::acquire_word(ptr)),
                }
            } else {
                ArchiveRoot {
                    text: StrOwner::acquire_word(root),
                    path: None,
                }
            }
        });
        make(
            base,
            &*format,
            MakeOptions {
                root,
                dir: text(dir),
                dry: WordKind::new(dry_kind).acquire(dry),
                owner: text(owner),
                group: text(group),
                base_class,
            },
        )
    };
    finish(
        result.map(|s| s.map_or(std::ptr::null(), StrOwner::into_raw)),
        std::ptr::null(),
    )
}

/// # Safety
/// The filename and every present optional string are rooted for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_unpack_archive(
    filename: *const Str,
    dest: usize,
    format: usize,
    filter: usize,
) {
    // SAFETY: the language call retains the filename and all non-null string words.
    let result = unsafe {
        let owned = |w: usize| (w != 0).then(|| StrOwner::acquire_word(w));
        unpack(
            StrOwner::acquire_word(filename as usize),
            owned(dest),
            (format as *const Str).as_ref().map(Str::as_str),
            owned(filter),
        )
    };
    finish(result, ());
}
