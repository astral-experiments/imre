mod analysis;
mod borrows;
mod callables;
mod character_locals;
mod classes;
mod contracts;
mod digit_sums;
mod dynamic;
mod effects;
use imre_regex as fixed_regex;
mod case_membership;
mod frontend;
mod guarded_text;
mod ir;
mod list_bounds;
mod list_specialization;
mod llvm;
mod loop_fusion;
mod modules;
mod narrowing;
mod pair_tables;
mod range_specialization;
mod ranges;
mod record_copies;
mod reductions;
#[cfg(test)]
mod slice_sum_validation;
mod sorted_rows;
mod split_fields;
mod tuples;
mod types;
#[cfg(any(debug_assertions, test, feature = "verify-ir"))]
mod verify;
mod version;
mod watch;
mod write_parts;
mod zip_entry;
pub use zip_entry::is_zip_path;

/// Emits native provider IR for packaging alongside the Linux toolchain.
pub fn write_bundled_provider_ir(directory: &std::path::Path) -> Result<(), String> {
    dynamic::provider_pack::write(directory)
}

use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Python language version used by type checking and native wheel consumers.
pub const PYTHON_VERSION: ruff_python_ast::PythonVersion = version::LANGUAGE;

/// Python implementation identifier used in interpreter metadata and wheel tags.
pub const IMPLEMENTATION_NAME: &str = "imre";

/// File extension of installed native archives linked into an application.
pub const NATIVE_ARCHIVE_EXTENSION: &str = "imre.a";

/// Startup choices made before a native Python command executes.
#[derive(Clone, serde::Serialize)]
pub struct CommandFlags {
    pub isolated: bool,
    pub ignore_environment: bool,
    pub context_aware_warnings: bool,
    /// A positive CPU-count override; zero selects the operating system.
    pub cpu_count: i32,
    /// Python's language optimization level; independent of LLVM optimization.
    pub optimize: i32,
    pub safe_path: bool,
    pub no_user_site: bool,
    pub no_site: bool,
    pub dont_write_bytecode: bool,
    pub unbuffered: bool,
    /// A missing override lets the runtime select the mode from the startup locale.
    pub utf8_mode: Option<bool>,
    pub warn_default_encoding: bool,
    /// Command-line or environment warning options make file finalization observable.
    pub has_warning_options: bool,
    /// Warning options supplied on the command line.
    pub warnoptions: Vec<String>,
    /// Ordered command-line operands used to construct sys._xoptions.
    pub xoptions: Vec<String>,
    pub faulthandler: bool,
    /// Number of Python frames retained per allocation. Zero disables startup tracing.
    pub tracemalloc: u16,
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
    /// Canonical import name requested by the interpreter's `-m` entry.
    pub module_entry: Option<String>,
    /// Run __main__ from the command-line import path, retaining the operand as argv[0].
    pub path_entry: bool,
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
    /// Optional archive of bundled native provider bodies for this target and LLVM optimization.
    pub bundled_provider_archive: Option<PathBuf>,
    /// Compiler-owned archive for a Python launcher missing its installed companion.
    pub stdlib_fallback: Option<PathBuf>,
    /// Existing user and prefix package directories in the launcher's search order.
    pub site_dirs: Vec<PathBuf>,
    /// User directory with .pth files to process during Python startup.
    pub user_site_pth: Option<PathBuf>,
    /// A source sitecustomize module is visible before the command path is prepended.
    pub sitecustomize: bool,
    /// Explicit installation roots containing rebuilt native modules and their type stubs.
    pub extension_paths: Vec<PathBuf>,
    /// Indices in extension_paths admitted as ZIPs from PYTHONPATH during Python startup.
    pub python_archive_indices: Vec<usize>,
}

impl CompileOptions {
    /// Return the launcher's flags and path for a Python command or a script.
    pub fn python_startup(&self) -> Option<(&CommandFlags, &str)> {
        self.command
            .as_ref()
            .map(|command| (&command.flags, command.executable.as_str()))
            .or_else(|| {
                self.script_startup
                    .as_ref()
                    .map(|(flags, executable)| (flags, executable.as_str()))
            })
    }

