use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use imre_compiler::CompileOptions;

use super::build_output::Verbosity;
use super::{Allocator, USAGE};

const DEFAULT_NATIVE_OPT_LEVEL: &str = match option_env!("IMRE_DEFAULT_NATIVE_OPT_LEVEL") {
    None => "3",
    Some(level) if matches!(level.as_bytes(), [b'0'..=b'3']) => level,
    Some(_) => panic!("IMRE_DEFAULT_NATIVE_OPT_LEVEL expects 0, 1, 2, or 3"),
};

fn site_path_files(directory: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                !name.starts_with('.') && name.ends_with(".pth")
            })
        })
        .map(|entry| entry.path())
}

/// Collect possible compiler inputs. Only site.addpackage knows which lines will run.
fn venv_path_inputs(directory: PathBuf) -> Option<(PathBuf, Vec<PathBuf>)> {
    let mut files: Vec<_> = site_path_files(&directory).collect();
    if files.is_empty() {
        return None;
    }
    files.sort_unstable();
    let mut roots = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        for line in text.split(|c: char| imre_ast::strings::is_line_boundary(u32::from(c))) {
            if line.starts_with('#') || line.starts_with("import ") || line.starts_with("import\t") {
                continue;
            }
            let path = line.trim_end_matches(imre_ast::strings::python_whitespace);
            if path.is_empty() {
                continue;
            }
            let root = ruff_db::system::SystemPath::absolute(path, directory.to_str()?)
                .as_std_path()
                .to_owned();
            if root.exists() && !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    Some((directory, roots))
}

#[derive(Default)]
pub(super) struct BuildOptions {
    pub source_path: Option<PathBuf>,
    pub help: bool,
    pub watch: bool,
    pub no_cache: bool,
    pub whole_program: bool,
    pub verbosity: Verbosity,
    pub output: Option<PathBuf>,
    pub runtime: Option<PathBuf>,
    pub llvm_path: Option<PathBuf>,
    pub module_root: Option<PathBuf>,
    pub lto_linker: Option<PathBuf>,
    pub target: Option<OsString>,
    pub allocator: Option<Allocator>,
    pub clang: Option<OsString>,
    pub optimization: OsString,
    pub compile: CompileOptions,
    pub link: LinkOptions,
}

#[derive(Default)]
pub(super) struct LinkOptions {
    pub keep_dead_code: bool,
    pub relro: Relro,
    pub args: Vec<OsString>,
    pub verbose: bool,
}

#[derive(Default)]
pub(super) enum Relro {
    #[default]
    Full,
    Partial,
    Off,
}

impl BuildOptions {
    fn initial() -> Self {
        Self {
            clang: cfg!(target_os = "macos").then(|| OsString::from("clang")),
            optimization: DEFAULT_NATIVE_OPT_LEVEL.into(),
            ..Self::default()
        }
    }

    pub fn parse(
        command: &OsStr,
        args: &mut impl Iterator<Item = OsString>,
    ) -> Result<Self, String> {
        let mut options = Self::initial();
        if command != "check" {
            options.read(command, &mut environment_flags()?.into_iter(), false)?;
        }
        options.read(command, args, true)?;
        options.add_pythonpath();
        Ok(options)
    }

    /// Code generation and linker flags stay separate from Python's environment and arguments.
    pub(super) fn for_python(
        invocation: crate::python_command::Invocation,
    ) -> Result<Self, String> {
        use crate::python_command::Invocation;
        let flags = match &invocation {
            Invocation::Command(command) => &command.flags,
            Invocation::Module { flags, .. } | Invocation::Script { flags, .. } => flags,
        };
        let mut options = Self::initial();
        if !flags.isolated {
            options.read(
                OsStr::new("run"),
                &mut environment_flags()?.into_iter(),
                false,
            )?;
        }
        if !flags.ignore_environment {
            options.add_pythonpath();
        }
        if options.module_root.is_some() {
            return Err(
                "--module-root takes a file entry; command strings use the interpreter search path"
                    .into(),
            );
        }
        options.verbosity = Verbosity::Quiet;
        match invocation {
            Invocation::Command(command) => {
                options.source_path = Some("-c".into());
                options.compile.command = Some(command);
            }
            Invocation::Module {
                name,
                flags,
                executable,
            } => {
                options.source_path = Some("-m".into());
                options.compile.module_entry = Some(name);
                options.compile.script_startup = Some((flags, executable));
            }
            Invocation::Script {
                path,
                flags,
                executable,
            } => {
                if path.is_dir() || imre_compiler::is_zip_path(&path) {
                    options.compile.module_entry = Some("__main__".into());
                    options.compile.path_entry = true;
                }
                options.source_path = Some(path);
                options.compile.script_startup = Some((flags, executable));
            }
        }
        let (flags, executable) = options.compile.python_startup().expect("Python invocation");
        let version = imre_compiler::PYTHON_VERSION;
        let layout = imre_ast::layout::InterpreterLayout::new(
            executable.as_ref(),
            (version.major, version.minor),
        )
        .map_err(|e| e.to_string())?;
        let stdlib = layout.stdlib_dir();
        let stdlib = stdlib.is_dir().then_some(stdlib);
        let venv = layout.venv_system_site;
        let user_site = if !flags.no_site
            && imre_ast::layout::InterpreterLayout::user_site_enabled(flags.no_user_site, venv)
                == Some(true)
        {
            imre_ast::layout::InterpreterLayout::user_base()
                .map(|base| layout.user_site(&base))
                .filter(|p| p.is_dir())
                .map(|p| std::path::absolute(p).map_err(|e| e.to_string()))
                .transpose()?
        } else {
            None
        };
        let mut prefix_sites = layout.site_packages(!flags.no_site);
        let mut site_dirs = Vec::new();
        let venv_site = venv.and_then(|_| prefix_sites.next());
        site_dirs.extend(venv_site.iter().cloned());
        site_dirs.extend(user_site.clone());
        site_dirs.extend(prefix_sites);
        site_dirs.retain(|path| path.is_dir());
        // site runs before Python prepends the script directory or command cwd.
        let has_hook = |root: &PathBuf| {
            root.join("sitecustomize.py").is_file() || root.join("sitecustomize").is_dir()
        };
        let stdlib_sitecustomize = !flags.no_site && stdlib.as_ref().is_some_and(has_hook);
        let sitecustomize = !flags.no_site
            && options
                .compile
                .extension_paths
                .iter()
                .chain(&site_dirs)
                .any(has_hook);
        let venv_site_pth = venv_site
            .filter(|_| !flags.no_site)
            .and_then(venv_path_inputs);
        // Process an enabled user site between the two venv prefix visits, including
        // when the saved program finds path files that were absent at compile time.
        options.compile.user_site_pth = user_site.filter(|dir| {
            sitecustomize || venv_site_pth.is_some() || site_path_files(dir).next().is_some()
        });
        options.compile.sitecustomize =
            sitecustomize || options.compile.user_site_pth.is_some() || venv_site_pth.is_some();
        options.compile.venv_site_pth = venv_site_pth;
        options.compile.stdlib_sitecustomize = stdlib_sitecustomize;
        #[cfg(unix)]
        if layout.stdlib_archive.is_file() {
            options.compile.installed_stdlib_archive = Some(layout.stdlib_archive.clone());
        } else {
            options.compile.stdlib_fallback = Some(super::build_cache::stdlib_archive_path()?);
        }
        options.compile.stdlib_dir = stdlib;
        options.compile.site_dirs = site_dirs;
        Ok(options)
    }

    /// Append Python import roots after the explicit compiler search paths.
    fn add_pythonpath(&mut self) {
        let Some(paths) = env::var_os("PYTHONPATH").filter(|value| !value.is_empty()) else {
            return;
        };
        let start = self.compile.extension_paths.len();
        for path in env::split_paths(&paths) {
            let path = if path.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                path
            };
            if !path.is_dir() {
                if !imre_compiler::is_zip_path(&path) {
                    continue;
                }
                self.compile
                    .python_archive_indices
                    .push(self.compile.extension_paths.len());
            }
            self.compile.extension_paths.push(path);
        }
        self.compile.pythonpath = start..self.compile.extension_paths.len();
    }

    fn read(
        &mut self,
        command: &OsStr,
        args: &mut impl Iterator<Item = OsString>,
        command_line: bool,
    ) -> Result<(), String> {
        while let Some(flag) = args.next() {
            if command_line && !flag.as_encoded_bytes().starts_with(b"-") {
                if self.source_path.replace(flag.into()).is_some() {
                    return Err(USAGE.to_owned());
                }
                continue;
            }
            if command_line && (flag == "--help" || flag == "-h") {
                self.help = true;
                break;
            }
            if command == "check" && (flag == "--watch" || flag == "-w") {
                self.watch = true;
                continue;
            }
            if command == "check"
                && flag != "--module-root"
                && flag != "--extension-path"
                && flag != "--include-module"
                && flag != "--reachable-only"
                && flag != "--entry-profile"
            {
                return Err(USAGE.to_owned());
            }
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{} requires a value", flag.display()))
            };
            match flag.to_str() {
                Some("--") if command_line && command == "run" => break,
                Some("-q" | "--quiet") => self.verbosity = Verbosity::Quiet,
                Some("-v" | "--verbose") => self.verbosity = Verbosity::Verbose,
                Some("--no-cache") => self.no_cache = true,
                Some("--whole-program") => self.whole_program = true,
                Some("--entry-profile") => {
                    self.compile.entry_profile = Some(value()?.into());
                    self.compile.reachable_only = true;
                }
                Some("--reachable-only") => self.compile.reachable_only = true,
                Some("--module-root") => self.module_root = Some(value()?.into()),
                Some("--extension-path") => {
                    let path: PathBuf = value()?.into();
                    if imre_compiler::is_zip_path(&path) {
                        self.compile
                            .python_archive_indices
                            .push(self.compile.extension_paths.len());
                    }
                    self.compile.extension_paths.push(path);
                }
                Some("--include-module") => {
                    let name = value()?
                        .into_string()
                        .map_err(|_| "--include-module requires a UTF-8 module name")?;
                    self.compile.include_modules.insert(name);
                }
                Some("-o") if command == "build" => self.output = Some(value()?.into()),
                Some("--runtime") => self.runtime = Some(value()?.into()),
                Some("--emit-llvm") => self.llvm_path = Some(value()?.into()),
                Some("--clang") => {
                    let driver = value()?;
                    self.clang = if driver.is_empty() {
                        Self::initial().clang
                    } else {
                        Some(driver)
                    };
                }
                Some("--lto-linker") => self.lto_linker = Some(value()?.into()),
                Some("--target") => self.target = Some(value()?),
                Some("--allocator") => {
                    self.allocator = Some(match value()?.to_str() {
                        Some("system") => Allocator::System,
                        Some("jemalloc") => Allocator::Jemalloc,
                        _ => return Err("--allocator expects system or jemalloc".to_owned()),
                    });
                }
                Some("--opt-level") => {
                    self.optimization = value()?;
                    if !matches!(self.optimization.to_str(), Some("0" | "1" | "2" | "3")) {
                        return Err("--opt-level expects 0, 1, 2, or 3".to_owned());
                    }
                }
                Some("--sanitize") => self.compile.address_sanitizer = true,
                Some("--gc-sections") => self.link.keep_dead_code = false,
                Some("--no-gc-sections") => self.link.keep_dead_code = true,
                Some("--verbose-link") => self.link.verbose = true,
                Some("-C") => {
                    let value = value()?;
                    self.link
                        .codegen(value.to_str().ok_or("-C expects a UTF-8 option")?)?;
                }
                Some(arg) if arg.starts_with("-C") => self.link.codegen(&arg[2..])?,
                _ => return Err(format!("Unknown option: {}\n{USAGE}", flag.display())),
            }
        }
        Ok(())
    }
}

