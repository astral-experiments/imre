//! File type queries on concrete paths and the cached views exposed by `Path.info`.

use super::super::{Object, Storage, StorageCell, Str, object, os};
use std::cell::OnceCell;
use std::fs::{FileType, Metadata};
use std::os::unix::fs::FileTypeExt;
use std::rc::Rc;

/// One path's data, independent of the owner so a saved view can outlive the path.
pub(in crate::objects) struct Info {
    name: Rc<Str>,
    origin: Origin,
    follow: OnceCell<Option<Box<Metadata>>>,
    nofollow: OnceCell<Option<Box<Metadata>>>,
}

pub(super) enum Origin {
    Path,
    // The scan can succeed even if a file type isn't available for this entry.
    Entry(Option<FileType>),
}

impl Info {
    pub(super) fn object(name: Rc<Str>, origin: Origin) -> Rc<Object> {
        Rc::new(Object(StorageCell::new(Storage::PathInfo(Box::new(
            Self {
                name,
                origin,
                follow: OnceCell::new(),
                nofollow: OnceCell::new(),
            },
        )))))
    }

    fn stat(&self, follow: bool) -> Option<&Metadata> {
        let cell = if follow { &self.follow } else { &self.nofollow };
        if let Some(previous) = cell.get() {
            return previous.as_deref();
        }
        let result = os::metadata(self.name.as_str(), follow).ok();
        if matches!(self.origin, Origin::Entry(_)) && result.is_none() {
            // DirEntry.stat caches a success; a later call can see a newly created target.
            return None;
        }
        cell.get_or_init(|| result.map(Box::new)).as_deref()
    }

    fn query(&self, kind: u32, follow: bool) -> bool {
        if let Origin::Entry(entry) = self.origin {
            if kind == 0 && !follow {
                return true;
            }
            if kind != 0
                && let Some(ty) = entry
                && (!follow || !ty.is_symlink())
            {
                return has_type(ty, kind);
            }
        }
        self.stat(follow)
            .is_some_and(|stat| has_type(stat.file_type(), kind))
    }
}

fn has_type(ty: FileType, kind: u32) -> bool {
    match kind {
        0 => true,
        1 => ty.is_dir(),
        2 => ty.is_file(),
        3 => ty.is_symlink(),
        4 => ty.is_fifo(),
        5 => ty.is_socket(),
        6 => ty.is_block_device(),
        7 => ty.is_char_device(),
        _ => unreachable!("unknown file type query"),
    }
}

/// Return a shared view, creating it without accessing the filesystem on first use.
///
/// # Safety
/// The caller roots an exact concrete Path for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_info(path: *const Object) -> *const Object {
    // SAFETY: only the owned cached view is transferred; no storage borrow escapes.
    unsafe {
        object(path).0.with(|s| {
            let Storage::Path { info, .. } = s else {
                unreachable!()
            };
            Rc::into_raw(
                info.get_or_init(|| Info::object(Rc::clone(s.path_text()), Origin::Path))
                    .clone(),
            )
        })
    }
}

/// Query a concrete Path's current type or consult a Path.info view's own cache.
///
/// # Safety
/// The caller roots a concrete Path or PathInfo and supplies a supported type query.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_query(path: *const Object, kind: u32, follow: u32) -> u32 {
    // SAFETY: there are no callbacks and only the boolean result leaves the borrow.
    unsafe {
        object(path).0.with(|s| {
            let follow = follow != 0;
            u32::from(match s {
                Storage::Path { .. } => os::metadata(s.path(), follow)
                    .is_ok_and(|stat| has_type(stat.file_type(), kind)),
                Storage::PathInfo(info) => info.query(kind, follow),
                _ => unreachable!(),
            })
        })
    }
}

/// The POSIX info view's repr is independent of its pathname and cached values.
#[unsafe(no_mangle)]
pub extern "C" fn imre_path_info_repr(_info: *const Object) -> *const Str {
    Rc::into_raw(Rc::new(Str::from("<PosixPath.info>")))
}
