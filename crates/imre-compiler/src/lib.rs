mod analysis;
mod borrows;
mod callables;
mod classes;
mod contracts;
mod dynamic;
mod effects;
use imre_regex as fixed_regex;
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
mod record_copies;
mod reductions;
#[cfg(test)]
mod slice_sum_validation;
mod sorted_rows;
mod tuples;
mod types;
#[cfg(any(debug_assertions, test, feature = "verify-ir"))]
mod verify;
mod version;
mod watch;

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Python language version used by type checking and native wheel consumers.
pub const PYTHON_VERSION: ruff_python_ast::PythonVersion = version::LANGUAGE;

/// Python implementation identifier used in interpreter metadata and wheel tags.
pub const IMPLEMENTATION_NAME: &str = "imre";

/// File extension of installed native archives linked into an application.
pub const NATIVE_ARCHIVE_EXTENSION: &str = "imre.a";

/// Startup choices made before a native Python command executes.
#[derive(Clone, Copy, serde::Serialize)]
pub struct CommandFlags {
    pub isolated: bool,
    /// Python's language optimization level; independent of LLVM optimization.
    pub optimize: i32,
    pub safe_path: bool,
    pub no_user_site: bool,
    pub dont_write_bytecode: bool,
    pub utf8_mode: bool,
}

/// The in-memory entry and launcher used by a Python-style `-c` invocation.
pub struct PythonCommand {
    pub source: String,
    pub flags: CommandFlags,
    pub executable: String,
}

#[derive(Default)]
pub struct CompileOptions {
    /// Compile the supplied command without creating a script in the caller's working directory.
    pub command: Option<PythonCommand>,
    /// Flags and executable for a file launched through the Python command line.
    pub script_startup: Option<(CommandFlags, String)>,
    /// A file describing the branches to include in a closed executable.
    pub entry_profile: Option<PathBuf>,
    /// Limit native lowering to definitions a closed entry program can use.
    pub reachable_only: bool,
    /// Allow file entries with ordinary imports to use the native object runtime after static lowering fails.
    pub runtime_imports: bool,
    /// Marks generated functions for LLVM's AddressSanitizer pass.
    pub address_sanitizer: bool,
    /// Explicit installation roots containing rebuilt native modules and their type stubs.
    pub extension_paths: Vec<PathBuf>,
}

impl CompileOptions {
    /// Return the launcher's flags and path for a Python command or a script.
    pub fn python_startup(&self) -> Option<(CommandFlags, &str)> {
        self.command
            .as_ref()
            .map(|command| (command.flags, command.executable.as_str()))
            .or_else(|| {
                self.script_startup
                    .as_ref()
                    .map(|(flags, executable)| (*flags, executable.as_str()))
            })
    }

    fn load_graph(
        &self,
        path: &Path,
        module_root: Option<&Path>,
        filesystem: watch::FileSystem<'_>,
    ) -> Result<modules::SourceGraph, String> {
        if let Some(command) = &self.command {
            return modules::SourceGraph::command(command, self, filesystem);
        }
        if let Some((flags, executable)) = &self.script_startup {
            return modules::SourceGraph::script(path, *flags, executable, self, filesystem);
        }
        modules::SourceGraph::load_with(
            path,
            module_root,
            &self.extension_paths,
            filesystem,
            self.reachable_only || self.entry_profile.is_some(),
            self.entry_profile.as_deref(),
        )
    }
}

/// Checks source syntax, ty diagnostics, and Imre's exact type contract.
pub fn check(source: &str) -> Result<(), String> {
    let (module, _) = checked_lower(&mut modules::SourceGraph::single(source)?)?;
    verify_after(&module, "lowering")
}

/// Compiles a checked module to LLVM IR; the resulting executable uses no CPython.
pub fn compile(source: &str, options: &CompileOptions) -> Result<String, String> {
    if options.entry_profile.is_some() {
        return Err("an entry profile requires a source file and its module name".to_owned());
    }
    let (mut module, _) = checked_lower(&mut modules::SourceGraph::single_with_mode(
        source,
        options.reachable_only,
    )?)?;
    specialize(&mut module)?;
    let ranges = ranges::analyze(&module);
    llvm::emit(&module, &ranges, options.address_sanitizer)
}

