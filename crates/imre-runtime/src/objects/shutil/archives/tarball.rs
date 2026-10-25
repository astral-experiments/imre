//! Stream tar headers and data without changing cwd or following directory symlinks.
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use super::{Error, Format, Kind, Result, errno_error, path_error};
use tar::{Builder, EntryType, Header};

/// Each reentrant NSS lookup fills caller-owned storage. Unknown names leave the
/// original owner/group unchanged, as shutil's _get_uid/_get_gid do.
fn account(user: bool, id: u32, name: Option<&str>) -> Option<(u32, String)> {
    let name = name.map(CString::new).transpose().ok()?;
    let mut buf = vec![0u8; 4096];
    macro_rules! lookup {
        ($entry:ty, $by_name:path, $by_id:path, $id:ident, $name:ident) => {{
            let mut entry = std::mem::MaybeUninit::<$entry>::uninit();
            let mut found = std::ptr::null_mut();
            // SAFETY: caller storage and the C name are valid for each complete libc lookup.
            let status = unsafe {
                match &name {
                    Some(s) => $by_name(
                        s.as_ptr(),
                        entry.as_mut_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        &mut found,
                    ),
                    None => $by_id(
                        id,
                        entry.as_mut_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        &mut found,
                    ),
                }
            };
            if status == libc::ERANGE {
                buf.resize(buf.len().checked_mul(2)?, 0);
                continue;
            }
            if status != 0 || found.is_null() {
                return None;
            }
            // SAFETY: a successful non-null result initialized both the record and its name.
            unsafe {
                let entry = entry.assume_init();
                return Some((
                    entry.$id,
                    CStr::from_ptr(entry.$name).to_string_lossy().into_owned(),
                ));
            }
        }};
    }
    loop {
        if user {
            lookup!(
                libc::passwd,
                libc::getpwnam_r,
                libc::getpwuid_r,
                pw_uid,
                pw_name
            );
        } else {
            lookup!(
                libc::group,
                libc::getgrnam_r,
                libc::getgrgid_r,
                gr_gid,
                gr_name
            );
        }
    }
}

struct Contents<'a> {
    source: &'a Path,
    base: &'a str,
    output: (u64, u64),
    owner: Option<(u32, String)>,
    group: Option<(u32, String)>,
}

impl Contents<'_> {
    fn write<W: Write>(&self, output: W) -> Result<W> {
        let mut tar = Builder::new(output);
        let base = self.base.trim_start_matches('/').trim_end_matches('/');
        let mut stack = vec![(
            self.source.to_owned(),
            PathBuf::from(if base.is_empty() { "." } else { base }),
        )];
        let mut inodes = HashMap::new();
        let mut users = HashMap::new();
        let mut groups = HashMap::new();
        while let Some((path, member)) = stack.pop() {
            let st = fs::symlink_metadata(&path).map_err(|e| path_error(e, &path))?;
            let inode = (st.dev(), st.ino());
            if inode == self.output || st.file_type().is_socket() {
                continue;
            }
            let mut hdr = Header::new_ustar();
            hdr.set_metadata(&st);
            let name = member.as_os_str().as_bytes();
            let mut link = None;
            if st.is_symlink() {
                link = Some(fs::read_link(&path).map_err(|e| path_error(e, &path))?);
            } else if st.is_file() && st.nlink() > 1 {
                if let Some(first) = inodes.get(&inode) {
                    hdr.set_entry_type(EntryType::Link);
                    link = Some(PathBuf::clone(first));
                } else {
                    inodes.insert(inode, member.clone());
                }
            }
            let owner = self.owner.as_ref().or_else(|| {
                users
                    .entry(st.uid())
                    .or_insert_with(|| account(true, st.uid(), None))
                    .as_ref()
            });
            let group = self.group.as_ref().or_else(|| {
                groups
                    .entry(st.gid())
                    .or_insert_with(|| account(false, st.gid(), None))
                    .as_ref()
            });
            let mut pax = Vec::new();
            if let Some((id, n)) = owner {
                hdr.set_uid((*id).into());
                if hdr.set_username(n).is_err() {
                    pax.push(("uname", n.as_bytes()));
                }
            }
            if let Some((id, n)) = group {
                hdr.set_gid((*id).into());
                if hdr.set_groupname(n).is_err() {
                    pax.push(("gname", n.as_bytes()));
                }
            }
            // Retain literal './' prefixes and long POSIX paths without normalizing the archive name.
            let raw = &mut hdr.as_old_mut().name;
            let kept = raw.len().min(name.len());
            raw[..kept].copy_from_slice(&name[..kept]);
            if kept != name.len() {
                pax.push(("path", name));
            }
            if let Some(link) = &link {
                let target = link.as_os_str().as_bytes();
                if hdr.set_link_name_literal(target).is_err() {
                    pax.push(("linkpath", target));
                }
                hdr.set_size(0);
            }
            let ns = i128::from(st.mtime()) * 1_000_000_000 + i128::from(st.mtime_nsec());
            let stamp = format!(
                "{}{}.{:09}",
                if ns < 0 { "-" } else { "" },
                ns.unsigned_abs() / 1_000_000_000,
                ns.unsigned_abs() % 1_000_000_000
            );
            if st.mtime_nsec() != 0 || ns < 0 {
                pax.push(("mtime", stamp.as_bytes()));
            }
            tar.append_pax_extensions(pax).map_err(errno_error)?;
            hdr.set_cksum();
            if hdr.entry_type().is_file() {
                let file = File::open(&path).map_err(|e| path_error(e, &path))?;
                tar.append(&hdr, file.take(st.len())).map_err(errno_error)?;
            } else {
                tar.append(&hdr, io::empty()).map_err(errno_error)?;
            }
            if st.is_dir() {
                let mut children: Vec<_> = fs::read_dir(&path)
                    .and_then(|d| d.map(|e| e.map(|e| e.file_name())).collect())
                    .map_err(|e| path_error(e, &path))?;
                children.sort_unstable();
                stack.extend(
                    children
                        .into_iter()
                        .rev()
                        .map(|n| (path.join(&n), member.join(n))),
                );
            }
        }
        tar.into_inner().map_err(errno_error)
    }
}

