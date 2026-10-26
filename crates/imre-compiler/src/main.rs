use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

mod build_cache;
mod build_output;
mod cli;
mod python_command;
#[cfg(unix)]
mod run_program;
mod wheel;

use build_output::BuildOutput;
use cli::{BuildOptions, Relro};
use imre_compiler::CodegenUnit;

#[derive(Clone, Copy, Default)]
enum Allocator {
    #[default]
    System,
    Jemalloc,
}

const JEMALLOC_RUNTIME: &str = "libimre_runtime_jemalloc.a";
const PYTHON_HEADER: &str = include_str!("../../imre-runtime/include/Python.h");

const USAGE: &str = "Usage: imre check SOURCE.py [--module-root DIRECTORY] [--extension-path DIRECTORY] [--reachable-only] [--runtime-imports] [--entry-profile FILE] [--watch | -w]
       imre [-I] [-B] [-X utf8[=0|1]] -c CODE [PROGRAM_ARGUMENTS...]
       imre extension-header
       imre build-target [--output-dir DIRECTORY]
       imre wheel PROJECT [--out-dir DIR] [--overlay DIR] [--python PYTHON]
       imre build SOURCE.py [-o OUTPUT] [BUILD OPTIONS]
       imre run SOURCE.py [BUILD OPTIONS] [-- PROGRAM_ARGUMENTS...]

Build options:
  --module-root DIRECTORY  --runtime LIBRARY.a  --emit-llvm FILE.ll
  --reachable-only         Compile definitions reachable from program entry and module initialization
  --runtime-imports        Allow native object dispatch when static lowering cannot compile an import graph
  --extension-path DIRECTORY  Search an installed Imre wheel directory (repeatable)
  --clang CLANG  --opt-level 0|1|2|3  --sanitize  --no-cache  --whole-program
  -q, --quiet              Suppress build status
  -v, --verbose            Show individual module decisions
  --target TRIPLE  --lto-linker PATH  --allocator system|jemalloc
  --gc-sections (default)  --no-gc-sections  --verbose-link
  -C relro-level=full|partial|off (Linux, default full)
  -C link-arg=ARG (repeatable, passes one argument to Clang)

