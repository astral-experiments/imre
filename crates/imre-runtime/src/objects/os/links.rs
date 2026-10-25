//! POSIX pathname operations retain their original str or bytes for OSError.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::super::exceptions::{Error, Filename, Kind, Result};
use super::super::files::errno_with_filenames;

fn checked<'a>(name: &'a Filename, operation: &str, parameter: &str) -> Result<&'a Path> {
    let spelling = match name {
        Filename::Str(s) => OsStr::new(s.as_str()),
        Filename::Bytes(b) => OsStr::from_bytes(b),
    };
    if spelling.as_bytes().contains(&0) {
        return Err(Error::new(
            Kind::ValueError,
            format!("{operation}: embedded null character in {parameter}"),
        ));
    }
    Ok(Path::new(spelling))
}

pub(in crate::objects) fn readlink(name: Filename) -> Result<OsString> {
    std::fs::read_link(checked(&name, "readlink", "path")?)
        .map(|target| target.into_os_string())
        .map_err(|error| errno_with_filenames(error, Some(name), None))
}

pub(in crate::objects) fn link(source: Filename, destination: Filename, kind: u32) -> Result<()> {
    let operation = if kind == 2 { "link" } else { "symlink" };
    let src = checked(&source, operation, "src")?;
    let dst = checked(&destination, operation, "dst")?;
    match kind {
        0 => std::os::unix::fs::symlink(src, dst),
        1 => return Ok(()),
        2 => {
            use rustix::fs::{AtFlags, CWD, linkat};
            linkat(CWD, src, CWD, dst, AtFlags::SYMLINK_FOLLOW).map_err(std::io::Error::from)
        }
        _ => unreachable!("unknown native link operation"),
    }
    .map_err(|error| errno_with_filenames(error, Some(source), Some(destination)))
}
