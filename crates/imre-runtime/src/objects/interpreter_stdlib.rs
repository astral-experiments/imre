//! Native process and installation information exposed by Python's system modules.

use std::ffi::CStr;
use std::io;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};

use serde_json::Value;

mod structure;
pub(crate) use structure::StructFormat;

pub(crate) const SCHEME_NAMES: &[&str] = &["posix_prefix", "venv"];
pub(crate) const PATH_NAMES: &[&str] = &[
    "stdlib",
    "platstdlib",
    "purelib",
    "platlib",
    "include",
    "platinclude",
    "scripts",
    "data",
];

pub(crate) struct InterpreterLayout {
    pub(crate) executable: PathBuf,
    pub(crate) prefix: PathBuf,
    pub(crate) version: String,
}

impl InterpreterLayout {
    /// Use the invocation path so a relocated distribution describes its own prefix.
    pub(crate) fn new(executable: &Path, python_version: (u8, u8)) -> io::Result<Self> {
        let directory = executable
            .parent()
            .filter(|_| executable.is_absolute() && executable.to_str().is_some())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "expected an absolute UTF-8 executable path",
                )
            })?;
        let prefix = if directory.ends_with("bin") {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        Ok(Self {
            executable: executable.to_owned(),
            prefix: prefix.to_owned(),
            version: format!("{}.{}", python_version.0, python_version.1),
        })
    }

    /// Return the installation destination or its format string for sysconfig.
    pub(crate) fn path(&self, scheme: &str, name: &str, expand: bool) -> Option<String> {
        if !SCHEME_NAMES.contains(&scheme) {
            return None;
        }
        let template = match name {
            "stdlib" | "platstdlib" => "{base}/lib/imre{py_version_short}",
            "purelib" | "platlib" => "{base}/lib/imre{py_version_short}/site-packages",
            "include" | "platinclude" => "{base}/include/imre{py_version_short}",
            "scripts"
                if scheme == "posix_prefix" && self.executable.parent() == Some(&self.prefix) =>
            {
                "{base}"
            }
            "scripts" => "{base}/bin",
            "data" => "{base}",
            _ => return None,
        };
        if !expand {
            return Some(template.to_owned());
        }
        let suffix = template
            .strip_prefix("{base}")
            .unwrap()
            .trim_start_matches('/');
        let path = if suffix.is_empty() {
            self.prefix.clone()
        } else {
            self.prefix
                .join(suffix.replace("{py_version_short}", &self.version))
        };
        path.into_os_string().into_string().ok()
    }

    pub(crate) fn config_var(&self, name: &str) -> Value {
        match name {
            "prefix" | "exec_prefix" | "base" | "platbase" | "installed_base"
            | "installed_platbase" => Value::from(self.prefix.to_str()),
            "py_version_short" => Value::from(self.version.as_str()),
            "implementation" => Value::from("Imre"),
            "implementation_lower" => Value::from("imre"),
            "Py_GIL_DISABLED" | "Py_DEBUG" => Value::from(0),
            "EXT_SUFFIX" => Value::from(".imre.a"),
            _ => Value::Null,
        }
    }
}

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
    // SAFETY: a null buffer and zero length query the required size including the NUL.
    let mut size = unsafe { libc::confstr(key, std::ptr::null_mut(), 0) };
    while size != 0 {
        let mut buffer = vec![0u8; size];
        // SAFETY: buffer has size writable bytes and confstr never writes past it.
        let required = unsafe { libc::confstr(key, buffer.as_mut_ptr().cast(), size) };
        if required > 0 && required <= size {
            buffer.truncate(required - 1);
            return String::from_utf8(buffer)
                .map(Some)
                .map_err(io::Error::other);
        }
        size = required;
    }
    Ok(None)
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
    fn relocates_a_bin_install_and_a_flat_bundle() {
        for (executable, scripts) in [
            ("/opt/example/bin/imre", "/opt/example/bin"),
            ("/opt/example/imre", "/opt/example"),
        ] {
            let layout = InterpreterLayout::new(Path::new(executable), (3, 12)).unwrap();
            assert_eq!(
                layout.path("posix_prefix", "scripts", true).as_deref(),
                Some(scripts)
            );
            assert_eq!(
                layout.path("venv", "scripts", false).as_deref(),
                Some("{base}/bin")
            );
            assert_eq!(
                layout.path("posix_prefix", "platlib", true).as_deref(),
                Some("/opt/example/lib/imre3.12/site-packages")
            );
            assert_eq!(layout.config_var("base"), "/opt/example");
        }
    }

    #[test]
    fn filesystem_decoding_preserves_undecodable_bytes() {
        assert_eq!(
            fsdecode(b"a\xc3\xa9\xe2\x82\xff").collect::<Vec<_>>(),
            [0x61, 0xe9, 0xdce2, 0xdc82, 0xdcff]
        );
    }
}