pub(super) fn create(
    file: File,
    format: Format,
    source: &Path,
    base: &str,
    owner: Option<&str>,
    group: Option<&str>,
) -> Result<()> {
    let st = file.metadata().map_err(errno_error)?;
    let tree = Contents {
        source,
        base,
        output: (st.dev(), st.ino()),
        owner: owner.and_then(|s| account(true, 0, Some(s))),
        group: group.and_then(|s| account(false, 0, Some(s))),
    };
    // Explicitly finish the codec so write failures on the trailer propagate to the caller.
    match format {
        Format::Tar => tree.write(file)?,
        Format::Gzip => tree
            .write(flate2::write::GzEncoder::new(
                file,
                flate2::Compression::default(),
            ))?
            .finish()
            .map_err(errno_error)?,
        Format::Bzip => tree
            .write(bzip2::write::BzEncoder::new(
                file,
                bzip2::Compression::best(),
            ))?
            .finish()
            .map_err(errno_error)?,
        Format::Xz => tree
            .write(
                lzma_rust2::XzWriter::new(file, lzma_rust2::XzOptions::with_preset(6))
                    .map_err(errno_error)?,
            )?
            .finish()
            .map_err(errno_error)?,
        Format::Zstd => tree
            .write(zstd::stream::Encoder::new(file, 3).map_err(errno_error)?)?
            .finish()
            .map_err(errno_error)?,
        Format::Zip => unreachable!(),
    };
    Ok(())
}

pub(super) fn unpack(path: &Path, destination: &Path, filter: Option<&str>) -> Result<()> {
    let file = File::open(path).map_err(|e| path_error(e, path))?;
    let mut file = BufReader::new(file);
    let magic = file.fill_buf().map_err(errno_error)?;
    let invalid = || {
        Error::new(
            Kind::ShutilReadError,
            format!(
                "{} is not a compressed or uncompressed tar file",
                path.display()
            ),
        )
    };
    let source: Box<dyn Read> = if magic.starts_with(b"\x1f\x8b") {
        Box::new(flate2::read::MultiGzDecoder::new(file))
    } else if magic.starts_with(b"BZh") {
        Box::new(bzip2::read::MultiBzDecoder::new(file))
    } else if magic.starts_with(b"\xfd7zXZ\0") {
        Box::new(lzma_rust2::XzReader::new(file, true))
    } else if magic.starts_with(b"\x28\xb5\x2f\xfd") {
        Box::new(zstd::stream::read::Decoder::with_buffer(file).map_err(|_| invalid())?)
    } else {
        Box::new(file)
    };
    super::tar_extract::extract(source, destination, filter, invalid)
}