/// The generated IR and every source file that must be protected from output writes.
pub struct Compilation {
    pub llvm_ir: String,
    pub input_paths: Vec<PathBuf>,
    pub source_inputs: SourceInputs,
    /// Native archives reached by the static import graph, in link order.
    pub extension_archives: Vec<PathBuf>,
}

/// Files and failed import lookups observed by the source loader.
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct SourceInputs {
    pub files: Vec<(PathBuf, PathBuf, String)>,
    pub missing: Vec<PathBuf>,
}

impl SourceInputs {
    pub fn fresh(&self) -> bool {
        self.missing
            .iter()
            .all(|p| matches!(p.try_exists(), Ok(false)))
            && self.files.iter().all(|(p, identity, digest)| {
                p.canonicalize().is_ok_and(|now| &now == identity)
                    && std::fs::read(p)
                        .is_ok_and(|bytes| blake3::hash(&bytes).to_hex().as_str() == digest)
            })
    }
}

/// Checks a closed first-party module graph, inferring its package root when omitted.
pub fn check_file(path: &Path, module_root: Option<&Path>) -> Result<(), String> {
    check_file_with_options(path, module_root, &CompileOptions::default())
}

/// Checks a first-party entry using the same reachability policy as its native build.
pub fn check_file_with_options(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<(), String> {
    check_file_with(path, module_root, options, None, Default::default())
}

/// Checks first-party code against the declarations in explicitly installed extension wheels.
pub fn check_file_with_extensions(
    path: &Path,
    module_root: Option<&Path>,
    extension_paths: &[PathBuf],
) -> Result<(), String> {
    check_file_with_options(
        path,
        module_root,
        &CompileOptions {
            extension_paths: extension_paths.to_vec(),
            ..CompileOptions::default()
        },
    )
}

fn check_file_with(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    cache: Option<&mut IncrementalCompiler>,
    filesystem: watch::FileSystem<'_>,
) -> Result<(), String> {
    let typed = (|| {
        let mut graph = options.load_graph(path, module_root, filesystem)?;
        let (module, _) = checked_lower_with(&mut graph, cache)?;
        verify_after(&module, "lowering")
    })();
    object_fallback(path, module_root, options, typed, |_| ())
}

/// Compile runtime import paths with native object operations when static lowering cannot resolve them.
fn object_fallback<T>(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    typed: Result<T, String>,
    boxed: impl FnOnce(Compilation) -> T,
) -> Result<T, String> {
    match typed {
        Ok(value) => Ok(value),
        Err(error) if options.entry_profile.is_none() => {
            match dynamic::compile(path, module_root, options)? {
                Some(compilation) => Ok(boxed(compilation)),
                None => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

/// Compiles one entry module and its statically resolved first-party dependencies.
pub fn compile_file(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<Compilation, String> {
    let typed = (|| {
        let mut graph = options.load_graph(path, module_root, Default::default())?;
        let (mut module, _) = checked_lower(&mut graph)?;
        specialize(&mut module)?;
        let ranges = ranges::analyze(&module);
        let (source_inputs, input_paths) = graph.disk_inputs(options.entry_profile.as_deref());
        Ok(Compilation {
            llvm_ir: llvm::emit(&module, &ranges, options.address_sanitizer)?,
            extension_archives: graph
                .modules
                .iter()
                .filter_map(|m| m.archive.clone())
                .collect(),
            source_inputs,
            input_paths,
        })
    })();
    object_fallback(path, module_root, options, typed, |c| c)
}

pub struct CodegenUnit {
    pub name: String,
    pub llvm_ir: String,
}

pub struct UnitCompilation {
    pub units: Vec<CodegenUnit>,
    pub input_paths: Vec<PathBuf>,
    pub source_inputs: SourceInputs,
    /// Native archives reached by the static import graph, in link order.
    pub extension_archives: Vec<PathBuf>,
}

/// Reuses ty's semantic queries across revisions of a first-party program in this process.
/// A change confined to literal `int` or `bool` defaults on explicitly typed top-level functions
/// can also reuse completed caller bodies that did not consume those defaults. Each call resolves
/// imports, checks the whole program and runs the requested code generation against current files.
#[derive(Default)]
pub struct IncrementalCompiler {
    cache: analysis::AnalysisCache,
    callers: frontend::CallerCache,
}

impl IncrementalCompiler {
    /// Checks the current revision, including unused definitions in every reachable file.
    pub fn check_file(&mut self, path: &Path, module_root: Option<&Path>) -> Result<(), String> {
        check_file_with(
            path,
            module_root,
            &CompileOptions::default(),
            Some(self),
            Default::default(),
        )
    }

    /// Reports an initial check and subsequent source changes until interrupted. Source
    /// diagnostics are delivered to `report`, so fixing a failed check can produce the next result.
    pub fn watch_file(
        &mut self,
        path: &Path,
        module_root: Option<&Path>,
        report: impl FnMut(Result<(), String>),
    ) -> Result<(), String> {
        self.watch_file_with_options(path, module_root, &CompileOptions::default(), report)
    }

    /// Watches the file graph used by a check with the given entry profile and import roots.
    pub fn watch_file_with_options(
        &mut self,
        path: &Path,
        module_root: Option<&Path>,
        options: &CompileOptions,
        report: impl FnMut(Result<(), String>),
    ) -> Result<(), String> {
        watch::check(self, path, module_root, options, report)
    }

    /// Watches the current revision and declarations in installed extension wheels.
    pub fn watch_file_with_extensions(
        &mut self,
        path: &Path,
        module_root: Option<&Path>,
        extension_paths: &[PathBuf],
        report: impl FnMut(Result<(), String>),
    ) -> Result<(), String> {
        self.watch_file_with_options(
            path,
            module_root,
            &CompileOptions {
                extension_paths: extension_paths.to_vec(),
                ..CompileOptions::default()
            },
            report,
        )
    }

    /// Checks the current revision and emits the same codegen units as [`compile_file_units`].
    pub fn compile_file_units(
        &mut self,
        path: &Path,
        module_root: Option<&Path>,
        options: &CompileOptions,
    ) -> Result<UnitCompilation, String> {
        compile_file_units_with(path, module_root, options, Some(self))
    }
}

/// Keeps whole-program checking and specialization, then partitions native code by source owner.
pub fn compile_file_units(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<UnitCompilation, String> {
    compile_file_units_with(path, module_root, options, None)
}

fn compile_file_units_with(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    cache: Option<&mut IncrementalCompiler>,
) -> Result<UnitCompilation, String> {
    let typed = (|| {
        let mut graph = options.load_graph(path, module_root, Default::default())?;
        let (mut module, mut owners) = checked_lower_with(&mut graph, cache)?;
        for original in specialize(&mut module)? {
            owners.push(owners[original]);
        }
        let ranges = ranges::analyze(&module);
        let names = graph
            .modules
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>();
        let (source_inputs, input_paths) = graph.disk_inputs(options.entry_profile.as_deref());
        Ok(UnitCompilation {
            units: llvm::emit_units(
                &module,
                &ranges,
                options.address_sanitizer,
                &owners,
                &names,
                graph.entry,
            )?,
            extension_archives: graph
                .modules
                .iter()
                .filter_map(|m| m.archive.clone())
                .collect(),
            input_paths,
            source_inputs,
        })
    })();
    object_fallback(path, module_root, options, typed, |c| UnitCompilation {
        units: vec![CodegenUnit {
            name: "__main__".into(),
            llvm_ir: c.llvm_ir,
        }],
        input_paths: c.input_paths,
        source_inputs: c.source_inputs,
        extension_archives: c.extension_archives,
    })
}

/// Reject unsupported operations before ty renders potentially exponential types.
fn checked_lower(
    graph: &mut modules::SourceGraph,
) -> Result<(ir::Module, Vec<modules::ModuleId>), String> {
    let lowered = frontend::lower_with_owners(graph)?;
    let (skipped, overloads) = graph.diagnostic_scopes();
    graph.session.check_scopes(skipped.as_deref(), &overloads)?;
    Ok(lowered)
}

fn checked_lower_with(
    graph: &mut modules::SourceGraph,
    cache: Option<&mut IncrementalCompiler>,
) -> Result<(ir::Module, Vec<modules::ModuleId>), String> {
    let Some(compiler) = cache else {
        return checked_lower(graph);
    };
    compiler.cache.restore_hints(graph)?;
    let lowered = frontend::lower_with_caller_cache(graph, compiler.callers.prepare(graph));
    compiler.cache.retain_hints(graph, lowered.is_ok());
    let checked = lowered.and_then(|lowered| {
        let (skipped, overloads) = graph.diagnostic_scopes();
        compiler.cache.check_scopes(
            &mut graph.session,
            &graph.inputs,
            skipped.as_deref(),
            &overloads,
        )?;
        Ok(lowered)
    });
    if checked.is_err() {
        compiler.callers = frontend::CallerCache::default();
    }
    checked
}

fn specialize(module: &mut ir::Module) -> Result<Vec<usize>, String> {
    verify_after(module, "lowering")?;
    sorted_rows::specialize(module);
    verify_after(module, "sorted record rows")?;
    let reader_origins = record_copies::scalarize(module);
    verify_after(module, "record copy scalarization")?;
    tuples::scalarize(module);
    verify_after(module, "tuple scalarization")?;
    list_bounds::specialize(module);
    verify_after(module, "list bounds specialization")?;
    list_specialization::specialize(module);
    verify_after(module, "list specialization")?;
    range_specialization::specialize(module);
    verify_after(module, "range specialization")?;
    reductions::specialize(module);
    verify_after(module, "reduction specialization")?;
    guarded_text::specialize(module);
    verify_after(module, "guarded text specialization")?;
    Ok(reader_origins)
}

/// Names the first stage that produces malformed IR, before later passes consume it.
fn verify_after(module: &ir::Module, stage: &str) -> Result<(), String> {
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    {
        verify::module(module)
            .map_err(|error| format!("internal compiler error: invalid IR after {stage}: {error}"))
    }
    #[cfg(not(any(debug_assertions, test, feature = "verify-ir")))]
    {
        let _ = (module, stage);
        Ok(())
    }
}

/// Python applies file encodings and source-file indentation policy only to disk input.
#[derive(Clone, Copy, PartialEq)]
enum SourceKind {
    File,
    Command,
}

/// Normalize physical newlines before Ruff, ty, and source-range checks.
fn normalize_source(source: &str, kind: SourceKind) -> Result<Cow<'_, str>, String> {
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
    // A -c operand is already decoded; Python ignores file encoding cookies here.
    if kind == SourceKind::File {
        check_encoding(&source)?;
    }
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
    fn unsupported_handle_comparisons_are_diagnosed_before_lowering() {
        for source in [
            "import tomli\nv = tomli.loads('x = 1')\nprint(v == v != v)\n",
            "import logging\nv = logging.getLogger('x')\nprint(v != v)\n",
            "import asyncio\nv = asyncio.Semaphore(1)\nprint(v != v)\n",
            "from tempfile import TemporaryDirectory\nv = TemporaryDirectory()\nprint(v == v)\n",
        ] {
            let error = super::check(source).unwrap_err();
            assert!(
                error.contains("unsupported comparison for these types"),
                "{error}"
            );
        }
    }

    #[test]
    fn unsupported_operations_on_shared_tuple_types_have_bounded_diagnostics() {
        let mut source = String::from("def f(x0: int) -> None:\n");
        for i in 0..14 {
            source.push_str(&format!("    x{} = (x{i}, x{i})\n", i + 1));
        }
        super::check(&source).unwrap();
        for operation in [
            "    x14()\n",
            "    with x14:\n        pass\n",
            "    raise x14\n",
        ] {
            let error = super::check(&format!("{source}{operation}")).unwrap_err();
            assert!(error.len() < 2048, "diagnostic had {} bytes", error.len());
        }
    }

    #[test]
    fn reject_starred_tuple_displays_before_expanding_their_fields() {
        let mut globals = String::from("a0 = (1,)\n");
        for i in 1..=16 {
            globals.push_str(&format!("a{i} = (*a{}, *a{})\n", i - 1, i - 1));
        }
        let function = format!(
            "def f() -> int:\n    {}return 1\n",
            globals.replace('\n', "\n    ")
        );
        for rejected in [
            globals.as_str(),
            function.as_str(),
            "def f(x: tuple[int] = (*(1,),)) -> int:\n    return 0\n",
        ] {
            let error = super::check(rejected).unwrap_err();
            assert!(
                error.contains("starred elements in tuple displays"),
                "{error}"
            );
        }
        super::check("def f(x: int) -> int:\n    a, *rest = (x, x)\n    b = [*rest]\n    return a + len(b)\n").unwrap();
    }

    #[test]
    fn deep_source_returns_a_diagnostic_but_wide_displays_compile() {
        for expression in [
            format!("{}1", "-".repeat(4096)),
            format!("{}1", "1+".repeat(4096)),
            format!("{}1{}", "[".repeat(2048), "]".repeat(2048)),
            format!("x{}", ".x".repeat(4096)),
        ] {
            let error = super::check(&format!("print({expression})\n")).unwrap_err();
            assert!(error.contains("syntax complexity limit"), "{error}");
        }
        let flat = format!("print(len([{}]))\n", "(1 + 2),".repeat(256));
        compile(&flat, &CompileOptions::default()).unwrap();

        let fields = "{1 + 2} ".repeat(256);
        super::check(&format!("print(f'{fields}')\n")).unwrap();
        for fields in [
            format!("{fields}{{{}}}", "1+".repeat(256) + "1"),
            format!("{{{}}}{fields}", "1+".repeat(256) + "1"),
        ] {
            let error = super::check(&format!("print(f'{fields}')\n")).unwrap_err();
            assert!(error.contains("syntax complexity limit"), "{error}");
        }
    }

    #[test]
    fn joins_conditional_writes_in_boolean_conditions() {
        for operator in ["and", "or"] {
            let source = format!(
                "def check(flag: bool) -> bool:\n    if (first := flag) {operator} (last := flag):\n        pass\n    return first\n"
            );
            super::check(&source).unwrap();
            assert!(super::check(&source.replace("return first", "return last")).is_err());
        }
    }

    #[test]
    fn nested_conditional_values_join_assignments_from_every_arm() {
        let source = "def f(flag: bool, value: int | None) -> int:\n    kept = 7\n    selected = (saved := kept) if flag else ((saved := value) if value is not None else (saved := 0))\n    return saved + selected + kept\n";
        super::check(source).unwrap();
        assert!(super::check(&source.replace("(saved := 0)", "0")).is_err());
    }

    #[test]
    fn nested_boolean_exits_preserve_only_guaranteed_writes() {
        let source = "def f(flag: bool, other: bool) -> bool:\n    result = (saved := flag) and (other or (flag and (saved := other)))\n    return saved\n";
        super::check(source).unwrap();
        assert!(super::check(&source.replace("(saved := flag)", "flag")).is_err());
    }

    #[test]
    fn elif_chains_count_toward_branch_depth() {
        let flat = format!("if False: pass\n{}", "elif False: pass\n".repeat(10_000));
        let nested = format!(
            "if False: pass\n{}else:\n    if False: pass\n{}",
            "elif False: pass\n".repeat(60),
            "    elif False: pass\n".repeat(60)
        );
        for source in [flat, nested] {
            let error = super::check(&source).unwrap_err();
            assert!(error.contains("syntax complexity limit"), "{error}");
        }
        super::check(&"if False: pass\nelif False: pass\n".repeat(128)).unwrap();
    }

    #[test]
    fn limits_exception_handlers_per_try_statement() {
        let handler = "except ValueError:\n    pass\n";
        let chain = |count| format!("try:\n    pass\n{}", handler.repeat(count));
        super::check(&chain(96)).unwrap();
        let error = super::check(&chain(97)).unwrap_err();
        assert!(error.contains("exception handler limit"), "{error}");
        super::check(&chain(2).repeat(100)).unwrap();
        let inner: String = chain(96)
            .lines()
            .map(|line| format!("    {line}\n"))
            .collect();
        super::check(&format!("try:\n{inner}{}", handler.repeat(96))).unwrap();
    }

    #[test]
    fn finally_updates_only_breaks_that_pass_through_it() {
        let source = concat!(
            "def choose(run: bool, early: bool) -> str:\n",
            "    while run:\n",
            "        if early:\n",
            "            word = 'early'\n",
            "            break\n",
            "        try:\n",
            "            break\n",
            "        finally:\n",
            "            word = 'late'\n",
            "    else:\n",
            "        word = 'else'\n",
            "    return word\n",
        );
        super::check(source).unwrap();
        let missing = source.replace("word = 'early'", "pass");
        assert!(super::check(&missing).is_err());
    }

    #[test]
    fn joins_elif_exits_through_an_exhaustive_type_guard() {
        let source = concat!(
            "def choose(value: str | bytes) -> str:\n",
            "    if isinstance(value, str):\n",
            "        answer = value\n",
            "    elif isinstance(value, bytes):\n",
            "        answer = value.decode()\n",
            "    else:\n",
            "        pass\n",
            "    return answer\n",
        );
        super::check(source).unwrap();
        let missing = source.replace("answer = value.decode()", "pass");
        assert!(super::check(&missing).is_err());
    }

    #[test]
    fn joins_chained_comparison_exits_after_the_first_two_operands() {
        let source = "def check(value: int) -> int:\n    (first := value) == (second := value) < (last := value)\n    return first + second\n";
        super::check(source).unwrap();
        assert!(super::check(&source.replace("first + second", "first + last")).is_err());
    }

    #[test]
    fn joins_only_falling_exception_handler_exits() {
        let source = "def choose(value: int) -> str:\n    try:\n        if value == 0:\n            raise ValueError('zero')\n        raise IndexError('one')\n    except ValueError:\n        word = 'zero'\n    except IndexError:\n        return 'one'\n    return word\n";
        super::check(source).unwrap();
        let missing = source.replace("return 'one'", "pass");
        assert!(super::check(&missing).is_err());
    }

    #[test]
    fn key_sort_checks_the_detached_list_before_using_it() {
        let source = concat!(
            "def key(word: str) -> int:\n",
            "    print(word)\n",
            "    return len(word)\n",
            "values = ['bb', 'a']\n",
            "values.sort(key=key)\n",
        );
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        let (_, after_call) = llvm.split_once("call ptr @imre_sort_detach(").unwrap();
        assert!(
            after_call
                .lines()
                .nth(1)
                .unwrap()
                .contains("call i32 @imre_error_pending()")
        );
    }

    #[test]
    fn sealed_exception_fields_keep_runtime_guards_and_reject_unknown_attributes() {
        let llvm = compile(
            "def fields(error: Exception) -> tuple[int, list[str], str, str, str]:\n    return error.returncode, error.cmd, error.stdout, error.output, error.stderr\n",
            &CompileOptions::default(),
        ).unwrap();
        assert!(llvm.contains("call i64 @imre_subprocess_error_field"));
        assert!(llvm.contains("call i32 @imre_error_pending"));
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
        assert!(llvm.contains("call ptr @imre_run_config"));
        assert!(llvm.contains("call i64 @imre_subprocess_timeout_field"));
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
        assert!(llvm.contains("call void @imre_args_init(i32 %argc, ptr %argv)"));
        assert_eq!(llvm.matches("call void @imre_args_clear()").count(), 2);
    }

    #[test]
    fn limits_semantic_snapshots_for_loop_transfers() {
        let mut source = String::from("def check(value: int) -> None:\n");
        for slot in 0..6_000 {
            source.push_str(&format!("    a{slot} = 0\n"));
        }
        for transfer in ["break", "continue"] {
            let body = format!("        if value: {transfer}\n").repeat(3_000);
            let error = super::check(&format!("{source}    while value:\n{body}")).unwrap_err();
            assert!(
                error.contains("loop transfers exceed the flow analysis limit"),
                "{error}"
            );
        }
    }

    #[test]
    fn limits_match_flow_snapshots_within_the_enclosing_scope() {
        let locals: String = (0..4000).map(|i| format!("    a{i}=0\n")).collect();
        let case = "        case 0: pass\n";
        let wide = case.repeat(600);
        let header = "def big(value: int) -> None:\n";
        let subject = "    match value:\n";
        let error = super::check(&format!("{header}{locals}{subject}{wide}")).unwrap_err();
        assert!(
            error.contains("match statement exceeds the flow analysis limit"),
            "{error}"
        );
        super::check(&format!(
            "{header}{locals}{subject}{case}def small(value: int) -> None:\n{subject}{wide}"
        ))
        .unwrap();
    }

    #[test]
    fn context_manager_depth_counts_flat_and_nested_items() {
        let managers = |n| vec!["f"; n].join(", ");
        let file = "f = open('/dev/null', 'rb')\n";
        let flat = format!("{file}with {}:\n    pass\n", managers(10_000));
        let nested = format!(
            "{file}with {}:\n    with {}:\n        pass\n",
            managers(60),
            managers(60)
        );
        for source in [flat, nested] {
            let error = super::check(&source).unwrap_err();
            assert!(
                error.contains("context manager nesting exceeds 96 levels"),
                "{error}"
            );
        }
        let sequential = format!("{file}{}", "with f, f:\n    pass\n".repeat(100));
        super::check(&sequential).unwrap();
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
                llvm.contains(&format!("call ptr @imre_{operation}(")),
                "{operation}"
            );
        }
        assert!(!llvm.contains("call ptr @imre_text_read("));
        assert!(!llvm.contains("call ptr @imre_str_splitlines("));
    }

    #[test]
    fn unused_intrinsic_import_does_not_initialize_arguments() {
        let llvm = compile("import sys\nprint(1)\n", &CompileOptions::default()).unwrap();
        assert!(llvm.contains("define i32 @main()"));
        assert!(!llvm.contains("call void @imre_args_init"));
        assert!(!llvm.contains("call void @imre_args_clear"));
    }

    #[test]
    fn nested_finally_emission_stays_bounded() {
        fn indent(body: &str) -> String {
            body.lines().map(|line| format!("    {line}\n")).collect()
        }
        fn program(depth: usize, nested_cleanup: bool) -> String {
            let mut body = "pass\n".to_owned();
            for _ in 0..depth {
                let nested = indent(&body);
                body = if nested_cleanup {
                    format!("try:\n    int(text)\nfinally:\n{nested}")
                } else {
                    format!("try:\n{nested}finally:\n    int(text)\n")
                };
            }
            format!("def f(text: str) -> None:\n{}f('1')\n", indent(&body))
        }
        let options = CompileOptions::default();
        let shallow = compile(&program(8, false), &options).unwrap();
        let deep = compile(&program(12, false), &options).unwrap();
        assert!(deep.len() < 2 * shallow.len());
        let error = compile(&program(14, true), &options).unwrap_err();
        assert!(error.contains("finally cleanup expansion"), "{error}");
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
    fn loop_break_does_not_restore_a_deleted_exception_target() {
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
        let error = compile(deleted, &CompileOptions::default()).unwrap_err();
        assert!(
            error.contains("local may be uninitialized on this path"),
            "{error}"
        );
    }
}