impl LinkOptions {
    fn codegen(&mut self, option: &str) -> Result<(), String> {
        match option.split_once('=') {
            Some(("link-arg", arg)) => self.args.push(arg.into()),
            Some(("relro-level", level)) => {
                self.relro = match level {
                    "full" => Relro::Full,
                    "partial" => Relro::Partial,
                    "off" => Relro::Off,
                    _ => return Err("-C relro-level expects full, partial, or off".to_owned()),
                };
            }
            _ => return Err(format!("Unknown codegen option: -C {option}")),
        }
        Ok(())
    }
}

fn environment_flags() -> Result<Vec<OsString>, String> {
    for name in ["IMRE_ENCODED_FLAGS", "IMREFLAGS"] {
        if let Some(value) = env::var_os(name) {
            let text = value
                .to_str()
                .ok_or_else(|| format!("{name} must be UTF-8"))?;
            return Ok(if text.is_empty() {
                Vec::new()
            } else if name == "IMRE_ENCODED_FLAGS" {
                text.split('\x1f').map(OsString::from).collect()
            } else {
                text.split_whitespace().map(OsString::from).collect()
            });
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_is_a_check_option() {
        for command in ["check", "build", "run"] {
            for flag in ["--watch", "-w"] {
                let mut options = BuildOptions::default();
                let mut args = ["main.py", flag].map(OsString::from).into_iter();
                let result = options.read(OsStr::new(command), &mut args, true);
                assert_eq!(result.is_ok(), command == "check");
                assert_eq!(options.watch, command == "check");
            }
        }
    }

    #[test]
    fn source_is_positional_only_on_the_command_line() {
        let mut options = BuildOptions::default();
        let command = OsStr::new("build");
        let args = || ["-q", "main.py", "-v"].map(OsString::from).into_iter();
        let error = options.read(command, &mut args(), false).unwrap_err();
        assert!(error.contains("Unknown option: main.py"));
        assert!(options.source_path.is_none());
        options.read(command, &mut args(), true).unwrap();
        assert_eq!(options.source_path, Some(PathBuf::from("main.py")));
        assert!(options.verbosity == Verbosity::Verbose);
    }
}
