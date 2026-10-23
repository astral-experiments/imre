use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use relar_compiler::CompileOptions;

use super::{Allocator, USAGE};

#[derive(Default)]
pub(super) struct BuildOptions {
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
    pub fn parse(
        command: &OsStr,
        args: &mut impl Iterator<Item = OsString>,
    ) -> Result<Self, String> {
        let mut options = Self {
            clang: cfg!(target_os = "macos").then(|| OsString::from("clang")),
            optimization: "3".into(),
            ..Self::default()
        };
        if command != "check" {
            options.read(command, &mut environment_flags()?.into_iter(), false)?;
        }
        options.read(command, args, command == "run")?;
        Ok(options)
    }

    fn read(
        &mut self,
        command: &OsStr,
        args: &mut impl Iterator<Item = OsString>,
        program_arguments: bool,
    ) -> Result<(), String> {
        while let Some(flag) = args.next() {
            if command == "check" && flag != "--module-root" {
                return Err(USAGE.to_owned());
            }
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{} requires a value", flag.display()))
            };
            match flag.to_str() {
                Some("--") if program_arguments => break,
                Some("--module-root") => self.module_root = Some(value()?.into()),
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
