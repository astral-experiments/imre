//! Installation scheme shared by the Python launcher and its system modules.
use std::io;
use std::path::{Path, PathBuf};

pub const SCHEME_NAMES: &[&str] = &["posix_prefix", "venv"];
pub const PATH_NAMES: &[&str] = &[
    "stdlib",
    "platstdlib",
    "purelib",
    "platlib",
    "include",
    "platinclude",
    "scripts",
    "data",
];

pub struct InterpreterLayout {
    pub executable: PathBuf,
    pub prefix: PathBuf,
    pub version: String,
}

impl InterpreterLayout {
    /// Use the invocation path so a relocated distribution describes its own prefix.
    pub fn new(executable: &Path, python_version: (u8, u8)) -> io::Result<Self> {
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
    pub fn path(&self, scheme: &str, name: &str, expand: bool) -> Option<String> {
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
            assert_eq!(layout.prefix, Path::new("/opt/example"));
        }
    }
}
