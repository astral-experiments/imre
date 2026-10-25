//! File type queries on concrete paths and the cached views exposed by `Path.info`.

use super::super::{Object, Storage, StorageCell, Str, object, os};
use crate::StrOwner;
use rustix::fs::FileType;
use std::cell::OnceCell;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;

/// One path's data, independent of the owner so a saved view can outlive the path.
pub(in crate::objects) struct Info {
    name: StrOwner,
    entry: Option<FileType>,
    follow: OnceCell<Option<Box<Metadata>>>,
    nofollow: OnceCell<Option<Box<Metadata>>>,
}

impl Info {
    pub(super) fn object(name: StrOwner, entry: Option<FileType>) -> crate::ObjectOwner {
        crate::ObjectOwner::new(Object(StorageCell::new(Storage::PathInfo(Box::new(
            Self {
                name,
                entry,
                follow: OnceCell::new(),
                nofollow: OnceCell::new(),
            },
        )))))
    }

    fn lookup(&self, follow: bool) -> Option<&Metadata> {
        let cell = if follow { &self.follow } else { &self.nofollow };
        if let Some(previous) = cell.get() {
            return previous.as_deref();
        }
        let result = os::metadata(self.name.as_str(), follow).ok();
        if self.entry.is_some() && result.is_none() {
            // DirEntry.stat caches a success; a later call can see a newly created target.
            return None;
        }
        cell.get_or_init(|| result.map(Box::new)).as_deref()
    }

    fn stat(&self, follow: bool) -> Option<&Metadata> {
        if follow && let Some(entry) = self.entry {
            // DirEntry.stat follows only an entry identified as a symlink. An
            // unknown d_type first asks lstat; successful lstat is shared with
            // the no-follow query, even when the pathname has since changed.
            let follows_link = entry == FileType::Symlink
                || entry == FileType::Unknown
                    && self
                        .lookup(false)
                        .is_some_and(|st| st.file_type().is_symlink());
            return self.lookup(follows_link);
        }
        self.lookup(follow)
    }

    fn query(&self, kind: u32, follow: bool) -> bool {
        if let Some(entry) = self.entry {
            if kind == 0 && !follow {
                return true;
            }
            if kind != 0 && entry != FileType::Unknown && (!follow || entry != FileType::Symlink) {
                return has_type(entry, kind);
            }
        }
        self.stat(follow)
            .is_some_and(|stat| has_type(FileType::from_raw_mode(stat.mode() as _), kind))
    }
}

fn has_type(ty: FileType, kind: u32) -> bool {
    match kind {
        0 => true,
        1 => ty == FileType::Directory,
        2 => ty == FileType::RegularFile,
        3 => ty == FileType::Symlink,
        4 => ty == FileType::Fifo,
        5 => ty == FileType::Socket,
        6 => ty == FileType::BlockDevice,
        7 => ty == FileType::CharacterDevice,
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
        object(path).0.with_mut(|s| {
            let Storage::Path { text, .. } = s else {
                unreachable!()
            };
            crate::ObjectOwner::into_raw(text.info())
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
                    .is_ok_and(|stat| has_type(FileType::from_raw_mode(stat.mode() as _), kind)),
                Storage::PathInfo(info) => info.query(kind, follow),
                _ => unreachable!(),
            })
        })
    }
}

/// The POSIX info view's repr is independent of its pathname and cached values.
#[unsafe(no_mangle)]
pub extern "C" fn imre_path_info_repr(_info: *const Object) -> *const Str {
    StrOwner::into_raw(StrOwner::new(Str::from("<PosixPath.info>")))
}