    fn implicit_sites(&self) -> &[PathBuf] {
        if self
            .python_startup()
            .is_some_and(|(flags, _)| flags.no_site)
        {
            &[]
        } else {
            &self.site_dirs
        }
    }

    fn load_graph(
        &self,
        path: &Path,
        module_root: Option<&Path>,
        filesystem: watch::FileSystem<'_>,
        import_failure: &mut Option<modules::EntryImportFailure>,
    ) -> Result<modules::SourceGraph, String> {
        if let Some(command) = &self.command {
            return modules::SourceGraph::command(command, self, filesystem, import_failure);
        }
        if let Some((flags, executable)) = &self.script_startup {
            return modules::SourceGraph::script(
                path,
                flags,
                executable,
                self,
                filesystem,
                import_failure,
            );
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
    /// A native run of this image can request Python source modules at runtime.
    #[serde(default)]
    pub boxed_imports: bool,
    pub files: Vec<(PathBuf, PathBuf, String)>,
    pub missing: Vec<PathBuf>,
    /// Existing namespace portions and the canonical directories they resolve to.
    #[serde(default)]
    pub directories: Vec<(PathBuf, PathBuf)>,
}

impl SourceInputs {
    pub fn fresh(&self) -> bool {
        self.missing
            .iter()
            .all(|p| matches!(p.try_exists(), Ok(false)))
            && self.directories.iter().all(|(p, identity)| {
                p.is_dir() && p.canonicalize().is_ok_and(|now| &now == identity)
            })
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
    mut cache: Option<&mut IncrementalCompiler>,
    filesystem: watch::FileSystem<'_>,
) -> Result<(), String> {
    let typed = |graph: &mut modules::SourceGraph| {
        let (module, _) = checked_lower_with(graph, cache.as_deref_mut())?;
        verify_after(&module, "lowering")
    };
    object_fallback(
        BoxedLayout::Whole,
        path,
        module_root,
        options,
        filesystem,
        typed,
        |_| (),
    )
}

fn named_bytecode(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().ends_with(b".pyc")
}

/// Inspect only the required prefix of a named bytecode operand, which may be a pipe.
fn read_bytecode_prefix(mut file: std::fs::File) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut prefix = Vec::with_capacity(37);
    file.by_ref().take(4).read_to_end(&mut prefix)?;
    if prefix == imre_ast::PYC_MAGIC_NUMBER_TOKEN.to_le_bytes() {
        file.by_ref().take(13).read_to_end(&mut prefix)?;
        if prefix.get(16).is_some_and(|tag| tag & 0x7f == b'c') {
            file.take(20).read_to_end(&mut prefix)?;
        }
    }
    Ok(prefix)
}

/// Classify a direct interpreter operand before decoding its contents as Python source.
fn bytecode_script_error(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    let magic = imre_ast::PYC_MAGIC_NUMBER_TOKEN.to_le_bytes();
    if !named_bytecode(path) && !bytes.starts_with(&magic[..2]) {
        return None;
    }
    if bytes.len() < 4 {
        return Some("EOFError: EOF read where not expected");
    }
    if bytes[..4] != magic {
        return Some("RuntimeError: Bad magic number in .pyc file");
    }
    if bytes.len() < 16 {
        return Some("EOFError: EOF read where not expected");
    }
    // The interpreter skips three header words, then expects a marshalled code record.
    // Its first fields are five 32-bit integers, and the tag may carry the reference bit.
    if bytes.len() < 16 + 1 + 5 * 4 || bytes[16] & 0x7f != b'c' {
        return Some("RuntimeError: Bad code object in .pyc file");
    }
    Some("NotImplementedError: native marshal cannot load CPython code objects")
}

/// Selects the LLVM outputs for a resolved boxed program.
enum BoxedLayout<'a> {
    Whole,
    Split,
    WholeWithUnits(&'a mut Vec<CodegenUnit>),
}

/// Uses the boxed backend when the checked graph requires Python object operations.
fn object_fallback<T>(
    layout: BoxedLayout<'_>,
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    mut filesystem: watch::FileSystem<'_>,
    mut lower: impl FnMut(&mut modules::SourceGraph) -> Result<T, String>,
    boxed: impl FnOnce(UnitCompilation) -> T,
) -> Result<T, String> {
    if options.module_entry.is_some() {
        return dynamic::compile(
            (path, module_root),
            options,
            true,
            None,
            None,
            layout,
            || Ok(()),
        )?
        .map(boxed)
        .ok_or_else(|| "could not compile the module entry".to_owned());
    }
    let boxed_startup = options.sitecustomize
        || options
            .python_startup()
            .is_some_and(|(flags, _)| flags.tracemalloc > 0);
    let mut import_failure = None;
    let mut infer_sets = true;
    let (error, graph, checked_entry) = loop {
        let mut graph = match options.load_graph(
            path,
            module_root,
            filesystem.reborrow(),
            &mut import_failure,
        ) {
            Ok(graph) => graph,
            Err(error) => break (error, None, false),
        };
        if !infer_sets {
            graph.set_layout_hint.set(modules::SetLayoutHint::Disabled);
        }
        match lower(&mut graph) {
            Ok(_) if graph.runtime_initializer.is_some() && !boxed_startup => {
                // All first-party code lowered successfully. The unresolved import alone
                // must use the same admission policy as an unresolved module in the loader.
                break (graph.runtime_initializer.take().unwrap(), None, false);
            }
            Ok(value) if !boxed_startup => return Ok(value),
            // Tracing and startup imports both need the same Python heap as the entry.
            Ok(_) => {
                break (
                    "Python startup requires a boxed entry".to_owned(),
                    Some(graph),
                    true,
                );
            }
            // A guessed layout must not turn a working object program into a type error,
            // or mask a diagnostic that the ordinary checked-file path would have issued.
            Err(_) if graph.set_layout_hint.get() == modules::SetLayoutHint::Applied => {
                infer_sets = false;
                import_failure = None;
            }
            Err(error) => break (error, Some(graph), false),
        }
    };
    // Python invocations and explicit object imports enforce annotated calls in
    // the boxed runtime, where their TypeError can be caught by the program.
    let checked_file = options.python_startup().is_none() && !options.runtime_imports;
    if options.entry_profile.is_some()
        || (checked_file
            && graph.as_ref().is_some_and(|g| {
                g.return_errors.borrow().contains(&error) || g.imports_struct_decorator()
            }))
    {
        return Err(error);
    }
    let operation = graph.as_ref().and_then(|g| {
        if checked_entry {
            Some(dynamic::ObjectOperation::CheckedEntry)
        } else if g.class_layout_error.borrow().as_ref() == Some(&error) {
            Some(dynamic::ObjectOperation::ClassLayout)
        } else {
            (g.object_builtin_errors.borrow().contains(&error)
                || g.has_native_bytearray.get() && !g.return_errors.borrow().contains(&error))
            .then_some(dynamic::ObjectOperation::Builtin)
        }
    });
    let ordered_command_import = options.command.is_some()
        && import_failure == Some(modules::EntryImportFailure::AfterStatement);
    let runtime_imports = options.runtime_imports
        || import_failure == Some(modules::EntryImportFailure::ZipImport)
        || (options.sitecustomize
            && import_failure == Some(modules::EntryImportFailure::MissingModule))
        || operation.is_some()
        || graph
            .as_ref()
            .is_some_and(|g| g.needs_object_runtime.get() || g.traceback_objects);
    let check_returns = || {
        if let Some(graph) = &graph {
            let requested = graph.check_fallback_calls.get();
            if !checked_file
                && (options.runtime_imports
                    || !matches!(requested, analysis::FallbackCalls::FunctionBodies))
            {
                return Ok(());
            }
            let (skipped, overloads) = graph.diagnostic_scopes();
            let entry = &graph.modules[graph.entry];
            // A deferred probe can call typed functions while initializing additional names.
            let compound_probe = graph.modules.iter().any(|module| {
                module
                    .optional_imports
                    .values()
                    .any(|names| names.len() > 1)
            });
            let all_calls = !matches!(requested, analysis::FallbackCalls::None)
                || checked_file && graph.has_native_bytearray.get()
                || !entry.optional_imports.is_empty()
                || compound_probe
                || !graph.needs_object_runtime.get()
                    && graph.object_builtin_errors.borrow().contains(&error)
                || checked_file && graph.uses_source_loader();
            let checked_calls = if !checked_file {
                requested
            } else if graph.check_method_factories.get() {
                analysis::FallbackCalls::DecoratorFactories
            } else if all_calls {
                analysis::FallbackCalls::All
            } else if entry.has_entry_guard() {
                analysis::FallbackCalls::Entry(graph.entry)
            } else {
                analysis::FallbackCalls::None
            };
            graph.session.diagnostic_copy()?.check_fallback_contracts(
                skipped.as_deref(),
                &overloads,
                checked_calls,
                graph
                    .modules
                    .iter()
                    .position(|module| module.is_bundled("socketserver")),
                checked_file.then_some(&graph.modules),
            )?;
        }
        Ok(())
    };
    match dynamic::compile(
        (path, module_root),
        options,
        runtime_imports,
        operation
            .or(ordered_command_import.then_some(dynamic::ObjectOperation::OrderedCommandImport)),
        graph.as_ref(),
        layout,
        check_returns,
    )? {
        Some(compilation) => Ok(boxed(compilation)),
        None => Err(error),
    }
}

/// Compiles one entry module and its statically resolved first-party dependencies.
pub fn compile_file(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<Compilation, String> {
    compile_file_with_layout(path, module_root, options, BoxedLayout::Whole)
}

/// Preserves the whole-program IR and returns independent boxed initializers for native codegen.
pub fn compile_file_with_boxed_units(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<(Compilation, Vec<CodegenUnit>), String> {
    let mut units = Vec::new();
    let compilation = compile_file_with_layout(
        path,
        module_root,
        options,
        BoxedLayout::WholeWithUnits(&mut units),
    )?;
    Ok((compilation, units))
}

/// Resolves and checks the source graph once for the requested boxed outputs.
fn compile_file_with_layout(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    layout: BoxedLayout<'_>,
) -> Result<Compilation, String> {
    let typed = |graph: &mut modules::SourceGraph| {
        let (mut module, _) = checked_lower(graph)?;
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
    };
    object_fallback(
        layout,
        path,
        module_root,
        options,
        Default::default(),
        typed,
        |mut c| Compilation {
            llvm_ir: c
                .units
                .pop()
                .expect("whole boxed program has one unit")
                .llvm_ir,
            input_paths: c.input_paths,
            source_inputs: c.source_inputs,
            extension_archives: c.extension_archives,
        },
    )
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
    cached_set_hint: bool,
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
    mut cache: Option<&mut IncrementalCompiler>,
) -> Result<UnitCompilation, String> {
    let typed = |graph: &mut modules::SourceGraph| {
        let (mut module, mut owners) = checked_lower_with(graph, cache.as_deref_mut())?;
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
    };
    object_fallback(
        BoxedLayout::Split,
        path,
        module_root,
        options,
        Default::default(),
        typed,
        |c| c,
    )
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
    let Some(compiler) =
        cache.filter(|_| graph.set_layout_hint.get() != modules::SetLayoutHint::Disabled)
    else {
        return checked_lower(graph);
    };
    compiler.cache.restore_hints(graph)?;
    let callers = compiler.callers.prepare(graph);
    if callers.is_some() && compiler.cached_set_hint {
        graph.set_layout_hint.set(modules::SetLayoutHint::Applied);
    }
    let lowered = frontend::lower_with_caller_cache(graph, callers);
    compiler.cached_set_hint =
        lowered.is_ok() && graph.set_layout_hint.get() == modules::SetLayoutHint::Applied;
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
    loop_fusion::specialize(module);
    verify_after(module, "producer-consumer loop fusion")?;
    sorted_rows::specialize(module);
    verify_after(module, "sorted record rows")?;
    let reader_origins = record_copies::scalarize(module);
    verify_after(module, "record copy scalarization")?;
    tuples::scalarize(module);
    verify_after(module, "tuple scalarization")?;
    character_locals::specialize(module);
    verify_after(module, "character local specialization")?;
    digit_sums::specialize(module);
    verify_after(module, "digit reduction fusion")?;
    write_parts::specialize(module);
    verify_after(module, "text write forwarding")?;
    split_fields::specialize(module);
    verify_after(module, "integer split fields")?;
    case_membership::specialize(module);
    verify_after(module, "case-converted string set membership")?;
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
    pair_tables::specialize(module);
    verify_after(module, "packed integer-pair tables")?;
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

/// Diagnose a malformed on-disk script after the file itself was read successfully.
fn script_decode_error(path: &Path, error: std::string::FromUtf8Error) -> String {
    let bytes = error.as_bytes();
    let invalid = error.utf8_error();
    let offset = invalid.valid_up_to();
    let byte = bytes[offset];
    // CR, LF and CRLF each terminate one physical line.
    let endings = bytes.iter().enumerate().filter_map(|(i, &c)| {
        (c == b'\n' || (c == b'\r' && bytes.get(i + 1) != Some(&b'\n'))).then_some(i)
    });
    let header_end = endings.clone().nth(1).unwrap_or(bytes.len());
    let mut line = 1;
    let mut start = 0;
    for end in endings.take_while(|&end| end < offset) {
        line += 1;
        start = end + 1;
    }
    let source = String::from_utf8_lossy(&bytes[..header_end])
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let path = path.display();
    let declared = match check_encoding(&source) {
        Ok(declared) => declared,
        Err(message) => return format!("{path}: {message}"),
    };
    let message = if declared {
        let bom = usize::from(start == 0 && bytes.starts_with(b"\xEF\xBB\xBF")) * 3;
        let mut position = offset - start - bom;
        if line == 2 {
            // While looking for a second-line cookie the tokenizer retains line one.
            let header = source.strip_prefix('\u{feff}').unwrap_or(&source);
            let first = header.lines().next().unwrap_or_default();
            let first_token = first.trim_start_matches([' ', '\t', '\x0c']);
            if (first_token.is_empty() || first_token.starts_with('#'))
                && check_encoding(first) == Ok(false)
            {
                position += first.len() + 1;
            }
        }
        let length = invalid.error_len().unwrap_or(bytes.len() - offset);
        // The tokenizer reads a final physical line with an implicit newline.
        let reason = if (0xc2..=0xf4).contains(&byte) {
            "invalid continuation byte"
        } else {
            "invalid start byte"
        };
        let span = if length == 1 {
            format!("byte 0x{byte:02x} in position {position}")
        } else {
            format!("bytes in position {position}-{}", position + length - 1)
        };
        format!("(unicode error) 'utf-8' codec can't decode {span}: {reason}")
    } else {
        format!(
            "Non-UTF-8 code starting with '\\x{byte:02x}' in file {path} on line {line}, but no encoding declared; see https://peps.python.org/pep-0263/ for details"
        )
    };
    format!("  File \"{path}\", line {line}\nSyntaxError: {message}")
}

/// Validate the first two lines and report whether a UTF-8 cookie or BOM was present.
fn check_encoding(source: &str) -> Result<bool, String> {
    let stripped = source.strip_prefix('\u{feff}');
    let source = stripped.unwrap_or(source);
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
                Ok(true)
            } else {
                Err(format!(
                    "unsupported source encoding {encoding:?}; use utf-8"
                ))
            };
        }
    }
    Ok(stripped.is_some())
}

#[cfg(test)]
mod tests {
    use super::{CompileOptions, compile};

    #[test]
    fn native_bytearray_construction_preserves_object_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        // Printing the mutable buffer directly uses its object implementation.
        std::fs::write(
            &path,
            "buffer = bytearray()\nbuffer.append(97)\nprint(buffer)\n",
        )
        .unwrap();
        super::check_file(&path, None).unwrap();

        // Taking the object route must still enforce a typed callee's argument.
        std::fs::write(
            &path,
            "def identity(x: int) -> int:\n    return x\n\nbuffer = bytearray()\nprint(identity('x'))\n",
        )
        .unwrap();
        assert!(super::check_file(&path, None).is_err());

        // Explicit runtime imports must also preserve annotated return errors.
        std::fs::write(
            &path,
            "def build() -> int:\n    buffer = bytearray()\n    return 'bad'\n\nprint(build())\n",
        )
        .unwrap();
        let options = super::CompileOptions {
            runtime_imports: true,
            ..Default::default()
        };
        let error = super::check_file_with_options(&path, None, &options).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }

    #[test]
    fn inferred_set_layout_preserves_checked_object_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        let gather = "def gather(text):\n    values = set()\n    for char in text:\n        values.add(char)\n    return sorted(values)\n";
        let mut incremental = super::IncrementalCompiler::default();
        for usage in [
            "values = gather('ab')\nvalues.append(1)\nprint(len(values))\n",
            "def integer(n: int) -> int:\n    return n\nprint(gather('a'), integer('x'))\n",
            "def wrong() -> int:\n    return 'x'\nprint(gather('a'), wrong())\n",
        ] {
            std::fs::write(&path, format!("{gather}{usage}")).unwrap();
            for runtime_imports in [false, true] {
                let options = CompileOptions {
                    runtime_imports,
                    ..Default::default()
                };
                let ordinary = super::object_fallback(
                    super::BoxedLayout::Whole,
                    &path,
                    None,
                    &options,
                    Default::default(),
                    |graph| {
                        graph
                            .set_layout_hint
                            .set(super::modules::SetLayoutHint::Disabled);
                        let (module, _) = super::checked_lower(graph)?;
                        super::verify_after(&module, "lowering")
                    },
                    |_| (),
                );
                assert_eq!(
                    super::check_file_with_options(&path, None, &options),
                    ordinary
                );
                if !runtime_imports {
                    assert_eq!(incremental.check_file(&path, None), ordinary);
                }
            }
        }
    }

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
    fn nonconvergent_list_hints_preserve_diagnostics_and_later_checks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("subject.py");
        let mut compiler = super::IncrementalCompiler::default();
        for (item, expected) in [
            ("(a, b)", "expected int, got tuple[int, int]"),
            ("a", "iterator values require an immediate"),
        ] {
            let source = format!(
                "def f():\n    rows = []\n    for a, b in zip([1], [2], strict=True):\n        rows.append({item})\n    nested = []\n    for _ in range(2):\n        nested = zip(nested)\nf()\n"
            );
            std::fs::write(&path, source).unwrap();
            let error = compiler.check_file(&path, None).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
        std::fs::write(
            &path,
            "def f():\n    rows = []\n    for c in 'xy':\n        rows.append(c)\n    return rows\nf()\n",
        )
        .unwrap();
        compiler.check_file(&path, None).unwrap();
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

        let clauses = "row is not None or ".repeat(64);
        super::check(&format!(
            "def f(row: int | None) -> bool:\n    return {clauses}False\n"
        ))
        .unwrap();
        let wide_boolean = "flag or ".repeat(4096);
        assert!(
            super::check(&format!(
                "def f(flag: bool):\n    return {wide_boolean}False\n"
            ))
            .unwrap_err()
            .contains("syntax complexity limit")
        );
        let handlers = format!(
            "handlers = {{{}}}\n",
            "0: lambda first, second: second, ".repeat(128)
        );
        super::analysis::check_source_complexity(&handlers).unwrap();
        let nested_defaults = format!(
            "f = {}0{}\n",
            "lambda a=0, b=".repeat(128),
            ": b".repeat(128)
        );
        assert!(super::analysis::check_source_complexity(&nested_defaults).is_err());

        let conditional = "1 if True or False else ".repeat(256);
        assert!(
            super::check(&format!("print({conditional}0)\n"))
                .unwrap_err()
                .contains("syntax complexity limit")
        );

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
        #[cfg(unix)]
        super::check("import _signal\n").unwrap();

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
        assert_eq!(llvm.matches("call void @imre_args_clear()").count(), 1);
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
    fn handler_flow_requires_a_direct_unconditional_raise() {
        let source = r#"
class Policy:
    def stop(self, value: str) -> None:
        "Report a rejected value."
        raise ValueError(value)

def parse(policy: Policy, text: str) -> int:
    try:
        parsed = int(text)
    except ValueError:
        policy.stop(text)
    return parsed
"#;
        super::check(source).unwrap();

        let conditional = source.replace(
            "        raise ValueError(value)",
            "        if value:\n            raise ValueError(value)",
        );
        let overridden = format!(
            "{source}\nclass Continue(Policy):\n    def stop(self, value: str) -> None:\n        pass\n"
        );
        for unproven in [conditional, overridden] {
            let error = super::check(&unproven).unwrap_err();
            assert!(
                error.contains("local may be uninitialized on this path"),
                "{error}"
            );
        }
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
