use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};

use relar_compiler::CompileOptions;

const USAGE: &str = "Usage: relar check SOURCE.py [--module-root DIRECTORY]\n       relar build SOURCE.py [-o OUTPUT] [BUILD OPTIONS]\n       relar run SOURCE.py [BUILD OPTIONS] [-- PROGRAM_ARGUMENTS...]\n\nBuild options:\n  --module-root DIRECTORY  --runtime LIBRARY.a  --emit-llvm FILE.ll\n  --clang CLANG  --opt-level 0|1|2|3  --sanitize\n  --target TRIPLE  --lto-linker PATH\n\nBuild writes the executable to the current directory using the source filename without its extension.\nRun compiles a temporary executable and passes arguments after -- to the program.\nThinLTO (--lto-linker) requires explicit --runtime and --target.";

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
    let mut output = None;
    let mut runtime = None;
    let mut llvm_path = None;
    let mut module_root = None;
    let mut lto_linker = None;
    let mut target = None;
    let mut clang = cfg!(target_os = "macos").then(|| OsString::from("clang"));
    let mut optimization = OsString::from("3");
    let mut options = CompileOptions::default();
    while let Some(flag) = args.next() {
        if execute && flag == "--" {
            break;
        }
        if command == "check" && flag != "--module-root" {
            return Err(USAGE.to_owned());
        }
        if flag == "--sanitize" {
            options.address_sanitizer = true;
            continue;
        }
        let value = args.next().ok_or(USAGE)?;
        match flag.to_str() {
            Some("--module-root") => module_root = Some(PathBuf::from(value)),
            Some("-o") if !execute => output = Some(PathBuf::from(value)),
            Some("--runtime") => runtime = Some(PathBuf::from(value)),
            Some("--emit-llvm") => llvm_path = Some(PathBuf::from(value)),
            Some("--clang") => clang = Some(value),
            Some("--lto-linker") => lto_linker = Some(PathBuf::from(value)),
            Some("--target") => target = Some(value),
            Some("--opt-level") if matches!(value.to_str(), Some("0" | "1" | "2" | "3")) => {
                optimization = value
            }
            _ => {
                return Err(format!(
                    "Unknown option or invalid value: {}\n{USAGE}",
                    flag.to_string_lossy()
                ));
            }
        }
    }
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
    let runtime = runtime.unwrap_or(
        std::env::current_exe()
            .map_err(|error| error.to_string())?
            .with_file_name("librelar_runtime.a"),
    );
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
        .arg(&runtime)
        .arg("-o")
        .arg(&output);
    if cfg!(target_os = "macos") {
        linker.arg("-liconv");
    } else {
        linker.args(["-ldl", "-lpthread", "-lm", "-lrt", "-lutil"]);
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
    let status = match linker.status() {
        Err(error) if clang.is_none() && error.kind() == ErrorKind::NotFound => {
            let mut fallback = Command::new("clang");
            fallback.args(linker.get_args()).status().map_err(|error| {
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

fn identity(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|error| error.to_string());
    }
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let parent = absolute
        .parent()
        .ok_or("output path has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    Ok(parent.join(absolute.file_name().ok_or("output path has no filename")?))
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
