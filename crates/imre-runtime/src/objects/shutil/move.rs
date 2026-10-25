//! Move by rename first. Copy and remove the source only after a failed rename
//! and a successful fallback; preserve symbolic links even when their targets are gone.

use crate::StrOwner;
use rustix::fs::{Access, AtFlags, CWD, accessat};
use std::path::PathBuf;

use super::*;
use crate::objects::posixpath::normalize;
use crate::objects::records::RecordDescriptor;
use crate::objects::shutil::rmtree;
use tree::{Callback, Failure, TreeResult};

fn destinsrc(src: &Name, dst: &Name) -> Result<bool> {
    fn absolute(name: &Name) -> Result<Vec<u8>> {
        let p = name.path();
        let cwd = if p.is_absolute() {
            None
        } else {
            Some(std::env::current_dir().map_err(errno_error)?)
        };
        let mut normalized = normalize(
            p.as_os_str().as_bytes(),
            cwd.as_deref().map(|p| p.as_os_str().as_bytes()),
        )
        .map_err(|_| Error::allocation())?;
        if !normalized.ends_with(b"/") {
            normalized.push(b'/');
        }
        Ok(normalized)
    }
    let source = absolute(src)?;
    let destination = absolute(dst)?;
    if matches!(src, Name::Bytes(_)) || matches!(dst, Name::Bytes(_)) {
        return Err(Error::new(
            Kind::TypeError,
            "endswith first arg must be bytes or a tuple of bytes, not str",
        ));
    }
    Ok(destination.starts_with(&source))
}

fn rename(src: &Name, dst: &Name) -> Result<()> {
    fs::rename(src.path(), dst.path())
        .map_err(|e| errno_with_filenames(e, Some(src.filename()), Some(dst.filename())))
}

fn reject_nul(name: &Name, message: &'static str) -> Result<()> {
    if name.path().as_os_str().as_bytes().contains(&0) {
        return Err(Error::new(Kind::ValueError, message));
    }
    Ok(())
}

enum Destination {
    None,
    Original,
    Joined(PathBuf),
}

