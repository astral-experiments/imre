use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use imre_compiler::CompileOptions;

use super::build_output::Verbosity;
use super::{Allocator, USAGE};

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
            optimization: "3".into(),
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
        Ok(options)
    }

    /// Code generation and linker flags stay separate from Python's environment and arguments.
    pub(super) fn for_python(
        invocation: crate::python_command::Invocation,
    ) -> Result<Self, String> {
        use crate::python_command::Invocation;
        let flags = match &invocation {
            Invocation::Command(command) => command.flags,
            Invocation::Script { flags, .. } => *flags,
        };
        let mut options = Self::initial();
        if !flags.isolated {
            options.read(
                OsStr::new("run"),
                &mut environment_flags()?.into_iter(),
                false,
            )?;
        }
        if !flags.isolated
            && let Some(paths) = env::var_os("PYTHONPATH")
        {
            for path in env::split_paths(&paths) {
                let path = if path.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    path
                };
                if path.is_dir() {
                    options.compile.extension_paths.push(path);
                }
            }
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
            Invocation::Script {
                path,
                flags,
                executable,
            } => {
                options.source_path = Some(path);
                options.compile.script_startup = Some((flags, executable));
            }
        }
        Ok(options)
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
                Some("--extension-path") => self.compile.extension_paths.push(value()?.into()),
                Some("-o") if command == "build" => self.output = Some(value()?.into()),
                Some("--runtime") => self.runtime = Some(value()?.into()),
                Some("--emit-llvm") => self.llvm_path = Some(value()?.into()),
                Some("--clang") => self.clang = Some(value()?),
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
