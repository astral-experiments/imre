//! Extract members in order. Resolve both a member and its link target against the extraction root.
use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, utimensat};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::{Error, Kind, Result, errno_error, mkdir_parent, path_error};
use crate::objects::{exceptions::string_repr, posixpath};

fn quoted(path: &Path) -> super::Str {
    string_repr(&path.to_string_lossy())
}

fn timestamp<R: Read>(
    member: &mut tar::Entry<'_, R>,
    invalid: &impl Fn() -> Error,
) -> Result<Timespec> {
    if let Some(pax) = member.pax_extensions().map_err(|_| invalid())? {
        for ext in pax {
            let ext = ext.map_err(|_| invalid())?;
            if ext.key_bytes() == b"mtime" {
                let seconds: f64 = ext
                    .value()
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|v: &f64| v.is_finite())
                    .ok_or_else(invalid)?;
                let whole = seconds.floor();
                let fraction = ((seconds - whole) * 1e9).round() as i64;
                return Ok(Timespec {
                    tv_sec: (whole as i64).saturating_add(fraction / 1_000_000_000),
                    tv_nsec: fraction % 1_000_000_000,
                });
            }
        }
    }
    Ok(Timespec {
        tv_sec: member
            .header()
            .mtime()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?,
        tv_nsec: 0,
    })
}

fn attributes(path: &Path, mode: Option<u32>, time: Timespec) -> Result<()> {
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|e| path_error(e, path))?;
    }
    let times = Timestamps {
        last_access: time,
        last_modification: time,
    };
    utimensat(CWD, path, &times, AtFlags::empty()).map_err(|e| path_error(e.into(), path))
}

// realpath(strict=ALLOW_MISSING): even a dangling symlink is followed before
// interpreting later '..' components. Other OS errors are never treated as missing.
fn resolve(path: &Path) -> Result<PathBuf> {
    let mut at = if path.is_absolute() {
        PathBuf::from("/")
    } else {
        std::env::current_dir().map_err(errno_error)?
    };
    let parts = |p: &Path| -> Vec<OsString> {
        p.as_os_str()
            .as_bytes()
            .split(|b| *b == b'/')
            .filter(|p| !p.is_empty())
            .map(|p| OsStr::from_bytes(p).to_owned())
            .collect()
    };
    let mut rest: VecDeque<_> = parts(path).into();
    let mut links = 0;
    while let Some(part) = rest.pop_front() {
        if part == "." {
            continue;
        }
        if part == ".." {
            at.pop();
            continue;
        }
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(st) if st.is_symlink() => {
                links += 1;
                if links > 40 {
                    return Err(path_error(io::Error::from_raw_os_error(libc::ELOOP), &at));
                }
                let target = fs::read_link(&at).map_err(|e| path_error(e, &at))?;
                if target.is_absolute() {
                    at = PathBuf::from("/");
                } else {
                    at.pop();
                }
                for p in parts(&target).into_iter().rev() {
                    rest.push_front(p);
                }
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(path_error(e, &at)),
            _ => {}
        }
    }
    Ok(at)
}

