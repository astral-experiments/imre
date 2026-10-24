use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use relar_compiler::{CompileOptions, PythonInvocation, PythonSource};

use super::build_output::Verbosity;
use super::{Allocator, USAGE};

#[derive(Default)]
pub(super) struct BuildOptions {
    pub source_path: Option<PathBuf>,
    pub help: bool,
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
    fn defaults() -> Self {
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
        let mut options = Self::defaults();
        if command != "check" {
            options.read(command, &mut environment_flags()?.into_iter(), false)?;
        }
        options.read(command, args, true)?;
        Ok(options)
    }

    pub fn for_python(invocation: PythonInvocation) -> Result<Self, String> {
        let mut options = Self {
            verbosity: Verbosity::Quiet,
            source_path: Some(invocation.argv0().to_owned()),
            ..Self::defaults()
        };
        if !invocation.isolated {
            options.read(
                OsStr::new("run"),
                &mut environment_flags()?.into_iter(),
                false,
            )?;
            if let Some(paths) = env::var_os("PYTHONPATH") {
                for mut path in env::split_paths(&paths) {
                    if path.as_os_str().is_empty() {
                        path.push(".");
                    }
                    if path.is_dir() {
                        options.compile.extension_paths.push(path);
                    }
                }
            }
        }
        options.compile.python = Some(invocation);
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
            if command == "check" && flag != "--module-root" && flag != "--extension-path" {
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

/// Stop parsing as soon as the command string or script is known. Every remaining
/// argument, including an option or a literal "--", belongs to the Python program.
pub(super) fn python_invocation(
    mut arg: OsString,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<PythonInvocation, String> {
    let mut isolated = false;
    let mut dont_write_bytecode = false;
    let source = 'options: loop {
        if arg == "--" {
            break PythonSource::Script(args.next().ok_or("expected script after --")?.into());
        }
        if !arg.as_encoded_bytes().starts_with(b"-") {
            break PythonSource::Script(arg.into());
        }
        if arg == "-" {
            return Err("reading a Python program from standard input is unsupported".into());
        }
        let mut flags = arg.to_str().ok_or("interpreter options must be UTF-8")?[1..].chars();
        while let Some(flag) = flags.next() {
            match flag {
                'I' => isolated = true,
                'B' => dont_write_bytecode = true,
                'c' => {
                    let code = if flags.as_str().is_empty() {
                        args.next()
                            .ok_or("-c requires a command string")?
                            .into_string()
                            .map_err(|_| "-c requires UTF-8 source")?
                    } else {
                        flags.as_str().to_owned()
                    };
                    break 'options PythonSource::Command(code);
                }
                _ => return Err(format!("Unknown interpreter option: -{flag}\n{USAGE}")),
            }
        }
        arg = args.next().ok_or(USAGE)?;
    };
    if !isolated && python_env_flag("PYTHONDONTWRITEBYTECODE") {
        dont_write_bytecode = true;
    }
    Ok(PythonInvocation {
        source,
        isolated,
        safe_path: isolated || env::var_os("PYTHONSAFEPATH").is_some_and(|v| !v.is_empty()),
        no_user_site: isolated || python_env_flag("PYTHONNOUSERSITE"),
        dont_write_bytecode,
    })
}

/// Python's integer-valued environment flags accept a nonnumeric string as true.
fn python_env_flag(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| {
        !value.is_empty()
            && value
                .to_str()
                .and_then(|s| s.trim_ascii_start().parse::<i32>().ok())
                != Some(0)
    })
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
    for name in ["RELAR_ENCODED_FLAGS", "RELARFLAGS"] {
        if let Some(value) = env::var_os(name) {
            let text = value
                .to_str()
                .ok_or_else(|| format!("{name} must be UTF-8"))?;
            return Ok(if text.is_empty() {
                Vec::new()
            } else if name == "RELAR_ENCODED_FLAGS" {
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
