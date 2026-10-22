mod analysis;
mod borrows;
mod callables;
mod classes;
mod effects;
mod frontend;
mod guarded_text;
mod ir;
mod list_bounds;
mod list_specialization;
mod llvm;
mod modules;
mod narrowing;
mod range_specialization;
mod ranges;
mod reductions;
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
    list_bounds::specialize(&mut module);
    list_specialization::specialize(&mut module);
    range_specialization::specialize(&mut module);
    reductions::specialize(&mut module);
    guarded_text::specialize(&mut module);
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
    list_bounds::specialize(&mut module);
    list_specialization::specialize(&mut module);
    range_specialization::specialize(&mut module);
    reductions::specialize(&mut module);
    guarded_text::specialize(&mut module);
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
    fn sealed_exception_fields_keep_runtime_guards_and_reject_unknown_attributes() {
        let llvm = compile(
            "def fields(error: Exception) -> tuple[int, list[str], str, str, str]:\n    return error.returncode, error.cmd, error.stdout, error.output, error.stderr\n",
            &CompileOptions::default(),
        ).unwrap();
        assert!(llvm.contains("call i64 @relar_subprocess_error_field"));
        assert!(llvm.contains("call i8 @relar_error_pending"));
        for source in [
            "def field(error: Exception) -> int:\n    return error.pid\n",
            "def field(text: str) -> str:\n    return text.stdout\n",
            "import subprocess\nsubprocess.unknown()\n",
            "def field(error: Exception) -> None:\n    error.stdout = 'replacement'\n",
            "if False:\n    print('text'.stdout)\n",
        ] {
            assert!(
                compile(source, &CompileOptions::default()).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn deadline_exceptions_keep_exact_byte_fields_through_category_proofs() {
        let llvm = compile(
            include_str!("../../../tests/fixtures/processes/timeouts.py"),
            &CompileOptions::default(),
        )
        .unwrap();
        assert!(llvm.contains("call ptr @relar_subprocess_run_timeout"));
        assert!(llvm.contains("call i64 @relar_subprocess_timeout_field"));
        compile(
            include_str!("../../../tests/fixtures/processes/timeout_arguments.py"),
            &CompileOptions::default(),
        )
        .unwrap();
    }

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
    fn file_iterators_pull_typed_lines_and_keep_reads_in_the_loop() {
        let source = r#"
from gzip import open

def scan(path: str) -> list[str]:
    with open(path, "rt", encoding="utf-8") as stream:
        return [line for line in stream if line]

def first(path: str) -> bytes:
    with open(path, "rb") as stream:
        chunk = stream.read(1)
        for line in stream:
            return chunk + line
        return chunk
"#;
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        for operation in [
            "gzip_open",
            "text_readline",
            "binary_read_size",
            "binary_readline",
        ] {
            assert!(
                llvm.contains(&format!("call ptr @relar_{operation}(")),
                "{operation}"
            );
        }
        assert!(!llvm.contains("call ptr @relar_text_read("));
        assert!(!llvm.contains("call ptr @relar_str_splitlines("));
    }

    #[test]
    fn unused_intrinsic_import_does_not_initialize_arguments() {
        let llvm = compile("import sys\nprint(1)\n", &CompileOptions::default()).unwrap();
        assert!(llvm.contains("define i32 @main()"));
        assert!(!llvm.contains("call void @relar_args_init"));
        assert!(!llvm.contains("call void @relar_args_clear"));
    }

    #[test]
    fn loop_else_joins_break_assignments_after_finally() {
        let source = r#"
def search(values: list[int]) -> int:
    for value in values:
        if value > 0:
            found = value
            break
    else:
        found = -1
    return found

def final_assignment(values: list[int]) -> int:
    for value in values:
        try:
            break
        finally:
            found = value
    else:
        found = -1
    return found

def overridden_break(values: list[int]) -> int:
    for value in values:
        try:
            break
        finally:
            continue
    else:
        found = -1
    return found

def present_after_exhaustion(value: int | None) -> int:
    while value is None:
        value = 1
    else:
        return value + 1
"#;
        compile(source, &CompileOptions::default()).unwrap();
    }

    #[test]
    fn loop_breaks_do_not_preserve_replaced_or_deleted_locals() {
        let replaced = r#"
def invalid(values: list[int]) -> int:
    result: int | None = 1
    for value in values:
        result = value
        try:
            break
        finally:
            result = None
    else:
        result = 1
    return result + 1
"#;
        assert!(compile(replaced, &CompileOptions::default()).is_err());
        let deleted = r#"
def invalid(values: list[int]) -> str:
    caught = ValueError("old")
    for value in values:
        try:
            raise ValueError("new")
        except ValueError as caught:
            break
    else:
        caught = ValueError("empty")
    return str(caught)
"#;
        assert!(compile(deleted, &CompileOptions::default()).is_err());
    }
}
