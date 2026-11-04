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

    /// CPython's per-user base, independent of the interpreter's installation prefix.
    pub fn user_base() -> Option<PathBuf> {
        if let Some(base) = std::env::var_os("PYTHONUSERBASE").filter(|s| !s.is_empty()) {
            return Some(PathBuf::from(base));
        }
        if cfg!(any(
            target_arch = "wasm32",
            target_os = "ios",
            target_os = "tvos",
            target_os = "watchos",
            target_os = "vxworks"
        )) {
            return None;
        }
        let home = || std::env::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        Some(if cfg!(windows) {
            std::env::var_os("APPDATA")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(home)
                .join("Python")
        } else {
            home().join(".local")
        })
    }

    /// Use the Python user scheme, also used by pip --user, for this language version.
    pub fn user_site(&self, base: &Path) -> PathBuf {
        let mut path = base.as_os_str().to_owned();
        if cfg!(windows) {
            path.push(format!(
                "\\Python{}\\site-packages",
                self.version.replace('.', "")
            ));
        } else {
            path.push(format!("/lib/python{}/site-packages", self.version));
        }
        path.into()
    }

    /// A virtual environment's own site directory takes precedence over the user's.
    pub fn venv_system_site(&self) -> Option<bool> {
        let bin = self.executable.parent()?;
        let cfg = [bin.join("pyvenv.cfg"), bin.parent()?.join("pyvenv.cfg")]
            .into_iter()
            .find(|p| p.is_file())?;
        // A venv with an unreadable configuration must not opt in to user packages.
        let contents = match std::fs::read_to_string(cfg) {
            Ok(contents) => contents,
            Err(_) => return Some(false),
        };
        Some(
            contents
                .lines()
                .filter_map(|line| line.split_once('='))
                .filter(|(key, _)| {
                    key.trim()
                        .eq_ignore_ascii_case("include-system-site-packages")
                })
                .map(|(_, value)| value.trim().eq_ignore_ascii_case("true"))
                .next_back()
                .unwrap_or(true),
        )
    }

    /// None signals a uid/gid mismatch; False means the user or their venv disabled this site.
    pub fn user_site_enabled(no_user_site: bool, venv_system_site: Option<bool>) -> Option<bool> {
        if no_user_site || venv_system_site == Some(false) {
            return Some(false);
        }
        // SAFETY: these calls read the process credentials and take no arguments.
        #[cfg(unix)]
        if unsafe { libc::getuid() != libc::geteuid() || libc::getgid() != libc::getegid() } {
            return None;
        }
        Some(true)
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