Build writes the executable to the current directory using the source filename without its extension.
Check --watch reports each revision to stderr until interrupted.
Build and run share cached module objects and executables on supported native Linux toolchains.
Use --whole-program to optimize all modules together.
Use --entry-profile FILE to compile selected branches and reject excluded modes at runtime.
Override the cache location with IMRE_CACHE_DIR or bypass it with --no-cache.
Run passes arguments after -- to the program.
ThinLTO (--lto-linker) requires explicit --runtime and --target.
Build and run read IMREFLAGS (whitespace-separated) before command-line options.
IMRE_ENCODED_FLAGS (ASCII 0x1f-separated) takes precedence over IMREFLAGS.";

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
    let mut args = std::env::args_os().skip(1).peekable();
    let command = args.next().ok_or(USAGE)?;
    if command == "--help" || command == "-h" {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if command == "--version" {
        println!(
            "imre {} (Ruff/ty 0.0.14; LLVM AOT)",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(ExitCode::SUCCESS);
    }
    if command == "build-target" {
        return wheel::report(args);
    }
    if command == "wheel" {
        return wheel::run(args);
    }
    if command == "extension-header" {
        if args.next().is_some() {
            return Err("imre extension-header takes no arguments".into());
        }
        print!("{PYTHON_HEADER}");
        return Ok(ExitCode::SUCCESS);
    }
    let (build_options, execute) = if command == "build" || command == "check" || command == "run" {
        if args
            .peek()
            .is_some_and(|arg| arg == "--help" || arg == "-h")
        {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        (BuildOptions::parse(&command, &mut args)?, command == "run")
    } else if command.as_encoded_bytes().starts_with(b"-")
        || Path::new(&command).is_file()
        || Path::new(&command)
            .extension()
            .is_some_and(|ext| ext == "py")
    {
        let invocation = match python_command::parse(command.clone(), &mut args) {
            Ok(value) => value,
            Err(message) => {
                eprintln!("{message}");
                return Ok(ExitCode::from(2));
            }
        };
        if let python_command::Invocation::Script { path, .. } = &invocation
            && let Err(error) = std::fs::metadata(path)
        {
            eprintln!("imre: can't open file '{}': {error}", path.display());
            return Ok(ExitCode::from(2));
        }
        (BuildOptions::for_python(invocation)?, true)
    } else {
        return Err(USAGE.to_owned());
    };
    let BuildOptions {
        source_path,
        help,
        watch,
        no_cache,
        whole_program,
        verbosity,
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
    } = build_options;
    if help {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let source_path = source_path.ok_or(USAGE)?;
    if command == "check" {
        if watch {
            imre_compiler::IncrementalCompiler::default().watch_file_with_options(
                &source_path,
                module_root.as_deref(),
                &options,
                |result| match result {
                    Ok(()) => eprintln!("Check passed"),
                    Err(error) => eprintln!("{error}\nCheck failed"),
                },
            )?;
        } else {
            imre_compiler::check_file_with_options(&source_path, module_root.as_deref(), &options)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if !cfg!(any(
        all(target_os = "linux", target_pointer_width = "64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )) {
        return Err("Imre supports 64-bit Linux and Apple Silicon macOS hosts".to_owned());
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
    let mut progress = BuildOutput::new(verbosity);
    let label = if execute { &source_path } else { &output };
    let bundled = clang
        .is_none()
        .then(build_cache::NativeToolchain::bundled)
        .flatten();
    let bundled_native = bundled
        .as_ref()
        .filter(|_| target.as_deref().is_none_or(native_linux_gnu_target));
    let native = if clang.is_none()
        && target.is_none()
        && lto_linker.is_none()
        && llvm_path.is_none()
        && !options.address_sanitizer
        && link.args.is_empty()
    {
        build_cache::native_toolchain(&runtime, bundled_native)
    } else {
        None
    };
    let split = native.is_some() && !whole_program;
    let native_lld = if lto_linker.is_some()
        || link.args.iter().any(|arg| {
            arg.as_encoded_bytes().starts_with(b"-fuse-ld=")
                || arg.as_encoded_bytes().starts_with(b"--ld-path=")
        }) {
        None
    } else {
        bundled_native
            .or(native.as_ref())
            .and_then(|tools| tools.lld.as_deref())
    };
    let cache = native
        .as_ref()
        .filter(|_| !no_cache && !options.reachable_only)
        .and_then(|native| {
            build_cache::Cache::open(
                (&source_path, &options),
                module_root.as_deref(),
                &optimization,
                &runtime,
                native,
                split,
                &link,
            )
        });
    // Wheel archives and their optional library manifests must be reread at link time.
    // Generated module objects are still keyed by their complete LLVM IR.
    let executable_cache = cache
        .as_ref()
        .filter(|_| options.extension_paths.is_empty());
    if let Some(cache) = &cache {
        if identity(&output)?.starts_with(&cache.root) {
            return Err("output cannot overwrite the executable cache".into());
        }
        // A verbose link must invoke the driver so its printed command is accurate.
        if !link.verbose
            && let Some((file, sources)) = executable_cache.and_then(|cache| cache.lookup())
        {
            validate_paths(
                sources.files.iter().map(|(p, _, _)| p.as_path()),
                &runtime,
                &output,
                None,
                native_lld,
                bundled.as_ref().map(|tools| tools.clang.as_path()),
            )?;
            build_cache::materialize(&file, &output).map_err(|e| e.to_string())?;
            build_cache::trace("hit");
            progress.cached(label);
            if let Some(directory) = run_directory {
                progress.running(&source_path);
                return run_program(&output, directory, &source_path, args);
            }
            return Ok(ExitCode::SUCCESS);
        }
        build_cache::trace(if link.verbose || executable_cache.is_none() {
            "relink"
        } else {
            "miss"
        });
    } else {
        build_cache::trace("bypass");
    }
    progress.compiling(&source_path, &optimization);
    enum Code {
        Whole(String),
        Units(Vec<CodegenUnit>),
    }
    let (code, input_paths, sources, extension_archives) = if split {
        let c = imre_compiler::compile_file_units(&source_path, module_root.as_deref(), &options)?;
        (
            Code::Units(c.units),
            c.input_paths,
            c.source_inputs,
            c.extension_archives,
        )
    } else {
        let c = imre_compiler::compile_file(&source_path, module_root.as_deref(), &options)?;
        (
            Code::Whole(c.llvm_ir),
            c.input_paths,
            c.source_inputs,
            c.extension_archives,
        )
    };
    let (library_manifests, native_libraries) = extension_libraries(&extension_archives)?;
    validate_paths(
        input_paths
            .iter()
            .chain(&extension_archives)
            .chain(&library_manifests)
            .map(PathBuf::as_path),
        &runtime,
        &output,
        llvm_path.as_deref(),
        lto_linker.as_deref().or(native_lld),
        bundled.as_ref().map(|tools| tools.clang.as_path()),
    )?;
    let stage = executable_cache.and_then(|c| c.stage());
    let staged_output = stage.as_ref().map(|s| s.path().join("program"));
    let mut temporary = None;
    let mut scratch = None;
    let native_inputs = match code {
        Code::Whole(ir) => {
            let mut file = tempfile::Builder::new()
                .prefix("imre-")
                .suffix(".ll")
                .tempfile()
                .map_err(|e| e.to_string())?;
            file.write_all(ir.as_bytes()).map_err(|e| e.to_string())?;
            if let Some(path) = llvm_path {
                std::fs::write(path, &ir).map_err(|e| e.to_string())?;
            }
            let input = file.path().to_owned();
            temporary = Some(file);
            vec![input]
        }
        Code::Units(units) => {
            if stage.is_none() {
                scratch = Some(tempfile::tempdir().map_err(|e| e.to_string())?);
            }
            let dir = stage
                .as_ref()
                .or(scratch.as_ref())
                .expect("unit compilation has a staging directory");
            let objects = build_cache::compile_objects(
                &units,
                native
                    .as_ref()
                    .expect("native toolchain for module objects"),
                &optimization,
                dir.path(),
                cache
                    .as_ref()
                    .filter(|_| stage.is_some() || !options.extension_paths.is_empty()),
                |name, hit| progress.unit(name, hit),
            )?;
            progress.linking();
            objects
        }
    };
    let mut linker = match &bundled {
        Some(tools) => tools.driver(),
        None => Command::new(clang.as_deref().unwrap_or(OsStr::new("clang-18"))),
    };
    if let Some(tools) = bundled_native {
        tools.native_link_paths(&mut linker);
    }
    if let Some(lld) = native_lld {
        let mut argument = OsString::from("--ld-path=");
        argument.push(lld);
        linker.arg(argument);
    }
    linker
        .arg(format!("-O{}", optimization.to_string_lossy()))
        .arg("-Wno-override-module")
        .args(&native_inputs)
        .args(&extension_archives)
        .arg(&runtime)
        .args(native_libraries);
    if let Some(stage) = &stage {
        let mut depfile = OsString::from("--dependency-file=");
        depfile.push(stage.path().join("link.d"));
        linker
            .env("TMPDIR", stage.path())
            .arg("-Xlinker")
            .arg(depfile);
        if native_lld.is_some()
            && let Ok(trace) = std::fs::File::create(stage.path().join("link.trace"))
        {
            linker.arg("-Wl,--trace").stdout(trace);
        }
    }
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
    linker
        .args(&link.args)
        .arg("-o")
        .arg(staged_output.as_ref().unwrap_or(&output));
    let invoke = |driver: &mut Command| {
        if link.verbose {
            eprintln!("{driver:?}");
        }
        driver.status()
    };
    let status = match invoke(&mut linker) {
        Err(error)
            if clang.is_none() && bundled.is_none() && error.kind() == ErrorKind::NotFound =>
        {
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
    if let (Some(cache), Some(stage), Some(file)) = (&cache, &stage, &staged_output) {
        build_cache::materialize(file, &output).map_err(|e| e.to_string())?;
        if cache
            .publish(sources, stage.path(), file, &output)
            .is_some()
        {
            build_cache::trace("store");
        }
    }
    progress.finished(label);
    if let Some(directory) = run_directory {
        progress.running(&source_path);
        drop(temporary);
        drop(scratch);
        drop(stage);
        return run_program(&output, directory, &source_path, args);
    }
    Ok(ExitCode::SUCCESS)
}

/// Read only library names, never arbitrary driver or linker options, from a wheel.
/// Keep input manifests protected by the same output alias check as the archives.
fn extension_libraries(archives: &[PathBuf]) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    let mut manifests = Vec::new();
    let mut libraries = Vec::new();
    for archive in archives {
        let path = archive.with_extension("libs");
        let canonical = match path.canonicalize() {
            Ok(canonical) => canonical,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if canonical.parent() != archive.parent() {
            return Err(format!(
                "link library manifest resolves outside the archive directory: {}",
                path.display()
            ));
        }
        let contents =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        for name in contents.lines().filter(|line| !line.is_empty()) {
            if !name.starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_')
                || !name
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"._+-".contains(&ch))
            {
                return Err(format!(
                    "{}: invalid native link library name {name:?}",
                    path.display()
                ));
            }
            let arg = format!("-l{name}");
            if !libraries.contains(&arg) {
                libraries.push(arg);
            }
        }
        manifests.push(path);
    }
    Ok((manifests, libraries))
}

fn validate_paths<'a>(
    sources: impl Iterator<Item = &'a Path>,
    runtime: &'a Path,
    output: &'a Path,
    llvm_path: Option<&'a Path>,
    linker: Option<&'a Path>,
    clang: Option<&'a Path>,
) -> Result<(), String> {
    let mut paths = sources.collect::<Vec<_>>();
    paths.extend([runtime, output]);
    if let Some(path) = llvm_path {
        paths.push(path);
    }
    if let Some(path) = linker {
        paths.push(path);
    }
    paths.extend(clang);
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
                "All source modules, runtime, linker, executable, and LLVM output must have distinct paths"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

fn run_program(
    output: &Path,
    directory: tempfile::TempDir,
    source: &Path,
    args: impl Iterator<Item = OsString>,
) -> Result<ExitCode, String> {
    #[cfg(unix)]
    let status = run_program::launch(output, directory, source, args);
    #[cfg(not(unix))]
    let status = {
        let _directory = directory;
        Command::new(output).args(args).status()
    };
    status
        .map(program_exit_code)
        .map_err(|e| format!("Could not run compiled {}: {e}", source.display()))
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
        Allocator::System => "libimre_runtime.a",
        Allocator::Jemalloc => JEMALLOC_RUNTIME,
    };
    Ok(std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(filename))
}

/// The release's startup objects and libraries match only its native GNU/Linux ABI.
fn native_linux_gnu_target(target: &OsStr) -> bool {
    let Some((arch, rest)) = target.to_str().and_then(|s| s.split_once('-')) else {
        return false;
    };
    arch == std::env::consts::ARCH
        && (rest == "linux-gnu"
            || rest
                .split_once('-')
                .is_some_and(|(_, os)| os == "linux-gnu"))
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

    #[test]
    fn private_link_inputs_match_the_native_arch_and_abi() {
        let arch = std::env::consts::ARCH;
        for (target, expected) in [
            (format!("{arch}-linux-gnu"), true),
            (format!("{arch}-unknown-linux-gnu"), true),
            (format!("{arch}-unknown-linux-musl"), false),
            (format!("{arch}-linux-gnux32"), false),
            ("other-unknown-linux-gnu".to_owned(), false),
        ] {
            assert_eq!(native_linux_gnu_target(OsStr::new(&target)), expected);
        }
    }

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
            (None, true, false, "libimre_runtime.a"),
            (Some(Allocator::System), true, false, "libimre_runtime.a"),
            (Some(Allocator::Jemalloc), true, false, JEMALLOC_RUNTIME),
            (None, false, false, "libimre_runtime.a"),
            (None, true, true, "libimre_runtime.a"),
        ] {
            assert_eq!(
                runtime_archive(None, allocator, linux, sanitize).unwrap(),
                std::env::current_exe().unwrap().with_file_name(filename)
            );
        }
    }
}
