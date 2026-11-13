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
use frontend::phase::{Diagnostic as LoweringDiagnostic, Kind as LoweringKind};

mod guarded_text;
mod installed_wheels;
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
mod single_emission_tests;
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
pub use zip_entry::{is_zip_path, packaged_stdlib_zip};

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
pub const NATIVE_ARCHIVE_EXTENSION: &str = imre_ast::NATIVE_ARCHIVE_EXTENSIONS[1];

/// Startup choices made before a native Python command executes.
#[cfg_attr(test, derive(Default))]
#[derive(Clone, serde::Serialize)]
pub struct CommandFlags {
    pub isolated: bool,
    pub ignore_environment: bool,
    pub context_aware_warnings: bool,
    /// Enable the optional frozen modules at startup.
    pub frozen_modules: bool,
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
    /// Operating system for the linked runtime. Library callers default to the compiler's target.
    pub target_os: Option<&'static str>,
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
    /// Marks generated functions for LLVM's AddressSanitizer pass.
    pub address_sanitizer: bool,
    /// Optional archive of bundled native provider bodies for this target and LLVM optimization.
    pub bundled_provider_archive: Option<PathBuf>,
    /// Compiler-owned archive for a Python launcher missing its installed companion.
    pub stdlib_fallback: Option<PathBuf>,
    /// Adjacent SDK archive selected by the Python launcher.
    pub installed_stdlib_archive: Option<PathBuf>,
    /// The base interpreter's installed filesystem standard library, when present.
    pub stdlib_dir: Option<PathBuf>,
    /// Existing user and prefix package directories in the launcher's search order.
    pub site_dirs: Vec<PathBuf>,
    /// User directory with .pth files to process during Python startup.
    pub user_site_pth: Option<PathBuf>,
    /// A source sitecustomize module is visible before the command path is prepended.
    pub sitecustomize: bool,
    /// An installed stdlib hook can run on object startup without selecting that backend.
    pub stdlib_sitecustomize: bool,
    /// Explicit installation roots containing rebuilt native modules and their type stubs.
    pub extension_paths: Vec<PathBuf>,
    /// Additional source modules to link for imports whose names are chosen at runtime.
    pub include_modules: std::collections::BTreeSet<String>,
    /// Entries contributed by PYTHONPATH, after explicitly configured installation roots.
    pub pythonpath: std::ops::Range<usize>,
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

    fn installed_dirs(&self) -> impl Iterator<Item = &PathBuf> {
        let sites = if self
            .python_startup()
            .is_some_and(|(flags, _)| flags.no_site)
        {
            &[][..]
        } else {
            self.site_dirs.as_slice()
        };
        self.stdlib_dir.iter().chain(sites)
    }

    fn load_graph(
        &self,
        path: &Path,
        module_root: Option<&Path>,
        filesystem: watch::FileSystem<'_>,
        import_failure: &mut Option<modules::EntryImportFailure>,
        site_probes: &mut modules::SiteProbes,
    ) -> Result<Option<modules::SourceGraph>, String> {
        if let Some(name) = &self.module_entry {
            return modules::SourceGraph::module_entry(path, name, self, filesystem, site_probes);
        }
        if let Some(command) = &self.command {
            return modules::SourceGraph::command(
                command,
                self,
                filesystem,
                import_failure,
                site_probes,
            )
            .map(Some);
        }
        if let Some((flags, executable)) = &self.script_startup {
            return modules::SourceGraph::script(
                path,
                flags,
                executable,
                self,
                filesystem,
                import_failure,
                site_probes,
            )
            .map(Some);
        }
        modules::SourceGraph::load_with(
            path,
            module_root,
            self,
            filesystem,
            import_failure,
            site_probes,
        )
        .map(Some)
    }
}

/// The isolated source origin has a diagnostic filename but no filesystem search root.
#[derive(Clone, Copy)]
enum ProgramEntry<'a> {
    Source(&'a str),
    File(&'a Path, Option<&'a Path>),
}

/// Checks source syntax, ty diagnostics, and Imre's exact type contract.
pub fn check(source: &str) -> Result<(), String> {
    check_with(
        ProgramEntry::Source(source),
        &CompileOptions::default(),
        None,
        Default::default(),
    )
}

/// Compiles a checked module to LLVM IR; the resulting executable uses no CPython.
pub fn compile(source: &str, options: &CompileOptions) -> Result<String, String> {
    if options.entry_profile.is_some() {
        return Err("an entry profile requires a source file and its module name".to_owned());
    }
    let options = CompileOptions {
        target_os: options.target_os,
        reachable_only: options.reachable_only,
        address_sanitizer: options.address_sanitizer,
        ..Default::default()
    };
    compile_with_layout(
        ProgramEntry::Source(source),
        &options,
        CodegenLayout::Whole,
        None,
    )
    .map(|(product, _)| product.into_whole().llvm_ir)
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
    /// Relink when a wheel or its native dependencies cannot be validated for an executable hit.
    #[serde(skip)]
    pub relink_site_wheel: bool,
    pub files: Vec<(PathBuf, PathBuf, String)>,
    /// Physical sources supplying type declarations without a runtime import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declaration_files: Vec<(PathBuf, PathBuf, String)>,
    pub missing: Vec<PathBuf>,
    /// File-kind queries that affect import admission without reading a provider's contents.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub file_checks: std::collections::BTreeMap<PathBuf, bool>,
    /// Resolution of runpy's normalized open path when it differs from the loader's origin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared_file_resolutions: Vec<(PathBuf, Option<PathBuf>)>,
    /// Import search directories and the canonical directories they resolve to.
    #[serde(default)]
    pub directories: Vec<(PathBuf, PathBuf)>,
    /// Installed distribution RECORD paths visible in each searched wheel root.
    #[serde(default)]
    pub wheel_records: Vec<(PathBuf, Vec<PathBuf>)>,
}

impl SourceInputs {
    /// Every physical source read by this compilation, including type-only imports.
    pub fn source_files(&self) -> impl Iterator<Item = &(PathBuf, PathBuf, String)> {
        self.files.iter().chain(&self.declaration_files)
    }

    /// Paths that a build must protect from being overwritten by its output.
    fn protected_paths(&self, profile: Option<&Path>) -> Vec<PathBuf> {
        self.source_files()
            .map(|(path, _, _)| path.clone())
            .chain(profile.map(Path::to_path_buf))
            .collect()
    }

