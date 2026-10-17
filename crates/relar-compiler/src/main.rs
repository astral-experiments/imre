use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use relar_compiler::CompileOptions;

const USAGE: &str = "Usage: relar check SOURCE.py [--module-root DIRECTORY]\n       relar build SOURCE.py -o OUTPUT [--module-root DIRECTORY]\n           [--runtime LIBRARY.a] [--emit-llvm FILE.ll] [--clang CLANG]\n           [--opt-level 0|1|2|3] [--sanitize]";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().ok_or(USAGE)?;
    if command == "--help" || command == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    if command == "--version" {
        println!(
            "relar {} (Ruff/ty 0.0.14; LLVM AOT)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }
    if command != "build" && command != "check" {
        return Err(USAGE.to_owned());
    }
    let source_path = PathBuf::from(args.next().ok_or(USAGE)?);
    let mut output = None;
    let mut runtime = None;
    let mut llvm_path = None;
    let mut module_root = None;
    let mut clang = OsString::from("clang");
    let mut optimization = OsString::from("3");
    let mut options = CompileOptions::default();
    while let Some(flag) = args.next() {
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
            Some("-o") => output = Some(PathBuf::from(value)),
            Some("--runtime") => runtime = Some(PathBuf::from(value)),
            Some("--emit-llvm") => llvm_path = Some(PathBuf::from(value)),
            Some("--clang") => clang = value,
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
        return relar_compiler::check_file(&source_path, module_root.as_deref());
    }
    if !cfg!(all(target_os = "linux", target_pointer_width = "64")) {
        return Err("The prototype currently supports 64-bit Linux hosts".to_owned());
    }
    let output = output.ok_or(USAGE)?;
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
                "All source modules, runtime, executable, and LLVM output must have distinct paths"
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
    let mut linker = Command::new(clang);
    linker
        .arg(format!("-O{}", optimization.to_string_lossy()))
        .arg("-Wno-override-module")
        .arg(temporary.path())
        .arg(&runtime)
        .args(["-ldl", "-lpthread", "-lm", "-lrt", "-lutil"])
        .arg("-o")
        .arg(&output);
    if options.address_sanitizer {
        linker.args(["-fsanitize=address", "-fno-omit-frame-pointer"]);
    }
    let status = linker
        .status()
        .map_err(|error| format!("Could not run clang: {error}"))?;
    if !status.success() {
        return Err(format!("LLVM compilation/linking failed: {status}"));
    }
    Ok(())
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
