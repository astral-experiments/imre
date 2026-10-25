//! ZIP writing uses the archive root as a path prefix, never the process cwd.
//! Extraction streams members through Imre's existing ZIP reader and CRC checks.
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{Error, Kind, Result, errno_error, mkdir_parent, normalized, path_error};
use crate::objects::zipfile::Archive;
use ::zip::{CompressionMethod, DateTime, ZipWriter, result::ZipError, write::SimpleFileOptions};

fn zip_error(e: ZipError) -> Error {
    match e {
        ZipError::Io(e) => errno_error(e),
        other => Error::new(Kind::BadZipFile, other.to_string()),
    }
}

fn options(st: &Metadata) -> Result<SimpleFileOptions> {
    let mut local = std::mem::MaybeUninit::uninit();
    // SAFETY: localtime_r initializes the caller's tm and does not retain either pointer.
    let time = unsafe {
        if libc::localtime_r(&(st.mtime() as libc::time_t), local.as_mut_ptr()).is_null() {
            return Err(errno_error(io::Error::last_os_error()));
        }
        local.assume_init()
    };
    let date = DateTime::from_date_and_time(
        (time.tm_year + 1900) as u16,
        (time.tm_mon + 1) as u8,
        time.tm_mday as u8,
        time.tm_hour as u8,
        time.tm_min as u8,
        time.tm_sec as u8,
    )
    .map_err(|_| {
        Error::new(
            Kind::ValueError,
            "ZIP does not support timestamps outside 1980–2107",
        )
    })?;
    Ok(SimpleFileOptions::default()
        .unix_permissions(st.mode())
        .last_modified_time(date)
        .large_file(st.size().saturating_mul(105) / 100 > i32::MAX as u64))
}

fn add(
    zip: &mut ZipWriter<File>,
    source: &Path,
    name: &Path,
    st: &Metadata,
    output: (u64, u64),
) -> Result<()> {
    let name = name
        .to_str()
        .ok_or_else(|| Error::new(Kind::UnicodeError, "ZIP filename is not valid UTF-8"))?;
    let name = name.trim_start_matches('/');
    let options = options(st)?;
    if st.is_dir() {
        zip.add_directory(name, options.compression_method(CompressionMethod::Stored))
            .map_err(zip_error)?;
    } else {
        zip.start_file(
            name,
            options.compression_method(CompressionMethod::Deflated),
        )
        .map_err(zip_error)?;
        let mut input = File::open(source).map_err(|e| path_error(e, source))?;
        // If a ZIP is created inside the archived tree, never chase bytes we are writing.
        if (st.dev(), st.ino()) == output {
            io::copy(&mut input.take(st.len()), zip)
        } else {
            io::copy(&mut input, zip)
        }
        .map_err(errno_error)?;
    }
    Ok(())
}

pub(super) fn create(file: File, source: &Path, base_dir: &str) -> Result<()> {
    let st = file.metadata().map_err(errno_error)?;
    let output = (st.dev(), st.ino());
    let mut zip = ZipWriter::new(file);
    let base = normalized(base_dir, None)?;
    let member = if base == "." {
        PathBuf::new()
    } else {
        PathBuf::from(base.as_str())
    };
    if !member.as_os_str().is_empty() {
        let st = fs::metadata(source).map_err(|e| path_error(e, source))?;
        add(&mut zip, source, &member, &st, output)?;
    }
    let mut stack = vec![(source.to_owned(), member)];
    while let Some((folder, prefix)) = stack.pop() {
        // os.walk without an onerror hook ignores inaccessible directory scans.
        let Ok(scan) = fs::read_dir(&folder) else {
            continue;
        };
        let mut directories = Vec::new();
        let mut files = Vec::new();
        for item in scan.flatten() {
            let path = item.path();
            let Ok(st) = fs::metadata(&path) else {
                continue;
            };
            let name = prefix.join(item.file_name());
            if st.is_dir() {
                directories.push((path, name, st));
            } else if st.is_file() {
                files.push((path, name, st));
            }
        }
        stack.extend(
            directories
                .iter()
                .rev()
                .filter(|(p, _, _)| !p.is_symlink())
                .map(|(p, n, _)| (p.clone(), n.clone())),
        );
        directories.sort_by(|a, b| a.1.cmp(&b.1));
        for (p, n, st) in directories.into_iter().chain(files) {
            add(&mut zip, &p, &n, &st, output)?;
        }
    }
    zip.finish().map_err(zip_error)?;
    Ok(())
}

pub(super) fn unpack(path: &Path, dest: &Path) -> Result<()> {
    let invalid = || {
        Error::new(
            Kind::ShutilReadError,
            format!("{} is not a zip file", path.display()),
        )
    };
    let file = File::open(path).map_err(|_| invalid())?;
    let archive = Archive::for_extraction(file)?.ok_or_else(invalid)?;
    archive.for_filenames(|name| {
        // Match ZipFile's _ignore_invalid_names POSIX policy. A backslash or colon
        // is an ordinary filename byte here; '..' only matters as a whole component.
        if name.starts_with('/') || name.split('/').any(|part| part == "..") {
            return Ok(());
        }
        let rel: PathBuf = name
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        let directory = name.ends_with('/');
        if rel.as_os_str().is_empty() && !directory {
            return Ok(());
        }
        let target = dest.join(rel);
        mkdir_parent(&target)?;
        if directory {
            fs::create_dir_all(&target).map_err(|e| path_error(e, &target))?;
        } else {
            let mut member = archive.member_by_name(name)?;
            let mut out = File::create(&target).map_err(|e| path_error(e, &target))?;
            loop {
                let chunk = member.read(Some(256 * 1024))?;
                if chunk.is_empty() {
                    break;
                }
                out.write_all(&chunk).map_err(errno_error)?;
            }
        }
        Ok(())
    })
}
