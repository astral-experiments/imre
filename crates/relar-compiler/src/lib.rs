mod analysis;
mod classes;
mod effects;
mod frontend;
mod ir;
mod llvm;
mod modules;
mod narrowing;
mod ranges;
mod tuples;
mod types;

use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct CompileOptions {
    /// Marks generated functions for LLVM's AddressSanitizer pass.
    pub address_sanitizer: bool,
}

/// Checks source syntax, ty diagnostics, and Relar's exact type contract.
pub fn check(source: &str) -> Result<(), String> {
    frontend::lower(&modules::SourceGraph::single(source)?).map(|_| ())
}

/// Compiles a checked module to LLVM IR; the resulting executable uses no CPython.
pub fn compile(source: &str, options: &CompileOptions) -> Result<String, String> {
    let mut module = frontend::lower(&modules::SourceGraph::single(source)?)?;
    tuples::scalarize(&mut module);
    let ranges = ranges::analyze(&module);
    Ok(llvm::emit(&module, &ranges, options.address_sanitizer))
}

/// The generated IR and every source file that must be protected from output writes.
pub struct Compilation {
    pub llvm_ir: String,
    pub input_paths: Vec<PathBuf>,
}

/// Checks a closed first-party module graph, inferring its package root when omitted.
pub fn check_file(path: &Path, module_root: Option<&Path>) -> Result<(), String> {
    frontend::lower(&modules::SourceGraph::load(path, module_root)?).map(|_| ())
}

/// Compiles one entry module and its statically resolved first-party dependencies.
pub fn compile_file(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<Compilation, String> {
    let graph = modules::SourceGraph::load(path, module_root)?;
    let mut module = frontend::lower(&graph)?;
    tuples::scalarize(&mut module);
    let ranges = ranges::analyze(&module);
    Ok(Compilation {
        llvm_ir: llvm::emit(&module, &ranges, options.address_sanitizer),
        input_paths: graph
            .modules
            .into_iter()
            .map(|module| module.path)
            .collect(),
    })
}

/// Applies Python's source preprocessing before Ruff, ty, and source-range checks.
fn normalize_source(source: &str) -> Result<Cow<'_, str>, String> {
    if source.contains('\0') {
        return Err("source code cannot contain null bytes".into());
    }
    // Python normalizes physical newlines even inside raw and triple-quoted strings.
    // Escaped `\r` characters remain untouched because they contain no physical CR.
    let source = if source.contains('\r') {
        Cow::Owned(source.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(source)
    };
    check_encoding(&source)?;
    Ok(source)
}

/// Recognizes Python's first-two-line encoding cookie while restricting it to UTF-8.
fn check_encoding(source: &str) -> Result<(), String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    for line in source.lines().take(2) {
        let line = line.trim_start_matches([' ', '\t', '\x0c']);
        if !line.starts_with('#') {
            if line.is_empty() {
                continue;
            }
            break;
        }
        for (offset, _) in line.match_indices("coding") {
            let rest = &line[offset + "coding".len()..];
            if !rest.starts_with([':', '=']) {
                continue;
            }
            let rest = rest[1..].trim_start_matches([' ', '\t']);
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | '.'))
                .unwrap_or(rest.len());
            if end == 0 {
                continue;
            }
            let encoding = &rest[..end];
            return if matches!(
                encoding.to_ascii_lowercase().as_str(),
                "utf-8" | "utf_8" | "utf8"
            ) {
                Ok(())
            } else {
                Err(format!(
                    "unsupported source encoding {encoding:?}; use utf-8"
                ))
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CompileOptions, compile};

    #[test]
    fn sealed_imports_work_without_filesystem_modules() {
        let source = r#"
import sys as process
from typing import TextIO as Stream
from io import UnsupportedOperation as WrongMode

def first(stream: Stream) -> str:
    try:
        with stream as current:
            return current.read()
    except WrongMode:
        return str(process.argv[0])
"#;
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        assert!(llvm.contains("define i32 @main(i32 %argc, ptr %argv)"));
        assert!(llvm.contains("call void @relar_args_init(i32 %argc, ptr %argv)"));
        assert_eq!(llvm.matches("call void @relar_args_clear()").count(), 2);
    }

    #[test]
    fn unused_intrinsic_import_does_not_initialize_arguments() {
        let llvm = compile("import sys\nprint(1)\n", &CompileOptions::default()).unwrap();
        assert!(llvm.contains("define i32 @main()"));
        assert!(!llvm.contains("call void @relar_args_init"));
        assert!(!llvm.contains("call void @relar_args_clear"));
    }
}
