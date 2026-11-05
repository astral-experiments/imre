use super::{Context, Error, Id, NONE, Result, Value, builtins, c_path, native, posixpath, stat};
use crate::objects::os::scandir as stream;
use std::{
    cell::{Cell, RefCell},
    ffi::CString,
    rc::Rc,
};

pub(in crate::dynamic) const ENTRY: &str = "posix.DirEntry";
pub(in crate::dynamic) const ITER: &str = "posix.ScandirIterator";
pub(in crate::dynamic) const ENTRY_METHODS: &[&str] = &[
    "__fspath__",
    "__repr__",
    "inode",
    "is_dir",
    "is_file",
    "is_junction",
    "is_symlink",
    "stat",
];
pub(in crate::dynamic) const ITER_METHODS: &[&str] = &[
    "__del__",
    "__enter__",
    "__exit__",
    "__iter__",
    "__next__",
    "close",
];

pub(in crate::dynamic) struct Scan {
    directory: RefCell<stream::Directory>,
    prefix: Option<CString>,
    bytes: bool,
    pub(in crate::dynamic) source: Cell<Id>,
    dir_fd: i32,
}

#[derive(Clone)]
pub(in crate::dynamic) struct Entry {
    pub(in crate::dynamic) name: Id,
    pub(in crate::dynamic) path: Id,
    ino: libc::ino_t,
    dtype: u8,
    dir_fd: i32,
    stat: Option<Status>,
    lstat: Option<Status>,
}

#[derive(Clone, Copy)]
struct Status {
    object: Id,
    mode: libc::mode_t,
}

impl Entry {
    pub(in crate::dynamic) fn cached_objects(&self) -> impl Iterator<Item = Id> {
        [self.stat, self.lstat]
            .into_iter()
            .flatten()
            .map(|status| status.object)
    }
}

fn path_error(ctx: &mut Context, error: std::io::Error, path: Id) -> Error {
    let native = crate::objects::errno_error(error).into();
    if path == NONE {
        native
    } else {
        ctx.with_errno_filename(native, path).unwrap_or_else(|e| e)
    }
}

pub(super) fn open(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [source] = builtins::bind_builtin_arguments("scandir", args, kwargs, ["path"], 0)?;
    let mut source = source.unwrap_or(NONE);
    let mut bytes = false;
    let mut dir_fd = libc::AT_FDCWD;
    let is_spelling = matches!(
        ctx.heap.get(ctx.scalar_value(source)),
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_)
    );
    let prefix = if source == NONE {
        Some(CString::from(c"."))
    } else if !is_spelling && let Some(fd) = ctx.index_value(source)? {
        stat::warn_bool_fd(ctx, source)?;
        let fd = native::directory_fd_number(&fd)?;
        if fd == -1 {
            Some(CString::from(c"."))
        } else {
            dir_fd = fd;
            None
        }
    } else {
        source = posixpath::path_arg(
            ctx,
            source,
            "scandir: path should be string, bytes, os.PathLike, integer or None",
        )?;
        bytes = matches!(ctx.heap.get(ctx.scalar_value(source)), Value::Bytes(_));
        Some(c_path(
            ctx,
            source,
            "scandir: embedded null character in path",
        )?)
    };
    let directory = match prefix.as_ref() {
        Some(prefix) => stream::Directory::open(prefix),
        None => stream::Directory::from_fd(stream::duplicate(dir_fd)?),
    }
    .map_err(|e| path_error(ctx, e, source))?;
    let iterator = ctx.alloc(Value::Scandir(Rc::new(Scan {
        directory: RefCell::new(directory),
        prefix,
        bytes,
        source: Cell::new(source),
        dir_fd,
    })));
    ctx.heap.register_finalizer(iterator);
    Ok(iterator)
}

pub(in crate::dynamic) fn iterator_repr(scan: &Rc<Scan>) -> String {
    format!("<posix.ScandirIterator object at {:p}>", Rc::as_ptr(scan))
}

/// Close the directory and stop scheduling cleanup for its iterator.
fn close(ctx: &mut Context, owner: Id, scan: &Scan) {
    scan.directory.borrow_mut().close();
    ctx.heap.disarm_finalizer(owner);
}

pub(in crate::dynamic) fn finalize(ctx: &mut Context, owner: Id, scan: &Rc<Scan>) {
    if scan.directory.borrow().is_open() {
        close(ctx, owner, scan);
        let iterator = iterator_repr(scan);
        if let Err(error) = ctx.emit_builtin_warning_with_source(
            crate::objects::exceptions::Kind::ResourceWarning,
            &format!("unclosed scandir iterator {iterator}"),
            1,
            owner,
        ) {
            let warning = ctx.builtin("Warning");
            if ctx.raised_error_matches(&error, warning) {
                super::super::super::unraisable::report(
                    ctx,
                    error,
                    true,
                    None,
                    &format!("Exception ignored while finalizing scandir iterator {iterator}"),
                );
            }
        }
    }
    ctx.heap.weak_edge_removed(scan.source.replace(NONE));
}

fn spelling(ctx: &mut Context, bytes: Vec<u8>, as_bytes: bool) -> Result<Id> {
    if as_bytes {
        return Ok(ctx.alloc(Value::Bytes(bytes)));
    }
    let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
    Ok(ctx.text_codepoints(codec.decode_filesystem(&bytes)?))
}