pub(super) fn extract(
    input: Box<dyn Read>,
    dest: &Path,
    filter: Option<&str>,
    invalid: impl Fn() -> Error,
) -> Result<()> {
    let mut input = BufReader::new(input);
    let mut first = [0u8; 512];
    input.read_exact(&mut first).map_err(|_| invalid())?;
    let mut archive = tar::Archive::new(first.as_slice().chain(input));
    let mut entries = archive.entries().map_err(|_| invalid())?;
    let mut entry = entries.next().transpose().map_err(|_| invalid())?;
    let filter = filter.unwrap_or("data");
    let trusted = match filter {
        "fully_trusted" => true,
        "data" | "tar" => false,
        name => {
            return Err(Error::new(
                Kind::ValueError,
                format!("filter {} not found", string_repr(name)),
            ));
        }
    };
    let data = filter == "data";
    if entry.is_none() {
        return Ok(());
    }
    let root = resolve(dest)?;
    let mut directories = Vec::new();
    while let Some(mut member) = entry {
        let name = member.path().map_err(|_| invalid())?.into_owned();
        let ty = member.header().entry_type();
        let mtime = timestamp(&mut member, &invalid)?;
        let target = if trusted {
            dest.join(&name)
        } else {
            let name_bytes = name.as_os_str().as_bytes();
            let stripped = &name_bytes[name_bytes
                .iter()
                .position(|b| *b != b'/')
                .unwrap_or(name_bytes.len())..];
            let target = root.join(OsStr::from_bytes(stripped));
            let resolved = resolve(&target)?;
            if !resolved.starts_with(&root) {
                return Err(Error::new(
                    Kind::TarOutsideDestinationError,
                    format!(
                        "{} would be extracted to {}, which is outside the destination",
                        quoted(&name),
                        quoted(&resolved)
                    ),
                ));
            }
            target
        };
        let mode = member.header().mode().map_err(|_| invalid())?;
        let mut mode = if trusted { mode } else { mode & 0o755 };
        if data && (ty.is_file() || ty.is_hard_link() || ty.is_gnu_sparse() || ty.is_contiguous()) {
            if mode & 0o100 == 0 {
                mode &= !0o111;
            }
            mode |= 0o600;
        }
        if ty.is_dir() {
            fs::create_dir_all(&target).map_err(|e| path_error(e, &target))?;
            directories.push((target, (!data).then_some(mode), mtime));
        } else if ty.is_symlink() || ty.is_hard_link() {
            let link = member
                .link_name()
                .map_err(|_| invalid())?
                .ok_or_else(&invalid)?
                .into_owned();
            if data && link.is_absolute() {
                return Err(Error::new(
                    Kind::TarAbsoluteLinkError,
                    format!("{} is a link to an absolute path", quoted(&name)),
                ));
            }
            let link = if data {
                PathBuf::from(OsString::from_vec(
                    posixpath::normalize(link.as_os_str().as_bytes(), None)
                        .map_err(|_| Error::allocation())?,
                ))
            } else {
                link
            };
            let link_target = if ty.is_symlink() {
                target.parent().unwrap_or(&root).join(&link)
            } else {
                dest.join(&link)
            };
            if data {
                let resolved = resolve(&link_target)?;
                if !resolved.starts_with(&root) {
                    return Err(Error::new(
                        Kind::TarLinkOutsideDestinationError,
                        format!(
                            "{} would link to {}, which is outside the destination",
                            quoted(&name),
                            quoted(&resolved)
                        ),
                    ));
                }
            }
            mkdir_parent(&target)?;
            let create = || {
                if ty.is_symlink() {
                    symlink(&link, &target)
                } else {
                    fs::hard_link(&link_target, &target)
                }
            };
            if let Err(e) = create() {
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(path_error(e, &target));
                }
                fs::remove_file(&target)
                    .and_then(|()| create())
                    .map_err(|e| path_error(e, &target))?;
            }
            if ty.is_hard_link() {
                attributes(&target, Some(mode), mtime)?;
            }
        } else if ty.is_file() || ty.is_contiguous() || ty.is_gnu_sparse() {
            mkdir_parent(&target)?;
            let mut out = File::create(&target).map_err(|e| path_error(e, &target))?;
            io::copy(&mut member, &mut out).map_err(errno_error)?;
            attributes(&target, Some(mode), mtime)?;
        } else if data {
            return Err(Error::new(
                Kind::TarSpecialFileError,
                format!("{} is a special file", quoted(&name)),
            ));
        } else {
            mkdir_parent(&target)?;
            member.unpack(&target).map_err(|e| path_error(e, &target))?;
        }
        entry = entries.next().transpose().map_err(|_| invalid())?;
    }
    directories.sort_by(|a, b| b.0.cmp(&a.0));
    for (path, mode, mtime) in directories {
        attributes(&path, mode, mtime)?;
    }
    Ok(())
}