fn move_path(
    src: &Name,
    dst: &Name,
    stat: bool,
    layout: &RecordDescriptor,
    callback: Option<Callback>,
    handles: (*const (), *const ()),
    original_src: Option<crate::ObjectOwner>,
) -> TreeResult<Destination> {
    let mut joined = None;
    if let Ok(directory) = fs::metadata(dst.path())
        && directory.is_dir()
    {
        // _samefile stats the source before joining a name to this directory.
        reject_nul(src, "stat: embedded null character in path")?;
        if fs::metadata(src.path())
            .is_ok_and(|s| s.dev() == directory.dev() && s.ino() == directory.ino())
            && !src.path().is_symlink()
        {
            rename(src, dst)?;
            return Ok(Destination::None);
        }
        // _basename strips trailing separators using the string os.path.sep.
        if matches!(src, Name::Bytes(_)) {
            return Err(Error::new(
                Kind::TypeError,
                "a bytes-like object is required, not 'str'",
            )
            .into());
        }
        if matches!(dst, Name::Bytes(_)) {
            return Err(Error::new(
                Kind::TypeError,
                "Can't mix strings and bytes in path components",
            )
            .into());
        }
        let name = src
            .path()
            .to_str()
            .unwrap()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap();
        let path = dst.path().join(name);
        if path.exists() {
            return Err(Error::new(
                Kind::ShutilError,
                format!("Destination path '{}' already exists", path.display()),
            )
            .into());
        }
        joined = Some(path);
    }
    let target;
    let real_dst = if let Some(path) = &joined {
        target = dst.joined(path)?;
        &target
    } else {
        dst
    };
    // rename rejects invalid arguments before the OSError fallback can run.
    reject_nul(src, "rename: embedded null character in src")?;
    reject_nul(real_dst, "rename: embedded null character in dst")?;
    if fs::rename(src.path(), real_dst.path()).is_err() {
        if src.path().is_symlink() {
            copy_link(src, real_dst)?;
            fs::remove_file(src.path()).map_err(|e| src.error(e))?;
        } else if src.path().is_dir() {
            if destinsrc(src, dst)? {
                return Err(Error::new(
                    Kind::ShutilError,
                    format!(
                        "Cannot move a directory '{}' into itself '{}'.",
                        src.display(),
                        dst.display()
                    ),
                )
                .into());
            }
            let metadata = fs::metadata(src.path()).map_err(|e| src.error(e))?;
            #[cfg(target_os = "macos")]
            let immutable = matches!(
                std::os::macos::fs::MetadataExt::st_flags(&metadata),
                libc::UF_IMMUTABLE | libc::SF_IMMUTABLE
            );
            #[cfg(not(target_os = "macos"))]
            let immutable = {
                let _ = metadata;
                false
            };
            let unwritable = !immutable
                && accessat(CWD, src.path(), Access::WRITE_OK, AtFlags::empty()).is_err()
                && fs::read_dir(src.path())
                    .map_err(|e| src.error(e))?
                    .try_fold(false, |_, entry| entry.map(|_| true))
                    .map_err(|e| src.error(e))?;
            if immutable || unwritable && cfg!(target_os = "macos") {
                return Err(Error::new(
                    Kind::PermissionError,
                    format!(
                        "Cannot move the non-empty directory '{}': Lacking write permission to '{}'.",
                        src.display(),
                        src.display()
                    ),
                ).into());
            }
            tree::copy_for_move(
                src,
                real_dst,
                tree::Options {
                    links: true,
                    stat,
                    ignore_dangling: false,
                    existing: false,
                },
                layout,
                callback,
            )?;
            rmtree::remove_root(src.path(), original_src)?;
        } else {
            if let Some(copy) = callback {
                copy.call_paths(
                    src,
                    real_dst,
                    Some(handles.0),
                    joined.is_none().then_some(handles.1),
                )?;
            } else {
                run(if stat { 5 } else { 4 }, src, real_dst, true)?;
            }
            fs::remove_file(src.path()).map_err(|e| src.error(e))?;
        }
    }
    Ok(joined.map_or(Destination::Original, Destination::Joined))
}

/// # Safety
/// Both path handles are rooted and have the indicated concrete types. The
/// compiler supplies the same immutable error tuple descriptor as for copytree.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_move(
    src: *const (),
    src_tag: u32,
    dst: *const (),
    dst_tag: u32,
    stat: u32,
    layout: *const RecordDescriptor,
    function: *const (),
    context: *const (),
    repr: u32,
    tagged: u32,
) -> usize {
    // SAFETY: the caller keeps all three inputs live through the operation.
    let result = unsafe {
        move_path(
            &Name::from_handle(src, src_tag),
            &Name::from_handle(dst, dst_tag),
            stat != 0,
            &*layout,
            Callback::new(function, context, repr).map(|c| c.tagged(tagged)),
            (src, dst),
            (src_tag == 2).then(|| crate::ObjectOwner::acquire_word(src as usize)),
        )
        .and_then(|out| {
            Ok(match out {
                Destination::None => 0,
                Destination::Original => {
                    retain_handle(dst, dst_tag) | if dst_tag == 2 { 6 } else { 0 }
                }
                Destination::Joined(path) => StrOwner::into_raw(StrOwner::new(
                    crate::objects::paths::path_utf8(path.into_os_string())?,
                )) as usize,
            })
        })
    };
    Failure::finish(result, 0)
}

/// # Safety
/// The compiler roots and exactly tags both str, bytes or Path handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_destinsrc(
    src: *const (),
    src_tag: u32,
    dst: *const (),
    dst_tag: u32,
) -> u32 {
    // SAFETY: each tag describes its rooted input.
    let result = unsafe {
        destinsrc(
            &Name::from_handle(src, src_tag),
            &Name::from_handle(dst, dst_tag),
        )
    };
    finish(result.map(u32::from), 0)
}