pub(in crate::dynamic) fn advance(
    ctx: &mut Context,
    owner: Id,
    scan: Rc<Scan>,
) -> Result<Option<Id>> {
    let next = scan.directory.borrow_mut().next_entry();
    // End-of-directory and read failures both close the native directory stream.
    if !matches!(next, Ok(Some(_))) {
        ctx.heap.disarm_finalizer(owner);
    }
    let next = next.map_err(|error| path_error(ctx, error, scan.source.get()))?;
    let Some(entry) = next else {
        return Ok(None);
    };
    let result = (|| {
        let path = match &scan.prefix {
            Some(prefix) => {
                let mut joined = prefix.as_bytes().to_vec();
                if !joined.ends_with(b"/") {
                    joined.push(b'/');
                }
                joined.extend_from_slice(&entry.name);
                Some(spelling(ctx, joined, scan.bytes)?)
            }
            None => None,
        };
        let name = spelling(ctx, entry.name, scan.bytes)?;
        Ok(ctx.alloc(Value::DirEntry(Entry {
            path: path.unwrap_or(name),
            name,
            dir_fd: scan.dir_fd,
            ino: entry.ino,
            dtype: entry.d_type,
            stat: None,
            lstat: None,
        })))
    })();
    if result.is_err() {
        close(ctx, owner, &scan);
    }
    result.map(Some)
}

fn metadata(ctx: &mut Context, id: Id, entry: &Entry, follow: bool) -> Result<Status> {
    let Value::DirEntry(current) = ctx.heap.get(id) else {
        unreachable!()
    };
    if let Some(status) = if follow { current.stat } else { current.lstat } {
        return Ok(status);
    }
    // A non-link shares one stat_result for both follow modes.
    let status = if follow && !symlink(ctx, id, entry)? {
        metadata(ctx, id, entry, false)?
    } else {
        let path = c_path(ctx, entry.path, "embedded null byte")?;
        let native = stream::stat(entry.dir_fd, &path, follow)
            .map_err(|e| path_error(ctx, e, entry.path))?;
        Status {
            mode: native.st_mode,
            object: stat::packed(ctx, &native),
        }
    };
    let Value::DirEntry(stored) = ctx.heap.get_mut(id) else {
        unreachable!()
    };
    if follow {
        stored.stat = Some(status);
    } else {
        stored.lstat = Some(status);
    }
    Ok(status)
}

fn symlink(ctx: &mut Context, id: Id, entry: &Entry) -> Result<bool> {
    if entry.dtype != libc::DT_UNKNOWN {
        return Ok(entry.dtype == libc::DT_LNK);
    }
    Ok(metadata(ctx, id, entry, false)?.mode & libc::S_IFMT == libc::S_IFLNK)
}

fn predicate(ctx: &mut Context, id: Id, entry: &Entry, wanted: u8, follow: bool) -> Result<bool> {
    if entry.dtype != libc::DT_UNKNOWN && (!follow || entry.dtype != libc::DT_LNK) {
        return Ok(entry.dtype == wanted);
    }
    match metadata(ctx, id, entry, follow) {
        Ok(status) => Ok(status.mode & libc::S_IFMT
            == match wanted {
                libc::DT_LNK => libc::S_IFLNK,
                libc::DT_DIR => libc::S_IFDIR,
                _ => libc::S_IFREG,
            }),
        Err(e) if e.kind == "FileNotFoundError" => Ok(false),
        Err(e) => Err(e),
    }
}

pub(in crate::dynamic) fn method(
    ctx: &mut Context,
    class: &str,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&id, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("descriptor '{name}' of '{class}' object needs an argument"),
        ));
    };
    if class == ENTRY
        && let Value::DirEntry(entry) = ctx.heap.get(id).clone()
    {
        let follow = if matches!(name, "stat" | "is_dir" | "is_file") {
            let [follow] =
                builtins::bind_method_arguments(name, args, kwargs, ["follow_symlinks"], 0)?;
            ctx.optional_truth(follow, true)?
        } else {
            no_args(class.trim_start_matches("posix."), name, args, kwargs)?;
            false
        };
        return Ok(match name {
            "__fspath__" => entry.path,
            "__repr__" => {
                let repr = ctx.repr(entry.name);
                ctx.text(format!("<DirEntry {repr}>"))
            }
            "inode" => ctx.alloc(Value::Int(crate::Int::new(entry.ino.into()))),
            "stat" => metadata(ctx, id, &entry, follow)?.object,
            "is_symlink" => builtins::boolean(predicate(ctx, id, &entry, libc::DT_LNK, false)?),
            "is_file" => builtins::boolean(predicate(ctx, id, &entry, libc::DT_REG, follow)?),
            "is_dir" => builtins::boolean(predicate(ctx, id, &entry, libc::DT_DIR, follow)?),
            "is_junction" => builtins::boolean(false),
            _ => unreachable!(),
        });
    }
    if class == ITER
        && let Value::Scandir(scan) = ctx.heap.get(id).clone()
    {
        if name != "__exit__" {
            no_args(class.trim_start_matches("posix."), name, args, kwargs)?;
        } else if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "ScandirIterator.__exit__() takes no keyword arguments",
            ));
        }
        return Ok(match name {
            "__iter__" | "__enter__" => id,
            "__del__" => {
                finalize(ctx, id, &scan);
                NONE
            }
            "__next__" => advance(ctx, id, scan)?.ok_or_else(|| Error::new("StopIteration", ""))?,
            "close" | "__exit__" => {
                close(ctx, id, &scan);
                NONE
            }
            _ => unreachable!(),
        });
    }
    Err(Error::new(
        "TypeError",
        format!(
            "descriptor '{name}' for '{class}' objects doesn't apply to a '{}' object",
            ctx.type_name(id)
        ),
    ))
}

fn no_args(class: &str, name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<()> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{class}.{name}() takes no keyword arguments"),
        ));
    }
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("{class}.{name}() takes no arguments ({} given)", args.len()),
        ));
    }
    Ok(())
}