    pub fn fresh(&self) -> bool {
        self.missing
            .iter()
            .all(|p| matches!(p.try_exists(), Ok(false)))
            && self
                .file_checks
                .iter()
                .all(|(path, expected)| path.is_file() == *expected)
            && self
                .wheel_records
                .iter()
                .all(|(root, expected)| &installed_wheels::records(root) == expected)
            && self
                .declared_file_resolutions
                .iter()
                .all(|(path, identity)| path.canonicalize().ok().as_ref() == identity.as_ref())
            && self.directories.iter().all(|(p, identity)| {
                p.is_dir() && p.canonicalize().is_ok_and(|now| &now == identity)
            })
            && self.source_files().all(|(p, identity, digest)| {
                !digest.is_empty()
                    && p.canonicalize().is_ok_and(|now| &now == identity)
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
    check_with(
        ProgramEntry::File(path, module_root),
        options,
        cache,
        filesystem,
    )
}

fn check_with(
    entry: ProgramEntry<'_>,
    options: &CompileOptions,
    mut cache: Option<&mut IncrementalCompiler>,
    filesystem: watch::FileSystem<'_>,
) -> Result<(), String> {
    object_fallback(
        None,
        entry,
        options,
        filesystem,
        cache.is_some(),
        |graph, call_error| {
            graph.session.explicit_check = true;
            checked_lower_with(graph, cache.as_deref_mut(), call_error)
        },
    )
    .map(drop)
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

/// Selects the LLVM outputs after the program has been checked.
pub enum CodegenLayout {
    /// Emit a single LLVM module.
    Whole,
    /// Partition functions and initializers by their source module.
    Split,
    /// Keep fixed functions together and split registered module initializers for ThinLTO.
    ThinLto,
    /// Return the native units and save a whole-program dump when their LLVM differs.
    SplitWithWhole,
}

/// The ordered code and optional whole-program dump requested by a build.
struct ProgramOutput {
    layout: CodegenLayout,
    compilation: UnitCompilation,
    whole: Option<String>,
}

/// Uses the boxed backend when the checked graph requires Python object operations.
fn object_fallback(
    mut output: Option<&mut ProgramOutput>,
    entry: ProgramEntry<'_>,
    options: &CompileOptions,
    mut filesystem: watch::FileSystem<'_>,
    incremental: bool,
    mut lower: impl FnMut(
        &mut modules::SourceGraph,
        Option<&LoweringDiagnostic>,
    ) -> Result<CheckedProgram, String>,
) -> Result<SourceInputs, String> {
    for name in &options.include_modules {
        if !modules::valid_module_operand(name) {
            return Err(format!(
                "--include-module requires an absolute module name, got {name:?}"
            ));
        }
    }
    let native_includes = || {
        if options.include_modules.is_empty() {
            Ok(())
        } else {
            Err(
                "--include-module requires a runtime module loader; this entry is fully typed"
                    .to_owned(),
            )
        }
    };
    let checked_file = options.python_startup().is_none();
    // A Python module launch enters through the runtime registration. Other callers
    // can return a native diagnostic before registration, including profiled entries.
    let registered_initializer =
        options.module_entry.is_some() && !checked_file && options.entry_profile.is_none();
    let boxed_startup = options.module_entry.is_some()
        || options.sitecustomize
        || options
            .python_startup()
            .is_some_and(|(flags, _)| flags.tracemalloc > 0);
    let mut import_failure = None;
    let mut site_probes = modules::SiteProbes::default();
    let loaded = match entry {
        ProgramEntry::Source(source) => Ok(Some(modules::SourceGraph::single_with_mode(
            source,
            options.reachable_only,
        )?)),
        ProgramEntry::File(path, module_root) => options.load_graph(
            path,
            module_root,
            filesystem.reborrow(),
            &mut import_failure,
            &mut site_probes,
        ),
    };
    let (mut refusal, graph) = match loaded {
        Ok(Some(mut graph)) => {
            graph.registered_startup = boxed_startup;
            site_probes.record(&mut graph.inputs);
            let initializer = check_initializer(&graph, registered_initializer);
            let call_error = initializer
                .as_ref()
                .and_then(analysis::InitializerCheck::fixed_error);
            if call_error.is_some() {
                // Both representations reject this initializer. Check it in the ordinary
                // layout once so an earlier annotated failure keeps its original priority.
                graph.layout_hint.set(modules::LayoutHint::Disabled);
            }
            let baseline = graph.lowering_baseline();
            let refusal = loop {
                let error = match lower(&mut graph, call_error)? {
                    CheckedProgram::Registered(reason) => break reason,
                    CheckedProgram::Native(mut fixed) => {
                        let (module, owners) = &mut fixed.ir;
                        verify_after(module, "lowering")?;
                        let Some(output) = output.as_deref_mut() else {
                            native_includes()?;
                            return Ok(graph.inputs);
                        };
                        match llvm::analyze_program(module) {
                            Ok(program) => {
                                for original in specialize(module)? {
                                    owners.push(owners[original]);
                                }
                                output.emit_fixed(
                                    &graph,
                                    &mut fixed,
                                    options.address_sanitizer,
                                    program,
                                );
                                native_includes()?;
                                return Ok(match entry {
                                    ProgramEntry::Source(_) => SourceInputs::default(),
                                    ProgramEntry::File(..) => output
                                        .compilation
                                        .native_inputs(graph, options.entry_profile.as_deref()),
                                });
                            }
                            Err(reason) => {
                                // Placement needs the original checked operations and their source sites.
                                fixed.register_definitions(&graph);
                                LoweringDiagnostic::plain(reason.0.to_owned()).classified(reason.1)
                            }
                        }
                    }
                    CheckedProgram::Runtime(error) => error,
                };
                // A guessed layout must not turn a working object program into a type error,
                // or mask a diagnostic that the ordinary checked-file path would have issued.
                if graph.layout_hint.get() != modules::LayoutHint::Applied {
                    break Some(error);
                }
                graph.retry_without_layout_hints(&baseline);
            };
            // Incremental lowering can replace the entry's type database with a cached session.
            graph.closed_initializer =
                initializer.filter(|check| !incremental && !check.read_import_binding);
            (refusal, Some(graph))
        }
        Ok(None) => (
            Some("the module launcher requires its registered sources".into()),
            None,
        ),
        Err(error) => (Some(error.into()), None),
    };
    // Admit an unresolved fixed signature only when its own definition has checked IR
    // for the registered calling convention. Other inference diagnostics still apply.
    if let Some(error) = &mut refusal
        && let LoweringKind::UnresolvedParameter(owner, start) = error.kind
        && let Some(graph) = graph.as_ref()
        && graph
            .registered_functions()
            .get(&owner)
            .is_some_and(|functions| functions.contains_key(&start))
    {
        error.kind = LoweringKind::ScopedObject;
    }
    let checked_entry = refusal.is_none();
    let kind = refusal
        .as_ref()
        .map_or(LoweringKind::Ordinary, |error| error.kind);
    let refusal_message = || {
        refusal.as_ref().map_or_else(
            || "the checked program requires a registered entry".to_owned(),
            ToString::to_string,
        )
    };
    // A newly found checked provider can lower normally. If lowering fails, its
    // dependencies alone do not admit the caller to the object importer.
    let fallback = |graph: Option<modules::SourceGraph>| {
        if let (Some(graph), LoweringKind::ExecutionOperand(id)) = (graph, kind)
            && graph.modules[id].is_program_source()
        {
            // Native lowering can fail before the file has a runtime registration.
            // Check its existing contracts before diagnosing the failed call's source.
            if let Some(contract) = &graph.source_contract {
                contract.original(&graph).clone()?;
            }
            graph
                .session
                .check_execution_escapes(std::iter::once((id, graph.modules[id].path.as_path())))?;
        }
        Err(refusal_message())
    };
    let mut checked_graph = graph;
    let graph = checked_graph
        .as_ref()
        .filter(|graph| !graph.has_checked_only_import);
    if options.entry_profile.is_some()
        || (checked_file
            && graph.as_ref().is_some_and(|g| {
                matches!(
                    kind,
                    LoweringKind::ReturnType | LoweringKind::DeclaredArgument
                ) || g.imports_struct_decorator()
            }))
    {
        return fallback(checked_graph);
    }
    let operation = graph.as_ref().and_then(|g| {
        if checked_entry {
            Some(dynamic::ObjectOperation::CheckedEntry)
        } else if kind.is_class_layout() {
            Some(dynamic::ObjectOperation::ClassLayout)
        } else {
            (kind.checks_object_calls()
                || g.has_object_constructor_fallback.get() && kind != LoweringKind::ReturnType)
                .then_some(dynamic::ObjectOperation::Builtin)
        }
    });
    let ordered_command_import = options.command.is_some()
        && import_failure == Some(modules::EntryImportFailure::AfterStatement);
    let object_imports = options.module_entry.is_some()
        || import_failure == Some(modules::EntryImportFailure::ZipImport)
        || (options.sitecustomize
            && import_failure == Some(modules::EntryImportFailure::MissingModule))
        || operation.is_some()
        || graph
            .as_ref()
            .is_some_and(|g| g.needs_object_runtime.get() || g.traceback_objects);
    let check_returns = |graph: Option<&mut modules::SourceGraph>,
                         entry_requests_objects: bool,
                         registrations: Option<&dynamic::RegisteredSources<'_>>|
     -> Result<(), String> {
        if let Some(graph) = graph.filter(|g| !g.has_checked_only_import) {
            let requested = graph.check_fallback_calls.get();
            // A runtime class export can admit a new import graph. Keep direct source-call
            // diagnostics unless this entry already requested Python object semantics.
            let check_class_import = graph.imports_runtime_class && !entry_requests_objects;
            if !checked_file
                && !check_class_import
                && !matches!(requested, analysis::FallbackCalls::FunctionBodies)
            {
                return Ok(());
            }
            let replaced = registrations.filter(|r| !r.provider_sources.is_empty());
            let body_checks = graph.registered_source_errors.borrow();
            // A clean body check covers the fallback lints on the original bindings.
            // Replacing a dependency also changes the contract of its callers.
            let checked_source = |id: usize| {
                !incremental
                    && replaced.is_none()
                    && graph.modules[id].checks_source_contracts()
                    && body_checks.get(&id).is_some_and(Vec::is_empty)
            };
            let (skipped, overloads) = graph.diagnostic_scopes();
            let entry = &graph.modules[graph.entry];
            let all_calls = !matches!(requested, analysis::FallbackCalls::None)
                || checked_file && graph.has_object_constructor_fallback.get()
                || !entry.optional_imports.is_empty()
                || !graph.needs_object_runtime.get() && kind.checks_object_calls()
                || checked_file && graph.uses_source_loader();
            let checked_calls = if !checked_file {
                if check_class_import
                    && !matches!(requested, analysis::FallbackCalls::FunctionBodies)
                {
                    analysis::FallbackCalls::NamedFunctions
                } else {
                    requested
                }
            } else if graph.check_method_factories.get() {
                analysis::FallbackCalls::DecoratorFactories
            } else if all_calls {
                analysis::FallbackCalls::All
            } else if check_class_import {
                analysis::FallbackCalls::NamedFunctions
            } else if !checked_source(graph.entry) && entry.has_entry_guard() {
                analysis::FallbackCalls::Entry(graph.entry)
            } else {
                analysis::FallbackCalls::None
            };
            let check = |session: &mut analysis::Session| {
                session.check_fallback_contracts(
                    skipped.as_deref(),
                    &overloads,
                    checked_calls,
                    // A replaced adapter is an import dependency. Its checked spans
                    // and layout errors belong to the original fixed source.
                    (0..graph.modules.len()).filter(|id| {
                        !checked_source(*id)
                            && replaced.is_none_or(|r| !r.provider_sources.contains_key(id))
                    }),
                    checked_file.then_some(&graph.modules),
                )
            };
            if incremental || replaced.is_some() {
                check(&mut graph.session.copy_for_diagnostics(None, replaced)?)?;
            } else {
                graph.session.with_diagnostic_rules(check)?;
            }
        }
        Ok(())
    };
    let check = |mut checked_graph: Option<&mut modules::SourceGraph>,
                 entry_requests_objects,
                 registered: Option<dynamic::RegisteredSources<'_>>| {
        check_returns(
            checked_graph.as_deref_mut(),
            entry_requests_objects,
            registered.as_ref(),
        )?;
        if let Some(mut registered) = registered {
            let layouts = analysis::check_registered_project(
                registered.root,
                registered.roots,
                registered.winners.iter().copied(),
                registered.has_file_body,
                checked_graph
                    .as_deref()
                    .map(|g| (g, &registered.provider_sources)),
                refusal.as_ref(),
            )?;
            registered.layouts.extend(layouts);
            if registered
                .file_alternatives
                .iter()
                .any(|source| source.check_execution)
            {
                // Check a linked file provider with its own bindings. The ordinary program
                // still imports the frozen definitions in the check above.
                let mut files: Vec<_> = registered
                    .winners
                    .iter()
                    .map(|source| dynamic::RegisteredSource {
                        is_program: false,
                        declares_signatures: false,
                        initial: None,
                        check_execution: false,
                        ..*source
                    })
                    .collect();
                files.extend_from_slice(registered.file_alternatives);
                if let Some(graph) = checked_graph.as_deref() {
                    registered.provider_sources.retain(|&id, _| {
                        !registered
                            .file_alternatives
                            .iter()
                            .any(|file| file.name == graph.modules[id].name)
                    });
                }
                let layouts = analysis::check_registered_project(
                    registered.root,
                    registered.roots,
                    files,
                    registered.has_file_body,
                    checked_graph
                        .as_deref()
                        .map(|g| (g, &registered.provider_sources)),
                    None,
                )?;
                registered.layouts.extend(layouts);
            }
        }
        Ok(())
    };
    // The checked loader can discover the package before an import requires object lowering.
    let entry = match entry {
        ProgramEntry::File(path, None) => {
            ProgramEntry::File(path, site_probes.package_root.as_deref())
        }
        entry => entry,
    };
    match dynamic::compile(
        (
            entry,
            site_probes.packaged_file_module.as_deref(),
            filesystem,
            &mut site_probes.wheel_records,
        ),
        options,
        object_imports,
        operation
            .or(ordered_command_import.then_some(dynamic::ObjectOperation::OrderedCommandImport)),
        &mut checked_graph,
        output,
        check,
    )? {
        Some(mut inputs) => {
            site_probes.record(&mut inputs);
            Ok(inputs)
        }
        None => fallback(checked_graph),
    }
}

/// Returns the native units and, when requested, a separate whole-program LLVM dump.
pub fn compile_file_with_layout(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    layout: CodegenLayout,
) -> Result<(UnitCompilation, Option<String>), String> {
    compile_with_layout(ProgramEntry::File(path, module_root), options, layout, None)
}

/// Compiles one entry module and its statically resolved first-party dependencies.
pub fn compile_file(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<Compilation, String> {
    compile_file_with_layout(path, module_root, options, CodegenLayout::Whole)
        .map(|(compilation, _)| compilation.into_whole())
}

/// Preserves the whole-program IR and returns independent boxed initializers for native codegen.
pub fn compile_file_with_boxed_units(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<(Compilation, Vec<CodegenUnit>), String> {
    let (compilation, whole) =
        compile_file_with_layout(path, module_root, options, CodegenLayout::SplitWithWhole)?;
    Ok(compilation.into_whole_and_units(whole))
}

/// Keeps typed programs whole and emits independent boxed initializers for ThinLTO.
pub fn compile_file_for_thin_lto(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<UnitCompilation, String> {
    compile_file_with_layout(path, module_root, options, CodegenLayout::ThinLto)
        .map(|(compilation, _)| compilation)
}

/// Resolves and checks the source graph once for the requested outputs.
fn compile_with_layout(
    entry: ProgramEntry<'_>,
    options: &CompileOptions,
    layout: CodegenLayout,
    mut cache: Option<&mut IncrementalCompiler>,
) -> Result<(UnitCompilation, Option<String>), String> {
    let mut output = ProgramOutput::new(layout);
    output.compilation.source_inputs = object_fallback(
        Some(&mut output),
        entry,
        options,
        Default::default(),
        cache.is_some(),
        |graph, call_error| checked_lower_with(graph, cache.as_deref_mut(), call_error),
    )?;
    Ok((output.compilation, output.whole))
}

pub struct CodegenUnit {
    pub name: String,
    pub llvm_ir: String,
}

impl ProgramOutput {
    fn new(layout: CodegenLayout) -> Self {
        Self {
            layout,
            compilation: UnitCompilation::default(),
            whole: None,
        }
    }

    /// Place each local source in program order, then finish the startup and its
    /// dependencies. A failed fixed emission leaves the registered retry untouched.
    fn emit_sources<'a, S, W, E>(
        &mut self,
        sources: impl IntoIterator<Item = Result<S, E>>,
        entry: (usize, &str),
        split: bool,
        mut writer: W,
        mut emit: impl FnMut(&mut W, usize, S, bool) -> Result<(&'a str, Option<String>), E>,
        finish: impl FnOnce(W, &mut [CodegenUnit], usize) -> Result<Option<String>, E>,
    ) -> Result<(), E> {
        let mut units = Vec::new();
        let mut startup = None;
        let mut sources = sources.into_iter().enumerate();
        while let Some((id, source)) = sources.next() {
            let is_entry = id == entry.0;
            let separate = split && !is_entry;
            let (name, body) = match emit(&mut writer, id, source?, separate) {
                Ok(placed) => placed,
                Err(error) => {
                    // Preparation errors precede an emission refusal in an earlier module.
                    sources.try_for_each(|(_, source)| source.map(drop))?;
                    return Err(error);
                }
            };
            let (name, body) = if is_entry {
                startup = Some(units.len());
                (entry.1, body.unwrap_or_default())
            } else if let Some(body) = body {
                debug_assert!(separate);
                (name, body)
            } else {
                continue;
            };
            units.push(CodegenUnit::new(name, body));
        }
        let entry = startup.expect("the checked sources include the program entry");
        let whole = finish(writer, &mut units, entry)?;
        self.compilation.units = units;
        self.whole = whole;
        Ok(())
    }
}

impl CodegenUnit {
    fn new(name: &str, llvm_ir: String) -> Self {
        Self {
            name: name.to_owned(),
            llvm_ir,
        }
    }
}

#[derive(Default)]
pub struct UnitCompilation {
    pub units: Vec<CodegenUnit>,
    pub input_paths: Vec<PathBuf>,
    pub source_inputs: SourceInputs,
    /// Native archives reached by the static import graph, in link order.
    pub extension_archives: Vec<PathBuf>,
}

impl UnitCompilation {
    fn native_inputs(
        &mut self,
        mut graph: modules::SourceGraph,
        profile: Option<&Path>,
    ) -> SourceInputs {
        let (inputs, paths) = graph.disk_inputs(profile);
        self.input_paths = paths;
        self.extension_archives = graph
            .modules
            .into_iter()
            .filter_map(|module| Some(module.archive?.identity))
            .collect();
        inputs
    }

    fn into_whole(self) -> Compilation {
        self.into_whole_and_units(None).0
    }

    fn into_whole_and_units(mut self, whole: Option<String>) -> (Compilation, Vec<CodegenUnit>) {
        let llvm_ir = whole.unwrap_or_else(|| {
            self.units
                .pop()
                .expect("whole program has one unit")
                .llvm_ir
        });
        let compilation = Compilation {
            llvm_ir,
            input_paths: self.input_paths,
            source_inputs: self.source_inputs,
            extension_archives: self.extension_archives,
        };
        (compilation, self.units)
    }
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
        compile_with_layout(
            ProgramEntry::File(path, module_root),
            options,
            CodegenLayout::Split,
            Some(self),
        )
        .map(|(compilation, _)| compilation)
    }
}

/// Keeps whole-program checking and specialization, then partitions native code by source owner.
pub fn compile_file_units(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
) -> Result<UnitCompilation, String> {
    compile_file_with_layout(path, module_root, options, CodegenLayout::Split)
        .map(|(compilation, _)| compilation)
}

type CheckedIr = (ir::Module, Vec<modules::ModuleId>);

type FixedFrameSources = Vec<(usize, ruff_text_size::TextRange, frontend::FixedBodySource)>;

pub(crate) struct FixedProgram {
    ir: CheckedIr,
    /// Original binder and operation sites indexed by checked function ID.
    frame_sources: FixedFrameSources,
    uses_builtin_module_names: bool,
}

impl FixedProgram {
    fn prepare_native_sources(&mut self, graph: &modules::SourceGraph) {
        // The registered launcher inventories its own winning providers. Only a
        // committed native entry needs the fixed graph's built-in module list.
        if self.uses_builtin_module_names {
            self.ir.0.builtin_module_sources = Some(dynamic::builtin_inventory_sources(graph));
        }
    }

    fn register_definitions(self, graph: &modules::SourceGraph) {
        let (module, owners) = self.ir;
        frontend::FixedFunctions {
            functions: module.functions,
            frames: self.frame_sources,
        }
        .register(graph, &owners, |_, _| true);
    }
}

/// A checked entry selects its startup. A representation refusal still needs runtime admission.
pub(crate) enum CheckedProgram {
    /// A checked Python frame or launcher can enter directly. A specific operation or
    /// unresolved import retains its own admission policy and diagnostic.
    Registered(Option<LoweringDiagnostic>),
    Native(Box<FixedProgram>),
    Runtime(LoweringDiagnostic),
}

#[cfg(test)]
impl CheckedProgram {
    fn into_native(self, graph: &modules::SourceGraph) -> Result<CheckedIr, String> {
        match self {
            Self::Native(mut fixed) => {
                fixed.prepare_native_sources(graph);
                Ok(fixed.ir)
            }
            Self::Runtime(reason) => Err(reason.to_string()),
            Self::Registered(Some(reason)) => Err(reason.to_string()),
            Self::Registered(None) => Err("the checked program requires a registered entry".into()),
        }
    }
}

#[cfg(test)]
fn checked_lower(graph: &mut modules::SourceGraph) -> Result<CheckedProgram, String> {
    let initializer = check_initializer(graph, false);
    let error = initializer
        .as_ref()
        .and_then(analysis::InitializerCheck::fixed_error);
    checked_lower_with(graph, None, error)
}

fn check_initializer(
    graph: &modules::SourceGraph,
    registered_entry: bool,
) -> Option<analysis::InitializerCheck> {
    analysis::ClosedSource::project_path(&graph.modules[graph.entry])
        .filter(|_| !registered_entry)
        .map(|entry| {
            // Python startup and -m have a separately selected registration.
            let runs_as_main = graph.command_startup.is_none()
                && graph.python_file("__main__") == Some(graph.entry);
            graph
                .session
                .initializer_call_error((graph.entry, entry), runs_as_main, |name| {
                    let id = graph.python_file(name)?;
                    Some((
                        id,
                        analysis::ClosedSource::project_path(&graph.modules[id])?,
                    ))
                })
        })
}

fn checked_lower_with(
    graph: &mut modules::SourceGraph,
    cache: Option<&mut IncrementalCompiler>,
    call_error: Option<&LoweringDiagnostic>,
) -> Result<CheckedProgram, String> {
    let retry_with_cached_hints =
        cache.is_some() && graph.layout_hint.get() == modules::LayoutHint::Disabled;
    let mut cache = cache.filter(|_| !retry_with_cached_hints);
    // A retry shares the quiet hint snapshot. Use its independent diagnostic session for
    // both the pending generator declarations and the subsequent closed-program check.
    let mut retry_checks = (retry_with_cached_hints && call_error.is_none())
        .then(|| graph.session.diagnostic_copy())
        .transpose()?;
    let lowered = if let Some(compiler) = cache.as_deref_mut() {
        let mut fresh = compiler.cache.restore_hints(graph)?;
        let callers = compiler.callers.prepare(graph);
        let result = frontend::lower_with_caller_cache(
            graph,
            callers,
            fresh.as_mut().map(frontend::SourceCheckSession::Borrowed),
        );
        compiler.cache.retain_hints(graph, result.is_ok());
        result
    } else {
        let source_checks = retry_checks
            .as_mut()
            .map(frontend::SourceCheckSession::Borrowed)
            .or_else(|| {
                retry_with_cached_hints.then_some(frontend::SourceCheckSession::SharedGraph)
            });
        frontend::lower_with_caller_cache(graph, None, source_checks)
    };
    // Registered-body admission runs after checking the application's source contracts.
    if let Some(error) = call_error {
        // Both representations reject this call. Preserve a prior annotated error at
        // its original checked location even when this file contains several failures.
        let diagnostic = match lowered {
            Err(prior) if prior.earlier_contract(error) => prior,
            _ => error.clone(),
        };
        if let Some(compiler) = cache {
            compiler.callers = frontend::CallerCache::default();
        }
        return Ok(CheckedProgram::Runtime(diagnostic));
    }
    // The file driver discards a failed speculative lowering before admission.
    // Only the ordinary attempt needs its closed-program diagnostic checks.
    if graph.layout_hint.get() == modules::LayoutHint::Applied
        && let Err(reason) = lowered
    {
        if let Some(compiler) = cache {
            compiler.callers = frontend::CallerCache::default();
        }
        return Ok(CheckedProgram::Runtime(reason));
    }
    if cache.is_none()
        && !retry_with_cached_hints
        && let Err(reason) = lowered
    {
        graph.session.set_closed_program_rules();
        graph.source_contract = Some(analysis::SourceContract::Pending {
            native: Box::new(reason.clone()),
            result: Default::default(),
        });
        return Ok(CheckedProgram::Runtime(reason));
    }
    let (skipped, overloads) = graph.diagnostic_scopes();
    let skipped = skipped.as_deref();
    // A fully lowered graph has already checked the actual first-party definitions.
    // Keep its existing diagnostics and proof scopes. When a representation is
    // unavailable, the declared Python call contracts still apply to the graph.
    let session = retry_checks.as_mut().unwrap_or(&mut graph.session);
    let result = if lowered.is_ok() {
        lowered.and_then(|ir| {
            if let Some(compiler) = cache.as_deref_mut() {
                compiler
                    .cache
                    .check_scopes(session, &graph.inputs, skipped, &overloads)?;
            } else {
                session.check_scopes(skipped, &overloads)?;
            }
            graph.source_contract = Some(analysis::SourceContract::Checked(Ok(())));
            Ok(ir)
        })
    } else {
        let native = lowered.as_ref().err();
        let checked = if let Some(compiler) = cache.as_deref_mut() {
            compiler.cache.check_closed_program(
                session,
                &graph.inputs,
                &graph.modules,
                skipped,
                &overloads,
                native,
            )
        } else {
            session.check_closed_program(&graph.modules, skipped, &overloads, native)
        };
        // An inapplicable object representation cannot replace a native source or ABI
        // diagnostic. A real registration checks this result before emitting its body.
        graph.source_contract = Some(analysis::SourceContract::Checked(checked));
        lowered
    };
    match result {
        Ok(program) => Ok(program),
        Err(reason) => {
            if let Some(compiler) = cache {
                compiler.callers = frontend::CallerCache::default();
            }
            Ok(CheckedProgram::Runtime(reason))
        }
    }
}

fn specialize(module: &mut ir::Module) -> Result<Vec<usize>, String> {
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
    DiskFile,
    File,
    DecodedFile,
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
    // String-backed file inputs retain their UTF-8 contract. Disk files are already decoded.
    if kind == SourceKind::File
        && let Some(encoding) = imre_ast::source_encoding::declared_encoding(&source)
        && !matches!(
            encoding.to_ascii_lowercase().as_str(),
            "utf-8" | "utf_8" | "utf8"
        )
    {
        return Err(format!(
            "unsupported source encoding {encoding:?}; use utf-8"
        ));
    }
    Ok(source)
}

struct DiskSource {
    text: String,
    raw_digest: Option<[u8; 32]>,
}

impl DiskSource {
    /// Keep the read buffer when the decoder borrowed all of it. A different
    /// spelling, including a stripped BOM, needs the physical registration digest.
    fn decode(path: &Path, bytes: Vec<u8>) -> Result<Self, String> {
        use imre_ast::source_encoding::{SourceDecodeError, decode_disk_source};
        use sha2::{Digest, Sha256};
        let (text, original) = decode_disk_source(bytes, path).map_err(|error| {
            let path = path.display();
            match error {
                SourceDecodeError::Invalid(msg, Some(range)) => {
                    format!("  File \"{path}\", line {}\nSyntaxError: {msg}", range.line)
                }
                SourceDecodeError::Invalid(msg, None) | SourceDecodeError::NonText(msg) => {
                    format!("{path}: SyntaxError: {msg}")
                }
                SourceDecodeError::Null => format!("{path}: source code cannot contain null bytes"),
                SourceDecodeError::Unsupported(codec) => {
                    format!("{path}: native source decoder for '{codec}' is unavailable")
                }
                SourceDecodeError::Allocation => format!("{path}: out of memory decoding source"),
            }
        })?;
        let raw_digest = original.map(|raw| Sha256::digest(raw).into());
        Ok(Self { text, raw_digest })
    }
}

#[cfg(test)]
mod tests {
    use super::{CompileOptions, compile};

    #[test]
    fn preparation_errors_precede_earlier_emission_errors() {
        let mut output = super::ProgramOutput::new(super::CodegenLayout::Split);
        let result = output.emit_sources(
            [Ok("entry"), Err("invalid second"), Err("invalid third")],
            (0, "__main__"),
            true,
            (),
            |_, _, _, _| Err("cannot emit entry"),
            |_, _, _| unreachable!(),
        );
        assert_eq!(result, Err("invalid second"));
        assert!(output.compilation.units.is_empty());
    }

    #[test]
    fn selected_disk_source_uses_physical_bytes_and_decoded_spans() {
        use ruff_text_size::Ranged;
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.py");
        let raw = b"#!/usr/bin/python\r# coding: latin-1\rvalue = '\xe9'; other = 1";
        std::fs::write(&path, raw).unwrap();
        let graph = crate::modules::SourceGraph::load(&path, None, &[]).unwrap();
        let module = &graph.modules[graph.entry];
        let second = module.parsed.syntax().body[1].range();
        assert_eq!(&module.source[second], "other = 1");
        assert!(module.source.ends_with("value = 'é'; other = 1"));
        assert_eq!(module.raw_digest, Some(Sha256::digest(raw).into()));
        assert_eq!(
            graph.inputs.files[graph.entry].2,
            blake3::hash(raw).to_hex().as_str()
        );
        for raw in ["s = 'π'\n", "s = 'π'\r\n", "# coding: utf_8\rs = 'π'"] {
            let bytes = raw.as_bytes().to_vec();
            let allocation = bytes.as_ptr();
            let capacity = bytes.capacity();
            let decoded = super::DiskSource::decode(&path, bytes).unwrap();
            assert_eq!(decoded.text.as_ptr(), allocation);
            assert_eq!(
                (
                    decoded.text.as_str(),
                    decoded.text.capacity(),
                    decoded.raw_digest
                ),
                (raw, capacity, None)
            );
        }
        let err = super::DiskSource::decode(&path, b"\xef\xbb\xbf# coding: utf8\n#\xff".to_vec())
            .err()
            .unwrap();
        assert_eq!(
            err,
            format!(
                "  File \"{}\", line 1\nSyntaxError: encoding problem: utf8 with BOM",
                path.display()
            )
        );
    }

    #[test]
    fn a_registered_call_preserves_later_operand_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app.py");
        let source = r#"def publish(a: int, b: int, send):
    total = a + b
    send(total)


def report(value):
    print(value)


handlers = {"callback": report}
sink = handlers[input()]
publish(3, 5, sink)
values: list[int] = [3]
checked_offset = 17
values.append(True)
print(values)
"#;
        std::fs::write(&app, source).unwrap();
        let error = super::check_file(&app, None).unwrap_err();
        assert!(
            error.contains("app.py:15:15: Imre: expected int, got bool"),
            "{error}"
        );
        std::fs::write(&app, source.replace("append(True)", "append(4)")).unwrap();
        super::check_file(&app, None).unwrap();
        let selected = source
            .replace(
                "publish(3, 5, sink)",
                r#"{"publish": publish}[input()](3, 5, sink)"#,
            )
            .replace("append(True)", "append(4)");
        std::fs::write(&app, selected).unwrap();
        super::check_file(&app, None).unwrap();
    }

    #[test]
    fn a_replaced_callable_can_supply_an_operand_to_print() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("app.py");
        let source = r#"def add(a: int, b: int) -> int:
    return a + b

def retire(self):
    globals()["call"] = add

previous = type("Previous", (), {"__del__": retire})
globals()["call"] = previous()
call = add
print("result", call(b=2, a=1))
"#;
        std::fs::write(&entry, source).unwrap();
        super::check_file(&entry, None).unwrap();
        let invalid = source.replace("a=1))", "a=1), invalid=True)");
        std::fs::write(&entry, invalid).unwrap();
        let error = super::check_file(&entry, None).unwrap_err();
        assert!(
            error.contains("keyword arguments are unsupported for this builtin"),
            "{error}"
        );
    }

    #[test]
    fn a_namespace_store_can_run_a_finalizer_after_installing_the_new_value() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("app.py");
        let source = include_str!("../../../tests/fixtures/registered_scope_replacement.py");
        let install = "globals()[\"values\"] = previous()";
        for statement in [
            install,
            "scope = globals()\nkey = 'values'\nscope[key] = previous()",
        ] {
            std::fs::write(&entry, source.replace(install, statement)).unwrap();
            super::check_file(&entry, None).unwrap();
        }
        for call in [
            "values.append(value=True)",
            "values.append(*[True])",
            "values.append(**{'value': True})",
        ] {
            std::fs::write(&entry, source.replace("values.append(True)", call)).unwrap();
            super::check_file(&entry, None).unwrap();
        }
        let nested = format!(
            "def take(value: int) -> int:\n    return value\n\n{}",
            source.replace("values.append(True)", "values.append(value=take('x'))")
        );
        std::fs::write(&entry, nested).unwrap();
        let error = super::check_file(&entry, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
        for statement in ["", &format!("if False:\n    {install}")] {
            std::fs::write(&entry, source.replace(install, statement)).unwrap();
            let error = super::check_file(&entry, None).unwrap_err();
            assert!(error.contains("expected int, got bool"), "{error}");
        }
    }

    #[test]
    fn registered_reduction_specializes_multiply_in_its_python_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("paired.py");
        std::fs::write(
            &path,
            include_str!("../../../tests/fixtures/registered_reduction_callback.py"),
        )
        .unwrap();
        let ir = super::compile_file(&path, None, &CompileOptions::default())
            .unwrap()
            .llvm_ir;
        let constant = |suffix: &str| {
            ir.lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(" = private unnamed_addr constant ")?;
                    value.ends_with(suffix).then_some(name)
                })
                .unwrap()
        };
        let file = constant(r#"paired.py\00""#);
        let body_at = |line| {
            let location = format!("ptr {file}, i32 {line}, i64 0)");
            let mut definitions = ir
                .split("\ndefine internal i64 @code")
                .skip(1)
                .filter_map(|entry| entry.split_once("\n}").map(|(body, _)| body))
                .filter(|body| body.contains(&location));
            let definition = definitions
                .next()
                .expect("statement in registered function");
            assert!(definitions.next().is_none(), "duplicate statement body");
            definition
        };
        let reduction = body_at(2);
        assert_eq!(reduction.matches("call i64 @int_mul(").count(), 1);
        assert!(reduction.contains("call i64 @imre_dyn_exact_int_word("));
        let multiply = constant(r#"c"*\00""#);
        assert!(reduction.contains(&format!(
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {multiply},"
        )));

        let publish = body_at(7);
        assert_eq!(publish.matches("call i64 @int_add(").count(), 1);
        assert_eq!(
            publish
                .matches("call i64 @imre_dyn_positional_call_at(")
                .count(),
            1
        );
    }

    #[test]
    fn a_registered_integer_subclass_can_supply_its_compiled_callback() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("app.py");
        std::fs::write(
            &entry,
            include_str!("../../../tests/fixtures/registered_add_callback.py"),
        )
        .unwrap();
        super::check_file(&entry, None).unwrap();
    }

    #[test]
    fn a_bound_callback_call_can_use_the_registered_definition() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("app.py");
        std::fs::write(&entry, "import operations\n").unwrap();
        std::fs::write(
            root.path().join("operations.py"),
            "def apply(send):\n    items: list[int] = [1]\n    items.append(7)\n    send(1)\ndef report(value):\n    print(value)\nhandlers = {'cb': report}\napply(handlers[input()])\n",
        )
        .unwrap();
        super::check_file(&entry, None).unwrap();
    }

