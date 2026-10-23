use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};

mod cli;

use cli::{BuildOptions, Relro};

#[derive(Clone, Copy, Default)]
enum Allocator {
    #[default]
    System,
    Jemalloc,
}

const JEMALLOC_RUNTIME: &str = "librelar_runtime_jemalloc.a";

const USAGE: &str = "Usage: relar check SOURCE.py [--module-root DIRECTORY]
       relar build SOURCE.py [-o OUTPUT] [BUILD OPTIONS]
       relar run SOURCE.py [BUILD OPTIONS] [-- PROGRAM_ARGUMENTS...]

Build options:
  --module-root DIRECTORY  --runtime LIBRARY.a  --emit-llvm FILE.ll
  --clang CLANG  --opt-level 0|1|2|3  --sanitize
  --target TRIPLE  --lto-linker PATH  --allocator system|jemalloc
  --gc-sections (default)  --no-gc-sections  --verbose-link
  -C relro-level=full|partial|off (Linux, default full)
  -C link-arg=ARG (repeatable, passes one argument to Clang)

Build writes the executable to the current directory using the source filename without its extension.
Run compiles a temporary executable and passes arguments after -- to the program.
ThinLTO (--lto-linker) requires explicit --runtime and --target.
Build and run read RELARFLAGS (whitespace-separated) before command-line options.
RELAR_ENCODED_FLAGS (ASCII 0x1f-separated) takes precedence over RELARFLAGS.";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().ok_or(USAGE)?;
    if command == "--help" || command == "-h" {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if command == "--version" {
        println!(
            "relar {} (Ruff/ty 0.0.14; LLVM AOT)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(ExitCode::SUCCESS);
    }
    let execute = command == "run";
    if command != "build" && command != "check" && !execute {
        return Err(USAGE.to_owned());
    }
    let source = args.next().ok_or(USAGE)?;
    if source == "--help" || source == "-h" {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let source_path = PathBuf::from(source);
    let BuildOptions {
        output,
        runtime,
        llvm_path,
        module_root,
        lto_linker,
        target,
        allocator,
        clang,
        optimization,
        compile: options,
        link,
    } = BuildOptions::parse(&command, &mut args)?;
    if command == "check" {
        relar_compiler::check_file(&source_path, module_root.as_deref())?;
        return Ok(ExitCode::SUCCESS);
    }
    if !cfg!(any(
        all(target_os = "linux", target_pointer_width = "64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )) {
        return Err("Relar supports 64-bit Linux and Apple Silicon macOS hosts".to_owned());
    }
    let run_directory = execute
        .then(tempfile::tempdir)
        .transpose()
        .map_err(|error| error.to_string())?;
    let output = match &run_directory {
        Some(directory) => directory.path().join("program"),
        None => output
            .or_else(|| source_path.file_stem().map(PathBuf::from))
            .ok_or("Could not infer an executable name from the source path; pass -o OUTPUT.")?,
    };
    if lto_linker.is_some() && (runtime.is_none() || target.is_none()) {
        return Err("--lto-linker requires explicit --runtime and --target".to_owned());
    }
    let lto_linker = lto_linker
        .map(|path| {
            if !path.is_file() {
                return Err(format!(
                    "ThinLTO linker is not an existing file: {}",
                    path.display()
                ));
            }
            // Linker symlinks such as ld.lld select a flavor through argv[0].
            // Resolve identities below, but preserve that name when invoking it.
            std::path::absolute(&path).map_err(|error| {
                format!(
                    "Could not resolve ThinLTO linker {}: {error}",
                    path.display()
                )
            })
        })
        .transpose()?;
    let target_os = target_os(target.as_deref());
    let linux = cfg!(target_os = "linux") && target_os == "linux";
    let runtime = runtime_archive(runtime, allocator, linux, options.address_sanitizer)?;
    if !runtime.is_file() {
        return Err(format!(
            "Runtime not found: {}. Build the workspace or pass --runtime.",
            runtime.display()
        ));
    }
    let compilation = relar_compiler::compile_file(&source_path, module_root.as_deref(), &options)?;
    let mut paths = compilation
        .input_paths
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    paths.extend([runtime.as_path(), output.as_path()]);
    if let Some(path) = &llvm_path {
        paths.push(path);
    }
    if let Some(path) = &lto_linker {
        paths.push(path);
    }
    let identities = paths
        .iter()
        .map(|path| identity(path))
        .collect::<Result<Vec<_>, _>>()?;
    for (i, path) in identities.iter().enumerate() {
        if identities[..i]
            .iter()
            .any(|previous| previous == path || same_file(previous, path))
        {
            return Err(
                "All source modules, runtime, ThinLTO linker, executable, and LLVM output must have distinct paths"
                    .to_owned(),
            );
        }
    }
    let ir = compilation.llvm_ir;
    let mut temporary = tempfile::Builder::new()
        .prefix("relar-")
        .suffix(".ll")
        .tempfile()
        .map_err(|error| error.to_string())?;
    temporary
        .write_all(ir.as_bytes())
        .map_err(|error| error.to_string())?;
    if let Some(path) = llvm_path {
        std::fs::write(path, &ir).map_err(|error| error.to_string())?;
    }
    let mut linker = Command::new(clang.as_deref().unwrap_or(OsStr::new("clang-18")));
    linker
        .arg(format!("-O{}", optimization.to_string_lossy()))
        .arg("-Wno-override-module")
        .arg(temporary.path())
        .arg(&runtime);
    match target_os {
        "macos" => {
            linker.arg("-liconv");
            if !link.keep_dead_code {
                linker.arg("-Wl,-dead_strip");
            }
        }
        "linux" => {
            linker.args(["-ldl", "-lpthread", "-lm", "-lrt", "-lutil"]);
            linker.arg(if link.keep_dead_code {
                "-Wl,--no-gc-sections"
            } else {
                "-Wl,--gc-sections"
            });
            linker.arg(match link.relro {
                Relro::Full => "-Wl,-z,relro,-z,now",
                Relro::Partial => "-Wl,-z,relro,-z,lazy",
                Relro::Off => "-Wl,-z,norelro",
            });
        }
        _ => {}
    }
    if options.address_sanitizer {
        linker.args(["-fsanitize=address", "-fno-omit-frame-pointer"]);
    }
    if let Some(path) = &lto_linker {
        let mut argument = OsString::from("-fuse-ld=");
        argument.push(path.as_os_str());
        linker.arg("-flto=thin").arg(argument);
        if optimization == "3" {
            // Runtime accessors cross a language boundary and exceed LLVM's
            // default import/inlining budgets even when their hot paths are small.
            linker.arg("-Wl,-mllvm,-import-instr-limit=1000,-mllvm,-inline-threshold=1000");
        }
    }
    if let Some(target) = target {
        let mut argument = OsString::from("--target=");
        argument.push(target);
        linker.arg(argument);
    }
    linker.args(&link.args).arg("-o").arg(&output);
    let invoke = |driver: &mut Command| {
        if link.verbose {
            eprintln!("{driver:?}");
        }
        driver.status()
    };
    let status = match invoke(&mut linker) {
        Err(error) if clang.is_none() && error.kind() == ErrorKind::NotFound => {
            let mut fallback = Command::new("clang");
            invoke(fallback.args(linker.get_args())).map_err(|error| {
                format!(
                    "Could not run clang-18 or clang: {error}. Install Clang 18 or pass --clang PATH."
                )
            })?
        }
        result => result.map_err(|error| {
            format!(
                "Could not run {}: {error}",
                linker.get_program().to_string_lossy()
            )
        })?,
    };
    if !status.success() {
        return Err(format!("LLVM compilation/linking failed: {status}"));
    }
    if let Some(directory) = run_directory {
        let mut program = Command::new(&output);
        #[cfg(unix)]
        program.arg0(&source_path);
        let mut child = program.args(args).spawn().map_err(|error| {
            format!("Could not run compiled {}: {error}", source_path.display())
        })?;
        // The child has opened its executable. Release both temporary files
        // before waiting, including when the child is subsequently interrupted.
        drop(temporary);
        drop(directory);
        let status = child.wait().map_err(|error| error.to_string())?;
        return Ok(program_exit_code(status));
    }
    Ok(ExitCode::SUCCESS)
}

fn program_exit_code(status: ExitStatus) -> ExitCode {
    #[cfg(unix)]
    let code = status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal));
    #[cfg(not(unix))]
    let code = status.code();
    ExitCode::from(code.unwrap_or(1) as u8)
}

fn runtime_archive(
    runtime: Option<PathBuf>,
    allocator: Option<Allocator>,
    linux: bool,
    sanitize: bool,
) -> Result<PathBuf, String> {
    if let Some(path) = runtime {
        if allocator.is_some() {
            return Err("--runtime already selects the allocator. Omit --allocator.".to_owned());
        }
        if sanitize && path.file_name() == Some(OsStr::new(JEMALLOC_RUNTIME)) {
            return Err("--sanitize requires a system-allocator runtime".to_owned());
        }
        return Ok(path);
    }
    let allocator = match allocator {
        Some(Allocator::Jemalloc) if sanitize => {
            return Err("--allocator jemalloc is incompatible with --sanitize".to_owned());
        }
        Some(Allocator::Jemalloc) if !linux => {
            return Err("--allocator jemalloc requires a Linux target and host".to_owned());
        }
        _ if sanitize || !linux => Allocator::System,
        requested => requested.unwrap_or_default(),
    };
    let filename = match allocator {
        Allocator::System => "librelar_runtime.a",
        Allocator::Jemalloc => JEMALLOC_RUNTIME,
    };
    Ok(std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(filename))
}

fn linux_target(target: &OsStr) -> bool {
    let Some(target) = target.to_str() else {
        return false;
    };
    // Clang accepts both arch-vendor-linux-env and arch-linux-env.
    let mut parts = target.split('-').skip(1);
    match (parts.next(), parts.next()) {
        (_, Some("linux")) => !parts.next().is_some_and(|abi| abi.starts_with("android")),
        (Some("linux"), None) => true,
        (Some("linux"), Some(abi)) => {
            abi.starts_with("gnu") || abi.starts_with("musl") || abi.starts_with("uclibc")
        }
        _ => false,
    }
}

fn target_os(target: Option<&OsStr>) -> &'static str {
    let Some(target) = target else {
        return std::env::consts::OS;
    };
    if linux_target(target) {
        "linux"
    } else if target.to_str().is_some_and(|triple| {
        triple
            .split('-')
            .skip(1)
            .any(|part| part.starts_with("darwin") || part.starts_with("macos"))
    }) {
        "macos"
    } else {
        "unknown"
    }
}

fn identity(path: &Path) -> Result<PathBuf, String> {
    let mut absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    loop {
        match absolute.canonicalize() {
            Ok(resolved) => return Ok(resolved),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let parent = absolute
            .parent()
            .ok_or("output path has no parent")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        // A dangling link names the file the build will create. Resolve its
        // target relative to the link's parent, including through other links.
        match absolute.read_link() {
            Ok(target) => absolute = parent.join(target),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(parent.join(absolute.file_name().ok_or("output path has no filename")?));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Canonical paths catch symlinks; inode identity also protects hard-linked inputs.
fn same_file(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(left), Ok(right)) = (left.metadata(), right.metadata()) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn dangling_output_links_share_their_destinations_identity() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("links")).unwrap();
        symlink("../output", root.join("links/ir")).unwrap();
        symlink("links/ir", root.join("chain")).unwrap();
        let output = identity(&root.join("output")).unwrap();
        assert_eq!(identity(&root.join("links/ir")).unwrap(), output);
        assert_eq!(identity(&root.join("chain")).unwrap(), output);
        assert_ne!(identity(&root.join("other")).unwrap(), output);
        assert!(!output.exists());
    }

    #[test]
    fn allocator_selects_the_sibling_runtime() {
        for (allocator, linux, sanitize, filename) in [
            (None, true, false, "librelar_runtime.a"),
            (Some(Allocator::System), true, false, "librelar_runtime.a"),
            (Some(Allocator::Jemalloc), true, false, JEMALLOC_RUNTIME),
            (None, false, false, "librelar_runtime.a"),
            (None, true, true, "librelar_runtime.a"),
        ] {
            assert_eq!(
                runtime_archive(None, allocator, linux, sanitize).unwrap(),
                std::env::current_exe().unwrap().with_file_name(filename)
            );
        }
    }
}
