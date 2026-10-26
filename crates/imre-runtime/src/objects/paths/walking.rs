//! A walk suspends with no open directories. Top-down callers retain the live
//! list that decides which directories the next pull visits.

use crate::StrOwner;
use std::cell::RefCell;
use std::fs;
use std::path::Path;

use super::{
    Error, Kind, Object, PathText, Result, Str, check_directory_path, directory_entries, object,
    path_with_text,
};
use crate::objects::{OwnedWord, Words, itertools, records, words::WordKind};

enum Work {
    Scan(Str),
    Descend(Str, crate::ObjectOwner),
    Yield(Row),
}

struct Row {
    path: Str,
    dirs: crate::ObjectOwner,
    files: crate::ObjectOwner,
}

pub(in crate::objects) struct Walker {
    stack: RefCell<Vec<Work>>,
    top_down: bool,
    follow: bool,
    dot_root: bool,
    layout: &'static records::RecordDescriptor,
}

impl Walker {
    pub(in crate::objects) fn next(&self) -> Result<Option<OwnedWord>> {
        let mut stack = self
            .stack
            .try_borrow_mut()
            .map_err(|_| Error::new(Kind::ValueError, "generator already executing"))?;
        let result = self.advance(&mut stack);
        if result.is_err() {
            stack.clear();
        }
        result
    }

    fn advance(&self, stack: &mut Vec<Work>) -> Result<Option<OwnedWord>> {
        while let Some(work) = stack.pop() {
            match work {
                Work::Yield(row) => return self.row(row).map(Some),
                Work::Descend(path, dirs) => {
                    // Read the actual list at resumption; a caller can prune, reorder
                    // or append names (including an absolute name) while suspended.
                    // SAFETY: the suspension owns this list; no callback or
                    // mutation of the list occurs while reading its names.
                    unsafe {
                        dirs.0.with(|storage| {
                            let names = storage.sequence_storage();
                            stack
                                .try_reserve(names.values.len())
                                .map_err(|_| Error::allocation())?;
                            let start = stack.len();
                            for (_, name) in names.strings() {
                                let child = Path::new(path.as_str()).join(name.as_str());
                                stack.push(Work::Scan(Str::from(child.to_str().unwrap())));
                            }
                            stack[start..].reverse();
                            Ok(())
                        })
                    }?;
                }
                Work::Scan(path) => {
                    check_directory_path(&path)?;
                    let entries = directory_entries(&path, |entry| {
                        let dir = entry.file_type().is_ok_and(|kind| {
                            kind.is_dir()
                                || (self.follow
                                    && kind.is_symlink()
                                    && fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()))
                        });
                        (entry.file_name(), dir)
                    });
                    let Ok(entries) = entries else { continue };
                    let mut dirs = Words::new(3);
                    let mut files = Words::new(3);
                    for (name, is_dir) in entries {
                        let name = name.into_string().map_err(|_| {
                            Error::new(
                                Kind::UnicodeDecodeError,
                                "directory entry is not valid UTF-8",
                            )
                        })?;
                        let names = if is_dir { &mut dirs } else { &mut files };
                        names
                            .values
                            .try_reserve(1)
                            .map_err(|_| Error::allocation())?;
                        names
                            .values
                            .push(StrOwner::into_raw(StrOwner::new(Str::from(name))) as usize);
                    }
                    let dirs = dirs.into_object();
                    let visit = Work::Descend(path.clone(), dirs.clone());
                    let row = Row {
                        path,
                        dirs,
                        files: files.into_object(),
                    };
                    stack.try_reserve(2).map_err(|_| Error::allocation())?;
                    if self.top_down {
                        stack.push(visit);
                        return self.row(row).map(Some);
                    }
                    stack.push(Work::Yield(row));
                    stack.push(visit);
                }
            }
        }
        Ok(None)
    }

    fn row(&self, row: Row) -> Result<OwnedWord> {
        // Path.walk strips the first two characters of os.walk's raw spelling for
        // a '.' root. Normally these are './'; callers can supply absolute names.
        let path = if self.dot_root {
            let start = row
                .path
                .char_indices()
                .nth(2)
                .map_or(row.path.len(), |(i, _)| i);
            let tail = &row.path[start..];
            Str::from(if tail.is_empty() { "." } else { tail })
        } else {
            row.path
        };
        // SAFETY: the generated static descriptor describes exactly
        // (Path, list[str], list[str]), all three slots independently owned.
        unsafe {
            let fields = records::allocate_record(self.layout);
            if fields.is_null() {
                return Err(Error::allocation());
            }
            fields.write(path_with_text(PathText::from_parsed_string(path)) as usize);
            fields
                .add(1)
                .write(crate::ObjectOwner::into_raw(row.dirs) as usize);
            fields
                .add(2)
                .write(crate::ObjectOwner::into_raw(row.files) as usize);
            Ok(OwnedWord::from_owned(fields as usize, WordKind::new(6)))
        }
    }
}

/// # Safety
/// The caller roots the path and supplies the compiler's immutable walk tuple
/// descriptor. The iterator only reads the filesystem when pulled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_walk(
    path: *const Object,
    top_down: u32,
    follow: u32,
    layout: &'static records::RecordDescriptor,
) -> *const Object {
    // SAFETY: the borrowed spelling is copied while its owner is rooted.
    let path = unsafe { object(path).0.with(|storage| Str::from(storage.path())) };
    let dot_root = path == ".";
    crate::ObjectOwner::into_raw(itertools::adapter(itertools::Adapter::PathWalk(Walker {
        stack: RefCell::new(vec![Work::Scan(path)]),
        top_down: top_down != 0,
        follow: follow != 0,
        dot_root,
        layout,
    })))
}