    #[test]
    fn inference_outcomes_survive_a_directory_rename() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("ordinary");
        let renamed = root
            .path()
            .join("inference is waiting for a function return type");
        std::fs::create_dir(&original).unwrap();
        std::fs::write(
            original.join("display.py"),
            "def unresolved(arg):\n    return arg\n\ndef display() -> str:\n    return (1.25).hex()\n\nprint(display())\n",
        )
        .unwrap();
        std::fs::write(
            original.join("saved.py"),
            "def waiting():\n    return waiting()\n\ndef witness() -> bool:\n    return isinstance(3, waiting())\n\nprint('steady')\n",
        )
        .unwrap();
        std::fs::write(
            original.join("recursive.py"),
            "def waiting():\n    return waiting()\n\nwaiting()\n",
        )
        .unwrap();

        // An operand may wait even though the enclosing expression replaces its result.
        std::fs::write(
            original.join("replace.py"),
            "def waiting():\n    return waiting()\n\ndef witness() -> str:\n    return 'x'.replace(waiting(), 'y')\n\nprint('steady')\n",
        )
        .unwrap();
        std::fs::write(
            original.join("stalled_forwards.py"),
            concat!(
                "def left(x: int):\n    return right(x)\n\n",
                "def right(x: int):\n    return left(x)\n\n",
                "def forward(x: int):\n    return leaf(x)\n\n",
                "def leaf(x: int):\n    return x + 1\n\n",
                "def bad(x: int):\n    return unknown\n\n",
                "print(forward(3))\n",
            ),
        )
        .unwrap();
        let check = |directory: &std::path::Path| {
            let program = super::compile_file(
                &directory.join("display.py"),
                None,
                &CompileOptions::default(),
            )
            .unwrap();
            assert!(program.llvm_ir.contains("call ptr @imre_dyn_new("));
            super::check_file(&directory.join("replace.py"), None).unwrap();
            for (file, diagnostic) in [
                (
                    "stalled_forwards.py",
                    "14:12: Imre: name is not a local value; globals and first-class functions are unsupported",
                ),
                (
                    "saved.py",
                    "5:26: Imre: isinstance requires supported static classes",
                ),
                (
                    "recursive.py",
                    "1:1: Imre: cannot infer function return type; add an annotation; recursive calls remain unsupported",
                ),
            ] {
                let path = directory.join(file).canonicalize().unwrap();
                let error = super::check_file(&path, None).unwrap_err();
                assert_eq!(error, format!("{}:{diagnostic}", path.display()));
            }
        };
        check(&original);
        std::fs::rename(&original, &renamed).unwrap();
        check(&renamed);
    }

    #[test]
    fn comments_do_not_change_normalized_provider_imports() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("main.py");
        let source = "import ｊｓｏｎ\nprint(ｊｓｏｎ.loads('42'))\n";
        for comment in ["", "# code\n"] {
            std::fs::write(&entry, format!("{source}{comment}")).unwrap();
            super::check_file(&entry, None).unwrap();
            super::compile_file(&entry, None, &CompileOptions::default()).unwrap();
        }

        std::fs::write(
            root.path().join("scores.py"),
            "def twice(x: int) -> int:\n    return x + x\n\ndef total() -> int:\n    return twice('x')\n",
        )
        .unwrap();
        let source = "import ｊｓｏｎ\nimport scores\nif False:\n    print(ｊｓｏｎ.loads('42'))\nprint(scores.total())\n";
        let errors = ["", "# code\n"].map(|comment| {
            std::fs::write(&entry, format!("{source}{comment}")).unwrap();
            super::check_file(&entry, None).unwrap_err()
        });
        assert!(errors[0].contains("scores.py:5:18"), "{}", errors[0]);
        assert_eq!(errors[0], errors[1]);
    }

    #[test]
    fn dictionary_set_construction_allows_native_and_object_printing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        let keys = "keys = set({'first': 1, 'second': 2})";
        for value in ["[3, 7]", "{'values': [3, 7]}"] {
            let source = format!("{keys}\nprint({value}, len(keys), 'second' in keys)\n");
            std::fs::write(&path, source).unwrap();
            super::compile_file(&path, None, &CompileOptions::default()).unwrap();
        }
    }

    #[test]
    fn module_list_writes_respect_the_element_declaration() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("app.py");
        let body = "def record():\n    events.append('rhs')\n";
        for declaration in ["events = []", "events: list[str] = []"] {
            std::fs::write(
                &path,
                format!("{declaration}\n{body}record()\nprint(events)\n"),
            )
            .unwrap();
            super::check_file(&path, None).unwrap();
        }

        std::fs::write(&path, format!("events: list[int] = []\n{body}")).unwrap();
        let error = super::check_file(&path, None).unwrap_err();
        assert!(
            error.contains("app.py:3:19: Imre: expected int, got str"),
            "{error}"
        );
    }

    #[test]
    fn mixed_appends_keep_the_original_module_list_layout() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("app.py");
        for (first, second, line) in [("7", "'rhs'", 7), ("'rhs'", "7", 4)] {
            let source = format!(
                "events = []\n\ndef first():\n    events.append({first})\n\ndef second():\n    events.append({second})\n\nfirst()\nsecond()\n"
            );
            std::fs::write(&path, source).unwrap();
            let error = super::check_file(&path, None).unwrap_err();
            assert!(
                error.contains(&format!("app.py:{line}:19: Imre: expected int, got str")),
                "{error}"
            );
        }
    }

    #[test]
    fn indexed_store_keeps_the_original_module_list_layout() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("app.py");
        std::fs::write(
            &path,
            "events = []\n\ndef replace():\n    events[0] = 7\n\ndef add():\n    events.append('rhs')\n\nadd()\nreplace()\n",
        )
        .unwrap();
        let error = super::check_file(&path, None).unwrap_err();
        assert!(
            error.contains("app.py:7:19: Imre: expected int, got str"),
            "{error}"
        );
    }

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

        // The same object route must preserve annotated return errors.
        std::fs::write(
            &path,
            "def build() -> int:\n    buffer = bytearray()\n    return 'bad'\n\nprint(build())\n",
        )
        .unwrap();
        let error = super::check_file(&path, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }

    const INFERRED_SET_GATHER: &str = "def gather(text):\n    values = set()\n    for char in text:\n        values.add(char)\n    return sorted(values)\n";

    #[test]
    fn inferred_set_layout_preserves_checked_object_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        let mut incremental = super::IncrementalCompiler::default();
        for usage in [
            "values = gather('ab')\nvalues.append(1)\nprint(len(values))\n",
            "def integer(n: int) -> int:\n    return n\nprint(gather('a'), integer('x'))\n",
            "def wrong() -> int:\n    return 'x'\nprint(gather('a'), wrong())\n",
        ] {
            std::fs::write(&path, format!("{INFERRED_SET_GATHER}{usage}")).unwrap();
            let options = CompileOptions::default();
            let ordinary = super::object_fallback(
                None,
                super::ProgramEntry::File(&path, None),
                &options,
                Default::default(),
                false,
                |graph, call_error| {
                    graph.layout_hint.set(super::modules::LayoutHint::Disabled);
                    super::checked_lower_with(graph, None, call_error)
                },
            )
            .map(drop);
            assert_eq!(
                super::check_file_with_options(&path, None, &options),
                ordinary
            );
            assert_eq!(incremental.check_file(&path, None), ordinary);
        }
    }

    #[test]
    fn rejected_initializer_uses_the_ordinary_set_layout_once() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        std::fs::write(
            root.path().join("scores.py"),
            "def take(n: int) -> int:\n    return n\n",
        )
        .unwrap();
        let source = format!(
            "import scores\n{INFERRED_SET_GATHER}def size() -> int:\n    return len(gather('ab'))\nscores.take('x')\n"
        );
        std::fs::write(&path, source).unwrap();
        let mut passes = 0;
        let result = super::object_fallback(
            None,
            super::ProgramEntry::File(&path, None),
            &CompileOptions::default(),
            Default::default(),
            false,
            |graph, call_error| {
                passes += 1;
                assert!(graph.layout_hint.get() == super::modules::LayoutHint::Disabled);
                super::checked_lower_with(graph, None, call_error)
            },
        )
        .map(drop);
        assert_eq!(passes, 1);
        assert_eq!(
            result.unwrap_err(),
            format!(
                "{}:9:13: Imre: expected int, got str; annotations are enforced contracts",
                path.canonicalize().unwrap().display()
            )
        );
    }

    #[test]
    fn set_layout_retry_uses_the_resolved_source() {
        use super::modules::LayoutHint;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.py");
        let source = format!(
            "{INFERRED_SET_GATHER}values = gather('ab')\nvalues.append(1)\nprint(len(values))\n"
        );
        std::fs::write(&path, &source).unwrap();
        let options = CompileOptions::default();
        let expected = super::check_file(&path, None);
        let mut passes = Vec::new();
        let result = super::object_fallback(
            None,
            super::ProgramEntry::File(&path, None),
            &options,
            Default::default(),
            false,
            |graph, call_error| {
                assert_eq!(graph.modules[graph.entry].original_source(), source);
                graph.session.explicit_check = true;
                let result = super::checked_lower_with(graph, None, call_error)?;
                let applied = graph.layout_hint.get() == LayoutHint::Applied;
                passes.push(applied);
                if applied {
                    std::fs::write(&path, "invalid syntax!").unwrap();
                }
                Ok(result)
            },
        )
        .map(drop);
        assert_eq!(passes, [true, false]);
        assert_eq!(result, expected);
        let fresh = super::check_file(&path, None);
        assert!(fresh.is_err());
        assert_ne!(fresh, result);
    }

    #[test]
    fn mutable_document_arguments_preserve_independent_any_parameters() {
        let source = r#"from dataclasses import dataclass
from numbers import Number
from typing import Any
from tomli import loads
from tomli_w import dumps

@dataclass(kw_only=True, repr=False)
class Settings:
    table: dict[str, Any]

def is_number(value: Any) -> bool:
    return isinstance(value, Number)

def update(table: dict[str, Any]) -> dict[str, Any]:
    table["name"] = "new"
    return table

doc = loads('name = "old"')
settings = Settings(table=doc)
print(dumps(update(settings.table)))
print(dumps(doc))
print(is_number(3))
"#;
        let llvm = super::compile(source, &CompileOptions::default()).unwrap();
        assert!(llvm.contains("call i32 @imre_system_exit_code_kind("));
        assert!(llvm.contains("call i64 @imre_document("));
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
    fn file_iterator_classes_check_the_next_method_return_type() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.py");
        let source = "class Items:\n    def __iter__(self):\n        return self\n    def __next__(self) -> int:\n        return 3\nprint(next(Items()))\n";
        let mut compiler = super::IncrementalCompiler::default();
        std::fs::write(&path, source).unwrap();
        compiler.check_file(&path, None).unwrap();

        std::fs::write(&path, source.replace("return 3", "return 'bad'")).unwrap();
        let error = compiler.check_file(&path, None).unwrap_err();
        assert!(error.contains("invalid-return-type"), "{error}");
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

        let calls = vec!["part()"; 84].join(" + ");
        super::analysis::check_source_complexity(&format!("table = tuple({calls})\n")).unwrap();
        for expression in [
            format!("f{}", "()".repeat(128)),
            format!("f({}){}", "1+".repeat(50) + "1", "[0]".repeat(50)),
            vec!["f()"; 128].join(" + "),
        ] {
            assert!(super::analysis::check_source_complexity(&expression).is_err());
        }

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
    fn stored_map_arguments_use_the_registered_call_binding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.py");
        let source = r#"
def twice(value: int) -> int:
    return 2 * value

cursor = map(twice, [1, 2], strict=True)
print(next(cursor))
"#;
        let mut compiler = super::IncrementalCompiler::default();
        std::fs::write(&path, source).unwrap();
        compiler.check_file(&path, None).unwrap();

        std::fs::write(&path, format!("{source}twice('bad')\n")).unwrap();
        let error = compiler.check_file(&path, None).unwrap_err();
        assert!(
            error.contains("main.py:7:7: Imre: expected int, got str"),
            "{error}"
        );
    }

    #[test]
    fn module_functions_take_precedence_over_builtin_calls() {
        let source = r#"
import builtins

def print(value: int) -> int:
    return value + 1

def sorted(value: int) -> list[int]:
    return [value]

def result() -> list[int]:
    increment = print
    return sorted(increment(print(40)))

builtins.print(result()[0])
"#;
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        assert!(!llvm.contains("call ptr @imre_dyn_new("));
    }

    #[test]
    fn generic_io_annotations_use_typed_stream_reads() {
        let source = r#"
import typing
from typing import IO as Stream

def text(stream: Stream[str]) -> str:
    return stream.readline()

def binary(stream: typing.IO[bytes]) -> bytes:
    return stream.readline()
"#;
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        assert!(llvm.contains("call ptr @imre_text_readline("));
        assert!(llvm.contains("call ptr @imre_binary_readline("));
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
        let nested = compile(&program(14, true), &options).unwrap();
        assert_eq!(nested.matches("call i64 @imre_int_parse(").count(), 14);
        for line in (3..=42).step_by(3) {
            assert!(
                nested.lines().any(|instruction| {
                    instruction.contains("call ptr @imre_exception_at_frame(ptr null")
                        && instruction.contains(&format!(", i64 1, i32 {line},"))
                }),
                "missing protected conversion at line {line}"
            );
        }
        assert!(nested.len() < 2 * deep.len());
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
    fn a_discarded_exception_call_still_needs_its_class_namespace() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.py");
        std::fs::write(
            directory.path().join("model.py"),
            "class Defect(ValueError):\n    def __init__(self, line=None):\n        if line is not None:\n            super().__init__(line)\n        self.line = line\n        print(line)\n",
        )
        .unwrap();
        std::fs::write(&main, "from model import Defect\nDefect()\nprint(17)\n").unwrap();
        super::compile_file(&main, None, &CompileOptions::default()).unwrap();
    }

    #[test]
    fn an_installed_exception_class_read_needs_a_python_frame() {
        let directory = tempfile::tempdir().unwrap();
        let installed = tempfile::tempdir().unwrap();
        let entry = directory.path().join("main.py");
        std::fs::write(
            &entry,
            "from helper import project\nprint(project(ValueError('test'), 3))\n",
        )
        .unwrap();
        std::fs::write(
            installed.path().join("helper.py"),
            "def project(error: BaseException, n: int) -> int:\n    module = type(error).__module__\n    return n * 31415927\n",
        )
        .unwrap();
        let options = CompileOptions {
            extension_paths: vec![installed.path().to_owned()],
            ..CompileOptions::default()
        };
        let program = super::compile_file(&entry, None, &options).unwrap();
        assert!(program.llvm_ir.contains("call ptr @imre_dyn_new("));
    }

    #[test]
    fn a_caught_exception_keeps_its_runtime_class_in_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.py");
        std::fs::write(
            &path,
            "try:\n    int('bad')\nexcept ValueError as error:\n    cls = type(error)\n    print(cls is ValueError, cls.__name__)\n",
        )
        .unwrap();
        super::IncrementalCompiler::default()
            .check_file(&path, None)
            .unwrap();
    }

    #[test]
    fn an_unrelated_exception_does_not_change_a_proven_handler_class() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.py");
        let compile = |raised: &str| {
            std::fs::write(
                &path,
                format!("class Detail(ValueError):\n    pass\ntry:\n    raise {raised}('marker')\nexcept ValueError as error:\n    print(type(error).__qualname__)\n"),
            )
            .unwrap();
            super::compile_file(&path, None, &CompileOptions::default())
                .unwrap()
                .llvm_ir
        };
        let uses_scoped_lookup = |ir: &str| {
            ir.lines().any(|line| {
                line.contains("call i64 @imre_boxed_named_op(")
                    || line.contains("call i64 @imre_boxed_guarded_named_op(")
            })
        };
        assert!(!uses_scoped_lookup(&compile("ValueError")));
        assert!(uses_scoped_lookup(&compile("Detail")));
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
