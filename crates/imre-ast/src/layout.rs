//! Installation scheme shared by the Python launcher and its system modules.
use std::io;
use std::path::{Path, PathBuf};

const STDLIB: &str = "{installed_base}/lib/imre{py_version_short}";
const SITE_PACKAGES: &str = "{base}/lib/imre{py_version_short}/site-packages";

pub const SCHEME_NAMES: &[&str] = &["posix_prefix", "posix_user", "venv"];
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

#[derive(Clone)]
pub struct InterpreterLayout {
    pub executable: PathBuf,
    pub prefix: PathBuf,
    pub base_executable: PathBuf,
    pub base_prefix: PathBuf,
    pub stdlib_archive: PathBuf,
    pub venv_system_site: Option<bool>,
    pub version: String,
}

fn config_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let (key, value) = line.split_once('=')?;
    key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
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
        let installation_prefix = |directory: &Path| {
            if directory.ends_with("bin") {
                directory.parent().unwrap_or(directory).to_owned()
            } else {
                directory.to_owned()
            }
        };
        let mut prefix = installation_prefix(directory);
        let mut base_prefix = prefix.clone();
        let mut base_executable = executable.to_owned();
        let mut stdlib_archive = executable.with_file_name("imre-stdlib.zip");
        let mut venv_system_site = None;
        if let Some(cfg) = Self::venv_config(executable) {
            prefix = cfg.parent().expect("configuration directory").to_owned();
            let contents = std::fs::read_to_string(&cfg);
            let home = contents
                .as_deref()
                .unwrap_or("")
                .lines()
                .find_map(|line| config_value(line, "home"))
                .map(Path::new)
                .filter(|home| home.is_absolute());
            let physical = executable
                .canonicalize()
                .unwrap_or_else(|_| executable.to_owned());
            base_prefix =
                installation_prefix(home.or_else(|| physical.parent()).unwrap_or(directory));
            let base_binary = if let Some(home) = home
                && physical == executable
            {
                home.join(executable.file_name().expect("launcher name"))
            } else {
                physical
            };
            stdlib_archive = base_binary.with_file_name("imre-stdlib.zip");
            if home.is_some() {
                base_executable = base_binary;
            }
            // getpath prefers the parent configuration, while site checks beside the executable first.
            let local_cfg = directory.join("pyvenv.cfg");
            let site_config = if local_cfg != cfg && local_cfg.is_file() {
                std::fs::read_to_string(local_cfg)
            } else {
                contents
            };
            venv_system_site = Some(site_config.is_ok_and(|text| {
                text.lines()
                    .rev()
                    .find_map(|line| config_value(line, "include-system-site-packages"))
                    .is_none_or(|value| value.eq_ignore_ascii_case("true"))
            }));
        }
        Ok(Self {
            executable: executable.to_owned(),
            prefix,
            base_prefix,
            base_executable,
            stdlib_archive,
            venv_system_site,
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

    /// Locate the configuration relative to the invocation name before following venv symlinks.
    pub fn venv_config(executable: &Path) -> Option<PathBuf> {
        let bin = executable.parent()?;
        [bin.parent()?.join("pyvenv.cfg"), bin.join("pyvenv.cfg")]
            .into_iter()
            .find(|cfg| cfg.is_file())
    }

    /// A venv shares the base interpreter's standard library, including with -S.
    pub fn stdlib_dir(&self) -> PathBuf {
        self.expand_path(&self.base_prefix, STDLIB)
    }

    /// Ordered installation sites; the venv participates even when the base cannot.
    pub fn site_packages(&self, site_initialized: bool) -> impl Iterator<Item = PathBuf> + '_ {
        std::iter::once(&self.prefix)
            .chain(
                (site_initialized
                    && self.venv_system_site == Some(true)
                    && self.base_prefix != self.prefix)
                    .then_some(&self.base_prefix),
            )
            .map(|prefix| self.expand_path(prefix, SITE_PACKAGES))
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
        let template = match (scheme, name) {
            ("posix_user", "stdlib" | "platstdlib") => "{userbase}/lib/python{py_version_short}",
            ("posix_user", "purelib" | "platlib") => {
                "{userbase}/lib/python{py_version_short}/site-packages"
            }
            ("posix_user", "include") => "{userbase}/include/python{py_version_short}",
            ("posix_user", "scripts") => "{userbase}/bin",
            ("posix_user", "data") => "{userbase}",
            ("posix_prefix" | "venv", "stdlib") => STDLIB,
            ("posix_prefix" | "venv", "platstdlib") => "{platbase}/lib/imre{py_version_short}",
            ("posix_prefix" | "venv", "purelib") => SITE_PACKAGES,
            ("posix_prefix" | "venv", "platlib") => {
                "{platbase}/lib/imre{py_version_short}/site-packages"
            }
            ("posix_prefix" | "venv", "include") => "{base}/include/imre{py_version_short}",
            ("posix_prefix" | "venv", "platinclude") => "{platbase}/include/imre{py_version_short}",
            ("posix_prefix", "scripts") if self.executable.parent() == Some(&self.prefix) => {
                "{base}"
            }
            ("posix_prefix" | "venv", "scripts") => "{base}/bin",
            ("posix_prefix" | "venv", "data") => "{base}",
            _ => return None,
        };
        if !expand {
            return Some(template.to_owned());
        }
        let userbase;
        let prefix = if scheme == "posix_user" {
            userbase = Self::user_base()?;
            &userbase
        } else if template == STDLIB {
            &self.base_prefix
        } else {
            &self.prefix
        };
        self.expand_path(prefix, template)
            .into_os_string()
            .into_string()
            .ok()
    }

    fn expand_path(&self, prefix: &Path, template: &str) -> PathBuf {
        let suffix = template.split_once('}').unwrap().1.trim_start_matches('/');
        if suffix.is_empty() {
            prefix.to_owned()
        } else {
            prefix.join(suffix.replace("{py_version_short}", &self.version))
        }
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
