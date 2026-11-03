//! Native process and installation information exposed by Python's system modules.

use std::ffi::CStr;
use std::io;
use std::mem::MaybeUninit;

mod structure;
pub(crate) use structure::{StructFormat, UnpackedField};

pub(crate) use imre_ast::layout::{InterpreterLayout, PATH_NAMES, SCHEME_NAMES};

pub(crate) struct Uname {
    pub(crate) system: String,
    pub(crate) node: String,
    pub(crate) release: String,
    pub(crate) version: String,
    pub(crate) machine: String,
}

pub(crate) fn uname() -> io::Result<Uname> {
    let mut output = MaybeUninit::uninit();
    // SAFETY: uname initializes every utsname field before returning success.
    if unsafe { libc::uname(output.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: uname succeeded and returned null-terminated field arrays.
    let output = unsafe { output.assume_init() };
    let field = |bytes: &[libc::c_char]| {
        // SAFETY: All callers pass one of the null-terminated utsname fields above.
        let bytes = unsafe { CStr::from_ptr(bytes.as_ptr()) };
        bytes.to_str().map(str::to_owned).map_err(io::Error::other)
    };
    Ok(Uname {
        system: field(&output.sysname)?,
        node: field(&output.nodename)?,
        release: field(&output.release)?,
        version: field(&output.version)?,
        machine: field(&output.machine)?,
    })
}

/// The product version reported by platform.mac_ver differs from the Darwin kernel release.
pub(crate) fn mac_ver() -> io::Result<(String, String)> {
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("/usr/bin/sw_vers")
            .env("SYSTEM_VERSION_COMPAT", "0")
            .arg("-productVersion")
            .output()?;
        if !version.status.success() {
            return Err(io::Error::other(
                "sw_vers could not read the macOS product version",
            ));
        }
        let version = std::str::from_utf8(&version.stdout).map_err(io::Error::other)?;
        Ok((version.trim().to_owned(), uname()?.machine))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok((String::new(), String::new()))
    }
}

pub(crate) fn sysconfig_platform() -> io::Result<String> {
    #[cfg(target_os = "macos")]
    {
        let (version, machine) = mac_ver()?;
        let major = version
            .split('.')
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .ok_or_else(|| io::Error::other("invalid macOS product version"))?;
        Ok(format!("macosx-{major}.0-{machine}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    }
}

pub(crate) fn confstr(name: &str) -> io::Result<Option<String>> {
    let key = match name {
        "CS_PATH" => libc::_CS_PATH,
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        "CS_GNU_LIBC_VERSION" => libc::_CS_GNU_LIBC_VERSION,
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        "CS_GNU_LIBPTHREAD_VERSION" => libc::_CS_GNU_LIBPTHREAD_VERSION,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unrecognized configuration name",
            ));
        }
    };
    confstr_bytes(key)?
        .map(String::from_utf8)
        .transpose()
        .map_err(io::Error::other)
}

/// Read a POSIX configuration string, where zero with errno clear means undefined.
pub(crate) fn confstr_bytes(key: libc::c_int) -> io::Result<Option<Vec<u8>>> {
    let mut small = [0; 255];
    let mut large = Vec::new();
    let mut buffer: &mut [u8] = &mut small;
    let size = loop {
        // SAFETY: errno is thread-local and confstr borrows the writable buffer
        // only until return. Its length includes the terminating NUL.
        let len = unsafe {
            #[cfg(target_os = "linux")]
            {
                *libc::__errno_location() = 0;
            }
            #[cfg(target_os = "macos")]
            {
                *libc::__error() = 0;
            }
            libc::confstr(key, buffer.as_mut_ptr().cast(), buffer.len())
        };
        if len == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(0) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        if len <= buffer.len() {
            break len - 1;
        }
        large
            .try_reserve_exact(len - large.len())
            .map_err(|_| io::ErrorKind::OutOfMemory)?;
        large.resize(len, 0);
        buffer = &mut large;
    };
    if large.is_empty() {
        large
            .try_reserve_exact(size)
            .map_err(|_| io::ErrorKind::OutOfMemory)?;
        large.extend_from_slice(&small[..size]);
    } else {
        large.truncate(size);
    }
    Ok(Some(large))
}

/// Decode a POSIX UTF-8 filename with Python's surrogateescape policy.
pub(crate) fn fsdecode(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes.utf8_chunks().flat_map(|chunk| {
        chunk
            .valid()
            .chars()
            .map(u32::from)
            .chain(chunk.invalid().iter().map(|&byte| 0xdc00 + u32::from(byte)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_decoding_preserves_undecodable_bytes() {
        assert_eq!(
            fsdecode(b"a\xc3\xa9\xe2\x82\xff").collect::<Vec<_>>(),
            [0x61, 0xe9, 0xdce2, 0xdc82, 0xdcff]
        );
    }
}
