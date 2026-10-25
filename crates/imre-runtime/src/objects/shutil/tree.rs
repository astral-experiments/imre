//! Snapshot each source directory before creating its destination. Finish siblings
//! after a filesystem failure and retain each (source, destination, message) tuple.

use crate::StrOwner;
use std::ffi::OsString;
use std::path::PathBuf;

use super::*;
use crate::objects::exceptions::{
    imre_exception_matches, imre_exception_message, replace_pending, take_pending,
};
use crate::objects::records::{RecordDescriptor, allocate_record};
use crate::objects::words::{OwnedWord, WordKind, Words};

#[path = "tree_callback.rs"]
mod callback;
pub(super) use callback::Callback;
#[path = "tree_ignore.rs"]
mod ignore;
use ignore::{Ignore, Ignored};

pub(super) enum Failure {
    Native(Error),
    Raised(crate::ObjectOwner),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Native(error)
    }
}

pub(super) type TreeResult<T> = std::result::Result<T, Failure>;

impl Failure {
    /// Publish the original raised object, including when a custom copier raised it.
    pub(super) fn finish(result: TreeResult<usize>, sentinel: usize) -> usize {
        match result {
            Ok(word) => word,
            Err(Self::Native(error)) => error.returning(sentinel),
            Err(Self::Raised(error)) => {
                replace_pending(Some(error));
                sentinel
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Options {
    pub(super) links: bool,
    pub(super) stat: bool,
    pub(super) ignore_dangling: bool,
    pub(super) existing: bool,
}

struct Frame {
    src: PathBuf,
    dst: PathBuf,
    entries: std::vec::IntoIter<fs::DirEntry>,
    ignored: Option<Ignored>,
}

fn makedirs(dst: &Name, existing: bool) -> Result<()> {
    if dst.path().as_os_str().as_bytes().contains(&0) {
        return Err(Error::new(
            Kind::ValueError,
            "mkdir: embedded null character in path",
        ));
    }
    match fs::create_dir(dst.path()) {
        Ok(()) => return Ok(()),
        Err(_) if existing && dst.path().is_dir() => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = dst.path().parent().filter(|p| !p.as_os_str().is_empty()) {
                let parent = dst.joined(parent)?;
                fs::create_dir_all(parent.path()).map_err(|e| parent.error(e))?;
            } else {
                return Err(dst.error(e));
            }
        }
        Err(e) => return Err(dst.error(e)),
    }
    match fs::create_dir(dst.path()) {
        Err(_) if existing && dst.path().is_dir() => Ok(()),
        result => result.map_err(|e| dst.error(e)),
    }
}

fn open(src: &Name, dst: &Name, existing: bool, ignore: Option<Ignore>) -> TreeResult<Frame> {
    if src.path().as_os_str().as_bytes().contains(&0) {
        return Err(
            Error::new(Kind::ValueError, "scandir: embedded null character in path").into(),
        );
    }
    // Collect entries and close scandir first: dst may be below src.
    let entries = fs::read_dir(src.path())
        .and_then(|it| it.collect::<io::Result<Vec<_>>>())
        .map_err(|e| src.error(e))?;
    let ignored = ignore.map(|f| f.apply(src, &entries)).transpose()?;
    makedirs(dst, existing)?;
    Ok(Frame {
        src: src.path().into(),
        dst: dst.path().into(),
        entries: entries.into_iter(),
        ignored,
    })
}

fn entry(
    src: &Name,
    dst: &Name,
    item: &fs::DirEntry,
    options: Options,
    callback: Option<Callback>,
    ignore: Option<Ignore>,
) -> TreeResult<Option<Frame>> {
    let kind = item.file_type().map_err(|e| src.error(e))?;
    let directory = if kind.is_symlink() {
        if options.links {
            copy_link(src, dst)?;
            metadata::copystat(src, dst, false)?;
            return Ok(None);
        }
        if options.ignore_dangling {
            let target = fs::read_link(src.path()).map_err(|e| src.error(e))?;
            if !target.exists() {
                return Ok(None);
            }
        }
        match fs::metadata(src.path()) {
            Ok(stat) => stat.is_dir(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(src.error(e).into()),
        }
    } else {
        kind.is_dir()
    };
    if directory {
        return Ok(Some(open(src, dst, options.existing, ignore)?));
    }
    if let Some(callback) = callback {
        callback.call(src, dst)?;
    } else {
        run(if options.stat { 5 } else { 4 }, src, dst, true)?;
    }
    Ok(None)
}

fn path_word(name: &Name) -> usize {
    match name {
        Name::Bytes(b) => crate::BytesOwner::into_raw(crate::BytesOwner::clone(b)) as usize | 1,
        Name::Text(s) | Name::Path(s) => StrOwner::into_raw(StrOwner::clone(s)) as usize,
    }
}

fn record(
    errors: &mut Words,
    layout: &RecordDescriptor,
    src: &Name,
    dst: &Name,
    error: Failure,
) -> TreeResult<()> {
    let message = match error {
        Failure::Native(e) => e.into_os_message()?,
        Failure::Raised(e) => {
            // SAFETY: this owner roots the original exception through both reads.
            unsafe {
                if imre_exception_matches(crate::ObjectOwner::as_ptr(&e), Kind::OSError as u32) == 0
                {
                    return Err(Failure::Raised(e));
                }
                if crate::objects::exceptions::append_tree_entries(&e, errors)? {
                    return Ok(());
                }
                let raw = imre_exception_message(crate::ObjectOwner::as_ptr(&e));
                if let Some(display_error) = take_pending() {
                    return Err(Failure::Raised(display_error));
                }
                StrOwner::from_raw(raw)
            }
        }
    };
    errors
        .values
        .try_reserve(1)
        .map_err(|_| Error::allocation())?;
    // SAFETY: the compiler passes a static tuple descriptor with exactly two
    // tagged str | bytes fields and one nonnullable str. The list owns the record.
    unsafe {
        let tuple = allocate_record(layout);
        if tuple.is_null() {
            return Err(Error::allocation().into());
        }
        tuple.write(path_word(src));
        tuple.add(1).write(path_word(dst));
        tuple
            .add(2)
            .cast::<*const Str>()
            .write(StrOwner::into_raw(message));
        errors.values.push(tuple as usize);
    }
    Ok(())
}

fn copy_tree(
    src: &Name,
    dst: &Name,
    options: Options,
    layout: &RecordDescriptor,
    callback: Option<Callback>,
    ignore: Option<Ignore>,
) -> TreeResult<()> {
    let mut stack = vec![open(src, dst, options.existing, ignore)?];
    let mut errors = Words::new(6);
    while let Some(frame) = stack.last_mut() {
        if let Some(item) = frame.entries.next() {
            let name: OsString = item.file_name();
            if let Some(ignored) = &frame.ignored
                && ignored.contains(&src.joined(Path::new(&name))?)
            {
                continue;
            }
            if matches!(src, Name::Bytes(_)) != matches!(dst, Name::Bytes(_)) {
                return Err(Error::new(
                    Kind::TypeError,
                    "Can't mix strings and bytes in path components",
                )
                .into());
            }
            let from = frame.src.join(&name);
            let to = frame.dst.join(&name);
            let from = src.joined(&from)?;
            let to = dst.joined(&to)?;
            match entry(&from, &to, &item, options, callback, ignore) {
                Ok(Some(frame)) => stack.push(frame),
                Ok(None) => {}
                Err(e) => record(&mut errors, layout, &from, &to, e)?,
            }
        } else {
            let from = src.joined(&frame.src)?;
            let to = dst.joined(&frame.dst)?;
            if let Err(e) = metadata::copystat(&from, &to, true) {
                record(&mut errors, layout, &from, &to, e.into())?;
            }
            stack.pop();
        }
    }
    if !errors.values.is_empty() {
        return Err(Error::tree_errors(errors.into_object()).into());
    }
    Ok(())
}

/// Copy a directory after rename fails. Move always preserves links and has no ignore filter.
pub(super) fn copy_for_move(
    src: &Name,
    dst: &Name,
    options: Options,
    layout: &RecordDescriptor,
    callback: Option<Callback>,
) -> TreeResult<()> {
    copy_tree(src, dst, options, layout, callback, None)
}

/// # Safety
/// Both path handles are rooted and exactly tagged. The layout describes a
/// program-lifetime (str | bytes, str | bytes, str) tuple. A non-null callback
/// accepts the borrowed context and two matching path values; repr identifies its result ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_copytree(
    src: *const (),
    src_tag: u32,
    dst: *const (),
    dst_tag: u32,
    links: u32,
    stat: u32,
    dangling: u32,
    existing: u32,
    layout: *const RecordDescriptor,
    function: *const (),
    context: *const (),
    repr: u32,
    ignore: Option<ignore::Function>,
    ignore_context: *const (),
    ignore_set: u32,
) -> usize {
    // SAFETY: the caller roots both handles and owns the static compiler layout.
    let result = unsafe {
        let src = Name::from_handle(src, src_tag);
        let target = Name::from_handle(dst, dst_tag);
        let options = Options {
            links: links != 0,
            stat: stat != 0,
            ignore_dangling: dangling != 0,
            existing: existing != 0,
        };
        copy_tree(
            &src,
            &target,
            options,
            &*layout,
            Callback::new(function, context, repr),
            ignore.map(|function| Ignore {
                function,
                context: ignore_context,
                set: ignore_set != 0,
            }),
        )
        .map(|()| retain_handle(dst, dst_tag))
    };
    Failure::finish(result, 0)
}
