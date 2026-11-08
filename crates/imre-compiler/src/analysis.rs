//! Hosts ty's real semantic analysis and Ruff's cached parser over one source snapshot.

#[cfg(test)]
mod closed_program_tests;
#[cfg(test)]
mod incremental_tests;
mod inferred_shapes;
mod tuple_displays;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruff_db::diagnostic::{Diagnostic, DiagnosticId, DisplayDiagnosticConfig, Severity};
use ruff_db::files::{File, Files, system_path_to_file};
use ruff_db::parsed::ParsedModuleRef;
use ruff_db::system::{InMemorySystem, System, SystemPathBuf};
use ruff_db::vendored::VendoredFileSystem;
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_ast::visitor::{Visitor, walk_stmt};
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::{Mode, lexer::lex};
use ruff_text_size::{Ranged, TextRange, TextSize};
use salsa::Setter;
use ty_python_core::program::ProgramSettings;
use ty_python_core::{Program, ProgramFile};
use ty_python_semantic::dependency::DependencyMetadata;
use ty_python_semantic::lint::{LintRegistry, RuleSelection};
use ty_python_semantic::{
    AnalysisSettings, Db, HasType, SemanticModel, check_file_unwrap, default_lint_registry,
};
use ty_site_packages::PythonVersionWithSource;

#[salsa::input(singleton)]
struct DiagnosticRules {
    rules: Arc<RuleSelection>,
}

// Layout hints are optional. Abandon a growing inferred type before it exhausts the stack.
const MAX_HINT_CYCLE_ITERATIONS: u8 = 16;
struct LayoutHintLimit;

thread_local! {
    static READING_LAYOUT_HINT: Cell<bool> = const { Cell::new(false) };
}

#[salsa::db]
#[derive(Clone)]
struct Database {
    storage: salsa::Storage<Self>,
    files: Files,
    system: InMemorySystem,
    vendored: VendoredFileSystem,
    settings: ProgramSettings,
    analysis: Arc<AnalysisSettings>,
}

/// A project database shared by ty's checks and Ruff's cached parser.
#[derive(Clone)]
pub(crate) struct Session {
    db: Database,
    files: Vec<File>,
    tuple_spreads: Vec<HashMap<TextSize, usize>>,
    rules: Arc<RuleSelection>,
    pub(crate) root: PathBuf,
    extension_roots: Vec<PathBuf>,
    hint_reads: Cell<usize>,
    /// Explicit checks can report conversions that executables perform at runtime.
    pub(crate) explicit_check: bool,
}

/// Source ownership used by the same diagnostics for checked graphs and object registrations.
pub(crate) trait ClosedSource {
    fn project_path(&self) -> Option<&Path>;
    fn supplies_declarations(&self) -> bool {
        true
    }
}

impl ClosedSource for crate::modules::SourceModule {
    fn project_path(&self) -> Option<&Path> {
        self.is_program_source().then_some(&self.path)
    }
    fn supplies_declarations(&self) -> bool {
        !self.bundled && !self.is_namespace()
    }
}

impl ClosedSource for (PathBuf, bool) {
    fn project_path(&self) -> Option<&Path> {
        self.1.then_some(&self.0)
    }
}

/// Check the winning runtime registrations together so that imports see the same definitions.
pub(crate) fn check_registered_project<'a>(
    root: &Path,
    extension_roots: &[PathBuf],
    sources: impl IntoIterator<
        Item = crate::dynamic::RegisteredSource<'a>,
        IntoIter: ExactSizeIterator + Clone,
    >,
    inputs: &crate::SourceInputs,
    checked: Option<&crate::modules::SourceGraph>,
) -> Result<(), String> {
    let sources = sources.into_iter();
    if sources.len() == 0 {
        return Ok(());
    }
    if let Some(graph) = checked {
        // Native lowering or the source check has validated these declarations.
        // Reuse this result only for the same bindings, bytes, and program roles.
        let mut remaining: HashMap<_, _> = graph
            .modules
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                !m.bundled
                    && !m.is_namespace()
                    && !m.path.extension().is_some_and(|ext| ext == "pyi")
            })
            .map(|(id, module)| (module.name.as_str(), id))
            .collect();
        let identities: HashMap<_, _> = inputs
            .files
            .iter()
            .map(|(path, identity, digest)| {
                // Only the entry spelling can be relative; imported paths are already absolute.
                let path = if path.is_absolute() {
                    std::borrow::Cow::Borrowed(path.as_path())
                } else {
                    std::borrow::Cow::Owned(
                        std::path::absolute(path).map_err(|error| error.to_string())?,
                    )
                };
                Ok((path, (identity, digest)))
            })
            .collect::<Result<_, String>>()?;
        let admitted = sources.clone().all(|winner| {
            let crate::dynamic::RegisteredSource { name, path, source, is_program } = winner;
            let key = if name == "__main__" { &graph.modules[graph.entry].name } else { name };
            let Some(id) = remaining.remove(key) else { return false };
            let module = &graph.modules[id];
            if is_program != module.is_program_source() {
                return false;
            }
            if name != "__main__" {
                // A checked bundled declaration can intentionally mask its source.
                // It cannot supply the type of a different, first-party registration.
                let session = &graph.session;
                let file = session.files[id];
                let model = SemanticModel::new(&session.db, session.db.program_file(file));
                if model
                    .resolve_module(Some(name), 0)
                    .and_then(|m| m.file(&session.db))
                    != Some(file)
                {
                    return false;
                }
            }
            if path == Path::new("<string>") {
                return graph.virtual_command && module.source == source;
            }
            graph
                .inputs
                .files
                .get(id)
                .is_some_and(|(_, identity, digest)| {
                    identities.get(path) == Some(&(identity, digest))
                        && (module.path == path || graph.module_filename(id) == path)
                        // The graph stores normalized text. Usually we can compare the
                        // actual bytes directly; physical CR/CRLF inputs retain their raw digest.
                        && (module.source == source || source.contains('\r') && blake3::hash(source.as_bytes()).to_hex().as_str() == digest)
                })
        });
        if admitted && remaining.is_empty() {
            return Ok(());
        }
    }

    let mut session =
        Session::with_storage(root, extension_roots, salsa::Storage::default(), true)?;
    let mut paths = Vec::with_capacity(sources.len());
    // Static SDK declarations can type an import even though there is no Python
    // body to register. An actual same-name registration supplies its own types.
    if let Some(graph) = checked {
        let replaced: std::collections::HashSet<_> =
            sources.clone().map(|winner| winner.name).collect();
        for module in &graph.modules {
            if !module.bundled
                && module.path.extension().is_some_and(|ext| ext == "pyi")
                && !replaced.contains(module.name.as_str())
            {
                session.add(&module.path, &module.source, crate::SourceKind::File)?;
                paths.push((module.path.clone(), false));
            }
        }
    }
    for winner in sources {
        let crate::dynamic::RegisteredSource {
            path,
            source,
            is_program,
            ..
        } = winner;
        let (path, kind) = if path == Path::new("<string>") {
            (root.join("<string>.py"), crate::SourceKind::Command)
        } else {
            (path.to_owned(), crate::SourceKind::File)
        };
        let source = crate::normalize_source(source, kind)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        session.add(&path, &source, kind)?;
        paths.push((path, is_program));
    }
    session.check_closed_program(&paths, None, &[], None)
}

impl Session {
    pub(crate) fn has_extension_roots(&self) -> bool {
        !self.extension_roots.is_empty()
    }

    pub(crate) fn hint_reads(&self) -> usize {
        self.hint_reads.get()
    }

    pub(crate) fn new(root: &Path, extension_roots: &[PathBuf]) -> Result<Self, String> {
        let storage = salsa::Storage::new(Some(Box::new(|event| {
            if let salsa::EventKind::WillIterateCycle { iteration, .. } = event.kind
                && iteration >= MAX_HINT_CYCLE_ITERATIONS
                && READING_LAYOUT_HINT.get()
            {
                std::panic::resume_unwind(Box::new(LayoutHintLimit));
            }
        })));
        Self::with_storage(root, extension_roots, storage, false)
    }

    /// Initialize the project inputs and rule set over independent Salsa storage.
    fn with_storage(
        root: &Path,
        extension_roots: &[PathBuf],
        storage: salsa::Storage<Database>,
        registered_project: bool,
    ) -> Result<Self, String> {
        let root = root.to_str().ok_or("module paths must be valid UTF-8")?;
        let system = InMemorySystem::default();
        system
            .fs()
            .create_directory_all(root)
            .map_err(|error| error.to_string())?;
        // The loader reserves this namespace for sealed compiler intrinsics.
        // Give ty its exact public signature without loading the dynamic CPython
        // compatibility implementation or weakening unresolved-import checks.
        let project = SystemPathBuf::from(root);
        let intrinsic = project.join("imre");
        let stdlib_stubs = intrinsic.join(".stdlib-stubs");
        // Resolved graphs pair compiler-bundled bodies with their declarations.
        // A manifest of winning project sources uses those declarations as fallbacks.
        let provider_stubs = if registered_project {
            &stdlib_stubs
        } else {
            &project
        };
        for (directory, name, stub) in [
            (
                &project,
                "imre/_zipimport",
                "from zipfile import ZipFile as _ZipFile\ndef open_index(path: str) -> _ZipFile: ...\n",
            ),
            (
                &project,
                "_imre_mock",
                "def mark_call_list(values: list, /) -> bool: ...\ndef is_call_list(values: list, /) -> bool: ...\n",
            ),
            (
                &project,
                "_imre_doctest",
                "def capture_start() -> None: ...\ndef capture_take() -> str: ...\ndef display(value: object, /) -> None: ...\ndef ascii_output(text: str, /) -> str: ...\ndef exception_name(exc: BaseException, /) -> str: ...\ndef execute(test: object, debug: bool, /) -> None: ...\n",
            ),
            (
                provider_stubs,
                "hashlib",
                include_str!("analysis/hashlib.pyi"),
            ),
            // The pinned typeshed stub admits integer shell flags and the check_output
            // keywords whose conflicts are reported at runtime (see licenses/typeshed.txt).
            (
                provider_stubs,
                "subprocess",
                include_str!("stubs/subprocess.pyi"),
            ),
            (
                provider_stubs,
                "tomli",
                "from tomllib import load as load, loads as loads\n",
            ),
            (
                provider_stubs,
                "unittest/mock",
                include_str!("../stdlib/unittest/mock.pyi"),
            ),
            (
                provider_stubs,
                "unidiff",
                include_str!("analysis/unidiff.pyi"),
            ),
            (
                provider_stubs,
                "tomli_w",
                "from typing import Any, BinaryIO\ndef dumps(obj: dict[str, Any]) -> str: ...\ndef dump(obj: dict[str, Any], fp: BinaryIO) -> None: ...\n",
            ),
        ] {
            system
                .fs()
                .write_file_all(directory.join(format!("{name}.pyi")), stub)
                .map_err(|error| error.to_string())?;
        }
        system
            .fs()
            .write_file_all(
                intrinsic.join("__init__.pyi"),
                "from . import json as json, toml as toml\nfrom typing import TypeVar, dataclass_transform\n_T = TypeVar('_T')\n@dataclass_transform(frozen_default=True)\ndef struct(cls: type[_T], /) -> type[_T]: ...\n",
            )
            .map_err(|error| error.to_string())?;
        system
            .fs()
            .write_file_all(
                intrinsic.join("_logging.pyi"),
                include_str!("../stdlib/logging/runtime.pyi"),
            )
            .map_err(|error| error.to_string())?;
        system
            .fs()
            .write_file_all(
                intrinsic.join("toml.pyi"),
                "from typing import TypeVar\n_T = TypeVar(\"_T\")\ndef decode(text: str, schema: type[_T], /) -> _T: ...\n",
            )
            .map_err(|error| error.to_string())?;
        system
            .fs()
            .write_file_all(
                intrinsic.join("json.pyi"),
                "from typing import TypeVar\n_T = TypeVar(\"_T\")\ndef decode(text: str, schema: type[_T], /) -> _T: ...\ndef encode(value: object, /, *, indent: int | None = None, ensure_ascii: bool = True) -> str: ...\n",
            )
            .map_err(|error| error.to_string())?;
        let vendored = ty_vendored::file_system().clone();
        // Based on ty_vendored 0.0.15 (licenses/typeshed.txt). Retain the cache
        // helpers' object parameters when importing updated typeshed declarations.
        system
            .fs()
            .write_file_all(
                stdlib_stubs.join("_frozen_importlib_external.pyi"),
                include_str!("stubs/_frozen_importlib_external.pyi"),
            )
            .map_err(|e| e.to_string())?;
        // CPython's _simple_enum builds the public enum classes from private member
        // factories. Check callers against typeshed's IntEnum/StrEnum declarations.
        let http = vendored
            .read_to_string("stdlib/http/__init__.pyi")
            .map_err(|e| e.to_string())?;
        system
            .fs()
            .write_file_all(provider_stubs.join("http/__init__.pyi"), http)
            .map_err(|e| e.to_string())?;
        // Refresh from the locked ty_vendored's vendor/typeshed/stdlib/inspect.pyi,
        // retaining the private _sentinel: object declaration for inspect.getattr_static.
        system
            .fs()
            .write_file_all(
                stdlib_stubs.join("inspect.pyi"),
                include_str!("stubs/inspect.pyi"),
            )
            .map_err(|e| e.to_string())?;
        // CPython's POSIX which() accepts a byte search path independently of
        // the command's type. The cross-platform typeshed overloads omit it.
        system
            .fs()
            .write_file_all(
                provider_stubs.join("shutil.pyi"),
                include_str!("stubs/shutil.pyi"),
            )
            .map_err(|error| error.to_string())?;
        let mut settings = ProgramSettings::empty(&vendored);
        settings.python_version.version = crate::version::LANGUAGE;
        let mut roots = vec![SystemPathBuf::from(root)];
        for path in extension_roots {
            let path = path.to_str().ok_or("extension paths must be valid UTF-8")?;
            system
                .fs()
                .create_directory_all(path)
                .map_err(|error| error.to_string())?;
            roots.push(SystemPathBuf::from(path));
        }
        roots.push(stdlib_stubs);
        settings.search_paths = ty_module_resolver::SearchPathSettings::new(roots)
            .to_search_paths(&system, &vendored, &ty_module_resolver::FallibleStrategy)
            .map_err(|error| error.to_string())?;
        // The closed checker enforces these type contracts and fixed tuple bounds.
        // Invalid indices and zero slice steps can also be runtime errors. Disable
        // these ty diagnostics before it expands types to construct their messages.
        let registry = default_lint_registry();
        let mut rules = RuleSelection::from_registry(registry);
        for name in [
            "unsupported-operator",
            "invalid-assignment",
            "invalid-argument-type",
            "invalid-return-type",
            "zero-stepsize-in-slice",
            "index-out-of-bounds",
            "unresolved-attribute",
        ] {
            rules.disable(registry.get(name).expect("registered ty lint"));
        }
        let db = Database {
            storage,
            files: Files::default(),
            system,
            vendored,
            settings,
            analysis: Arc::new(AnalysisSettings {
                respect_type_ignore_comments: false,
                ..AnalysisSettings::default()
            }),
        };
        db.settings.search_paths.try_register_static_roots(&db);
        // Lowering queries types with lints disabled. Tracking the rules invalidates
        // these cached queries when the full check enables diagnostics.
        DiagnosticRules::new(&db, Arc::default());
        Ok(Self {
            db,
            files: Vec::new(),
            tuple_spreads: Vec::new(),
            rules: Arc::new(rules),
            root: PathBuf::from(root),
            extension_roots: extension_roots.to_vec(),
            hint_reads: Cell::new(0),
            explicit_check: false,
        })
    }

    /// Parses a normalized source once; semantic checks wait until every import is present.
    pub(crate) fn add(
        &mut self,
        path: &Path,
        source: &str,
        kind: crate::SourceKind,
    ) -> Result<ParsedModuleRef, String> {
        let path = path.to_str().ok_or("module paths must be valid UTF-8")?;
        let input = imre_ast::indentation::prepare(source);
        check_source_complexity(&input.text).map_err(|error| format!("{path}: {error}"))?;
        let file = self.write_file(path, &input.text)?;
        let parsed = self.parsed(file);
        if let Some(issue) = input.first_issue(parsed.errors().first()) {
            return Err(format!("{path}: {issue}"));
        }
        if let Some(error) = parsed.errors().first() {
            return Err(format!("{path}: {error}"));
        }
        check_scope_syntax(source, parsed.syntax()).map_err(|error| format!("{path}:{error}"))?;
        check_handler_count(parsed.tokens()).map_err(|error| format!("{path}: {error}"))?;
        // Source files additionally restrict form feeds in suite and replacement-field indentation.
        if kind == crate::SourceKind::File {
            check_form_feeds(source, parsed.tokens())
                .map_err(|error| format!("{path}: {error}"))?;
        }
        let mut limits = SuiteLimits {
            tokens: parsed.tokens(),
            scope_start: TextSize::default(),
            active_managers: 0,
            error: None,
        };
        limits.visit_body(&parsed.syntax().body);
        if let Some(error) = limits.error {
            return Err(format!("{path}: {error}"));
        }
        let spreads = if parsed
            .tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::Star)
        {
            tuple_displays::check(&parsed.syntax().body).map_err(|(node, reason)| {
                format!(
                    "{path}:{}",
                    crate::frontend::diagnostic_message(source, node, reason)
                )
            })?
        } else {
            HashMap::new()
        };
        inferred_shapes::check(&parsed.syntax().body, parsed.tokens().len())
            .map_err(|error| format!("{path}: {error}"))?;
        self.files.push(file);
        self.tuple_spreads.push(spreads);
        Ok(parsed)
    }

    pub(crate) fn has_valid_semantic_syntax(&self, module: usize) -> bool {
        ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]))
            .semantic_syntax_errors()
            .is_empty()
    }

    /// Width proved before ty expands the display's field types.
    pub(crate) fn tuple_spread_width(&self, module: usize, start: TextSize) -> Option<usize> {
        self.tuple_spreads.get(module)?.get(&start).copied()
    }

    /// Only modules with statically proved starred tuple fields can need the
    /// corresponding exact list-subscript diagnostic site.
    pub(crate) fn has_tuple_spreads(&self, module: usize) -> bool {
        !self.tuple_spreads[module].is_empty()
    }

    fn write_file(&self, path: &str, source: &str) -> Result<File, String> {
        self.db
            .system
            .fs()
            .write_file_all(path, source)
            .map_err(|error| error.to_string())?;
        system_path_to_file(&self.db, path).map_err(|error| error.to_string())
    }

    fn parsed(&self, file: File) -> ParsedModuleRef {
        ruff_db::parsed::parsed_module(&self.db, self.db.program_file(file).python_file(&self.db))
            .load(&self.db)
    }

    /// Resolve a source offset with the cached line index for the defining module.
    pub(crate) fn source_line(&self, module: usize, offset: TextSize) -> u32 {
        ruff_db::source::line_index(&self.db, self.files[module])
            .line_index(offset)
            .get() as u32
    }

    /// Refresh the semantic AST after a source rewrite that preserves byte offsets.
    pub(crate) fn replace_source(
        &mut self,
        module: usize,
        source: String,
    ) -> Result<ParsedModuleRef, String> {
        let source =
            match imre_ast::indentation::parser_source(&source).map_err(|e| e.to_string())? {
                std::borrow::Cow::Borrowed(_) => source,
                std::borrow::Cow::Owned(input) => input,
            };
        let file = self.files[module];
        let original = ruff_db::source::source_text(&self.db, file);
        let value = original.with_text(source, &Default::default());
        file.set_source_text_override(&mut self.db).to(Some(value));
        Ok(self.parsed(file))
    }

    /// Start diagnostics with independent rules and Salsa storage. Every file has already
    /// passed the loader's preflight in this snapshot.
    pub(crate) fn diagnostic_copy(&self) -> Result<Self, String> {
        let mut copy = Self::with_storage(
            &self.root,
            &self.extension_roots,
            salsa::Storage::default(),
            false,
        )?;
        copy.explicit_check = self.explicit_check;
        for &file in &self.files {
            let path = file
                .path(&self.db)
                .as_system_path()
                .expect("first-party source");
            let source = ruff_db::source::source_text(&self.db, file);
            let file = copy.write_file(path.as_str(), source.as_str())?;
            copy.files.push(file);
        }
        Ok(copy)
    }

    /// Whether this definition needs Python argument binding in a suspended generator frame.
    pub(crate) fn is_untyped_generator_local(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
    ) -> bool {
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        untyped_generator_local(index, function).is_some()
    }

    fn sole_binding(
        &self,
        module: usize,
        expression: &Expr,
    ) -> Option<ty_python_core::definition::Definition<'_>> {
        self.hint_reads.set(self.hint_reads.get() + 1);
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        let scope = index.try_expression_scope_id(expression)?;
        let use_id = index.try_expression_use_id(expression.into())?;
        let mut bindings = index.use_def_map(scope).bindings_at_use(use_id);
        let first = bindings.next()?.binding.definition()?;
        bindings.next().is_none().then_some(first)
    }

    /// Fold a local import only while that statement is the sole binding reaching this read.
    pub(crate) fn has_import_binding(
        &self,
        module: usize,
        expression: &Expr,
        alias: TextRange,
        parsed: &ParsedModuleRef,
    ) -> bool {
        self.sole_binding(module, expression)
            .is_some_and(|def| def.kind(&self.db).target_range(parsed) == alias)
    }

    /// Check the reaching declaration of the original callee. A forwarded call can
    /// reuse an original call's index while substituting a different function.
    pub(crate) fn calls_annotated_parameter(
        &self,
        module: usize,
        call: &ruff_python_ast::ExprCall,
        operand: TextRange,
        parsed: &ParsedModuleRef,
    ) -> bool {
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        if index
            .try_expression_scope_id(&ruff_python_ast::ExprRef::Call(call))
            .is_none()
        {
            return false;
        }
        let ruff_python_ast::AnyRootNodeRef::Expr(Expr::Call(original)) =
            parsed.get_by_index(call.node_index.load())
        else {
            return false;
        };
        original.range() == call.range()
            && original.func == call.func
            && original.arguments.range().contains_range(operand)
            && self.sole_binding(module, &original.func).is_some_and(|def| {
                matches!(def.kind(&self.db), ty_python_core::definition::DefinitionKind::Parameter(p)
                    if p.annotation(parsed).is_some())
            })
    }

    /// Only eager global reads with no reaching definition can fall back to a builtin here.
    pub(crate) fn is_unbound_module_read(&self, module: usize, expression: &Expr) -> bool {
        self.hint_reads.set(self.hint_reads.get() + 1);
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        let global = ty_python_core::FileScopeId::global();
        if index.try_expression_scope_id(expression) != Some(global) {
            return false;
        }
        let Some(use_id) = index.try_expression_use_id(expression.into()) else {
            return false;
        };
        index
            .use_def_map(global)
            .bindings_at_use(use_id)
            .all(|binding| binding.binding.definition().is_none())
    }

    /// A function-local module alias can be shadowed independently of the package binding.
    pub(crate) fn is_stdlib_module(&self, module: usize, expression: &Expr, name: &str) -> bool {
        use ty_python_semantic::types::Type as Ty;
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(expression.inferred_type(&model), Some(Ty::ModuleLiteral(m))
            if m.module(&self.db).name(&self.db).as_str() == name)
    }

    /// Only the stdlib function can add a literal dependency, even if a local shares its spelling.
    pub(crate) fn is_stdlib_function(
        &self,
        module: usize,
        expression: &Expr,
        owner: &str,
        name: &str,
    ) -> bool {
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        resolves_to_stdlib_function(&model, expression, owner, name)
    }

    /// Recognize the sealed regex receiver even when its pattern cannot be lowered statically.
    pub(crate) fn is_regex_pattern(&self, module: usize, expression: &Expr) -> bool {
        use ty_python_semantic::types::{Type, TypeDefinition};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        let Some(ty @ Type::NominalInstance(_)) = expression.inferred_type(&model) else {
            return false;
        };
        let Some(TypeDefinition::StaticClass(class)) =
            ty.definition(&self.db, &model.program_environment())
        else {
            return false;
        };
        class.name(&self.db).as_deref() == Some("Pattern")
            && class.file(&self.db).path(&self.db).is_vendored_path()
            && model
                .resolve_module(Some("re"), 0)
                .and_then(|module| module.file(&self.db))
                == Some(class.file(&self.db))
    }

    pub(crate) fn data_attribute(&self, module: usize, expression: &ruff_python_ast::Expr) -> bool {
        use ty_python_semantic::types::Type as Ty;
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(
            expression.inferred_type(&model),
            Some(Ty::NominalInstance(_) | Ty::LiteralValue(_))
        )
    }

    pub(crate) fn is_typing_protocol(&self, module: usize, expression: &Expr) -> bool {
        use ty_python_semantic::types::{SpecialFormType, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(
            expression.inferred_type(&model),
            Some(Ty::SpecialForm(SpecialFormType::Protocol))
        )
    }

    /// Resolve a bare `object` base at the point where the class is defined.
    pub(crate) fn builtin_object_base(&self, module: usize, base: &Expr) -> bool {
        base.as_name_expr().is_some_and(|name| name.id == "object")
            && self.is_known_class(module, base, ty_python_semantic::types::KnownClass::Object)
    }

    /// The native list and tuple storage belongs to the exact builtins, even through an alias.
    pub(crate) fn is_builtin_sequence_class(&self, module: usize, expression: &Expr) -> bool {
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let file = self.db.program_file(self.files[module]);
        let model = SemanticModel::new(&self.db, file);
        let Some(ty @ Ty::ClassLiteral(_)) = expression.inferred_type(&model) else {
            return false;
        };
        let env = ProgramEnvironment::from_file(file);
        ty.definition(&self.db, &env).is_some_and(|definition| {
            [KnownClass::List, KnownClass::Tuple]
                .into_iter()
                .any(|class| {
                    class
                        .to_subclass_of(&self.db, &env)
                        .definition(&self.db, &env)
                        .as_ref()
                        == Some(&definition)
                })
        })
    }

    /// Check the class definition at the use site, including aliases and local shadowing.
    pub(crate) fn is_stdlib_class(
        &self,
        module: usize,
        expr: &Expr,
        owner: &str,
        name: &str,
    ) -> bool {
        use ty_python_semantic::types::{Type, TypeDefinition};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        let Some(ty @ Type::ClassLiteral(_)) = expr.inferred_type(&model) else {
            return false;
        };
        matches!(ty.definition(&self.db, &model.program_environment()),
            Some(TypeDefinition::StaticClass(class))
                if class.name(&self.db).as_deref() == Some(name)
                    && class.file(&self.db).path(&self.db).is_vendored_path()
                    && model.resolve_module(Some(owner), 0).and_then(|m| m.file(&self.db))
                        == Some(class.file(&self.db)))
    }

    /// Identify exact OrderedDict and deque class objects even when imported under another name.
    pub(crate) fn is_collections_storage_base(&self, module: usize, base: &Expr) -> bool {
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let file = self.db.program_file(self.files[module]);
        let model = SemanticModel::new(&self.db, file);
        let Some(owner @ Ty::ClassLiteral(_)) = base.inferred_type(&model) else {
            return false;
        };
        let env = ProgramEnvironment::from_file(file);
        owner.definition(&self.db, &env).is_some_and(|definition| {
            [KnownClass::OrderedDict, KnownClass::Deque]
                .into_iter()
                .any(|class| {
                    let canonical = class.to_instance(&self.db, &env);
                    canonical.definition(&self.db, &env).as_ref() == Some(&definition)
                })
        })
    }

    /// Check the class object bound to a name or attribute at its point of use.
    pub(crate) fn is_known_class(
        &self,
        module: usize,
        expression: &Expr,
        class: ty_python_semantic::types::KnownClass,
    ) -> bool {
        use ty_python_semantic::types::{ProgramEnvironment, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let file = self.db.program_file(self.files[module]);
        let model = SemanticModel::new(&self.db, file);
        let Some(ty @ Ty::ClassLiteral(_)) = expression.inferred_type(&model) else {
            return false;
        };
        let env = ProgramEnvironment::from_file(file);
        // ty represents tuple instances by their element types. Its subclass view
        // exposes the builtin definition without constructing an unparameterized tuple.
        let canonical = match class {
            ty_python_semantic::types::KnownClass::Tuple => class.to_subclass_of(&self.db, &env),
            _ => class.to_instance(&self.db, &env),
        };
        ty.definition(&self.db, &env)
            .is_some_and(|definition| canonical.definition(&self.db, &env) == Some(definition))
    }

    /// Bound methods on these sealed native receiver layouts cannot dispatch to a
    /// first-party class. Source subclasses of those layouts are rejected by the
    /// fixed-class collector. Unknowns, unions, protocols and first-party
    /// receivers keep the ordinary conservative name-based roots.
    pub(crate) fn sealed_native_method(
        &self,
        module: usize,
        attr: &ruff_python_ast::ExprAttribute,
    ) -> bool {
        use ty_python_semantic::types::{ProgramEnvironment, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let program = self.db.program_file(self.files[module]);
        let model = SemanticModel::new(&self.db, program);
        if !matches!(attr.inferred_type(&model), Some(Ty::BoundMethod(_))) {
            return false;
        }
        let Some(receiver @ Ty::NominalInstance(_)) = attr.value.inferred_type(&model) else {
            return false;
        };
        let Some(class) = receiver.definition(&self.db, &ProgramEnvironment::from_file(program))
        else {
            return false;
        };
        let Some(range) = class.focus_range(&self.db) else {
            return false;
        };
        let Some(path) = range.file().path(&self.db).as_vendored_path() else {
            return false;
        };
        // Use the bundled stdlib source identity, never a user-controlled module name.
        let module = match path.file_name() {
            Some("__init__.pyi") => path.parent().and_then(|parent| parent.file_name()),
            _ if path.extension() == Some("pyi") => path.file_stem(),
            _ => None,
        };
        matches!(
            module,
            Some("zipfile" | "io" | "_io" | "pathlib" | "re" | "gzip" | "hashlib" | "_hashlib")
        )
    }

    /// A nominal instance here, instead of its declared optional union, records
    /// a source-level narrowing. The backend must still guard mutable rereads.
    pub(crate) fn narrowed_instance(&self, module: usize, expression: &Expr) -> bool {
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let program = self.db.program_file(self.files[module]);
        let model = SemanticModel::new(&self.db, program);
        let Some(ty @ Ty::NominalInstance(_)) = expression.inferred_type(&model) else {
            return false;
        };
        ty != KnownClass::NoneType.to_instance(&self.db, &ProgramEnvironment::from_file(program))
    }

    /// Get a scalar layout hint for an append operand in its own lexical scope.
    pub(crate) fn scalar_type(&self, module: usize, value: &Expr) -> Option<crate::ir::Type> {
        use crate::ir::Type;
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};

        if matches!(
            value,
            Expr::Tuple(_) | Expr::List(_) | Expr::Set(_) | Expr::Dict(_)
        ) {
            return None;
        }
        self.hint_reads.set(self.hint_reads.get() + 1);
        let file = self.db.program_file(self.files[module]);
        // Inferring one expression visits its scope. An unrelated loop can grow a
        // nested tuple type at every iteration, even for a simple scalar operand.
        let previous = READING_LAYOUT_HINT.replace(true);
        let inferred = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            value.inferred_type(&SemanticModel::new(&self.db, file))
        }));
        READING_LAYOUT_HINT.set(previous);
        let ty = match inferred {
            Ok(ty) => ty?,
            Err(payload) if payload.is::<LayoutHintLimit>() => return None,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        if !matches!(
            ty,
            Ty::LiteralValue(_) | Ty::NominalInstance(_) | Ty::Union(_)
        ) {
            return None;
        }
        let env = ProgramEnvironment::from_file(file);
        // ty expands a float annotation to float | int. Match that annotation
        // after the narrower bool and int cases; Imre checks the exact operand.
        [
            (KnownClass::Bool, Type::Bool),
            (KnownClass::Int, Type::Int),
            (KnownClass::Float, Type::Float),
            (KnownClass::Str, Type::Str),
            (KnownClass::Bytes, Type::Bytes),
        ]
        .into_iter()
        .find(|(class, _)| {
            let mut instance = class.to_instance(&self.db, &env);
            if matches!(class, KnownClass::Float) {
                instance = ty_python_semantic::types::UnionType::from_two_elements(
                    &self.db,
                    &env,
                    instance,
                    KnownClass::Int.to_instance(&self.db, &env),
                );
            }
            ty.is_assignable_to(&self.db, &env, instance)
        })
        .map(|(_, ty)| ty)
    }

    /// Recognize the canonical tuple[str, ...] type across calls, attributes and names.
    pub(crate) fn is_string_tuple(&self, module: usize, value: &Expr) -> bool {
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};
        self.hint_reads.set(self.hint_reads.get() + 1);
        let file = self.db.program_file(self.files[module]);
        let env = ProgramEnvironment::from_file(file);
        let model = SemanticModel::new(&self.db, file);
        let text = KnownClass::Str.to_instance(&self.db, &env);
        value.inferred_type(&model) == Some(Ty::homogeneous_tuple(&self.db, &env, text))
    }

    /// Compare an argv expression to the canonical built-in list[str] instance,
    /// as declared for sys.argv in ty's vendored standard library. Equality
    /// (not assignability) rules out Any and lists with a different field ABI.
    pub(crate) fn is_string_list(&self, module: usize, value: &Expr) -> bool {
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        let Some(actual) = value.inferred_type(&model) else {
            return false;
        };
        let Ok(sys) = ruff_db::files::vendored_path_to_file(&self.db, "stdlib/sys/__init__.pyi")
        else {
            return false;
        };
        let sys_model = SemanticModel::new(&self.db, self.db.program_file(sys));
        self.parsed(sys).syntax().body.iter().any(|stmt| {
            matches!(stmt, Stmt::AnnAssign(a) if a.target.as_name_expr().is_some_and(|name| name.id == "argv") && a.target.inferred_type(&sys_model) == Some(actual))
        })
    }

    pub(crate) fn is_never(&self, module: usize, value: &ruff_python_ast::Expr) -> bool {
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(
            value.inferred_type(&model),
            Some(ty_python_semantic::types::Type::Never)
        )
    }

    /// Preserve first-party contracts when lowering a checked file to object operations.
    pub(crate) fn check_fallback_contracts(
        &mut self,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        calls: FallbackCalls,
        runtime_provider: Option<usize>,
        checked_modules: Option<&[crate::modules::SourceModule]>,
    ) -> Result<(), String> {
        let checks = match calls {
            FallbackCalls::All | FallbackCalls::DecoratorFactories => FileChecks::FallbackCalls,
            FallbackCalls::FunctionBodies => FileChecks::FunctionBodyCalls,
            FallbackCalls::NamedFunctions => FileChecks::NamedFunctionCalls,
            FallbackCalls::None | FallbackCalls::Entry(_) => FileChecks::FallbackReturns,
        };
        self.set_fallback_rules(
            checks != FileChecks::FallbackReturns,
            matches!(calls, FallbackCalls::DecoratorFactories),
            checked_modules.is_some(),
        );
        let provider = runtime_provider.map(|index| self.files[index]);
        self.errors(skipped, overloads, checks, provider, checked_modules)?;
        if let FallbackCalls::Entry(entry) = calls {
            // Only the entry gained a new statement order. Imported modules retain
            // the argument-checking policy they had before the guard.
            self.set_fallback_rules(true, false, checked_modules.is_some());
            let checked = CheckedFile::new(
                &self.db,
                self.files[entry],
                skipped.map_or(&[][..], |ranges| ranges[entry].as_slice()),
                overloads[entry].as_slice(),
                FileChecks::FallbackCalls,
                provider,
                checked_modules.is_some_and(|modules| !modules[entry].bundled),
                self.explicit_check,
                None,
            );
            let errors = file_errors(&self.db, checked);
            if !errors.is_empty() {
                return Err(errors
                    .iter()
                    .map(|error| error.rendered.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"));
            }
        }
        Ok(())
    }

    fn set_fallback_rules(
        &mut self,
        scoped_builtin: bool,
        check_callees: bool,
        checked_file: bool,
    ) {
        let mut rules = RuleSelection::default();
        let arguments = if scoped_builtin {
            CALL_ARGUMENT_RULES
        } else {
            &[]
        };
        let callee = check_callees.then_some("call-non-callable");
        for name in ["invalid-return-type"]
            .into_iter()
            .chain(arguments.iter().copied())
            .chain(
                (self.explicit_check && checked_file && !scoped_builtin)
                    .then_some("no-matching-overload"),
            )
            .chain(callee)
            .chain(checked_file.then_some("invalid-assignment"))
        {
            rules.enable(
                default_lint_registry()
                    .get(name)
                    .expect("registered ty lint"),
                Severity::Error,
                ty_python_semantic::lint::LintSource::Default,
            );
        }
        DiagnosticRules::get(&self.db)
            .set_rules(&mut self.db)
            .to(Arc::new(rules));
    }

    /// Checks every loaded file, including unused functions and inactive main guards.
    pub(crate) fn check_scopes(
        &mut self,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
    ) -> Result<(), String> {
        DiagnosticRules::get(&self.db)
            .set_rules(&mut self.db)
            .to(self.rules.clone());
        self.errors(skipped, overloads, FileChecks::Full, None, None)
    }

    /// Check calls to explicit source parameters before choosing a representation.
    /// Keep an existing native diagnostic when it describes the same argument.
    pub(crate) fn check_closed_program(
        &mut self,
        modules: &[impl ClosedSource],
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        native: Option<&crate::frontend::phase::Diagnostic>,
    ) -> Result<(), String> {
        // Native lowering and the ordinary fallback checker own the rest of the type
        // contract. Check the same declared calls even when lowering stops at a
        // representation choice, or registration selects a different source graph.
        let mut rules = RuleSelection::default();
        rules.enable(
            default_lint_registry()
                .get("invalid-argument-type")
                .expect("registered ty lint"),
            Severity::Error,
            ty_python_semantic::lint::LintSource::Default,
        );
        DiagnosticRules::get(&self.db)
            .set_rules(&mut self.db)
            .to(Arc::new(rules));
        self.closed_program_errors(modules, skipped, overloads, native)
    }

    fn closed_program_errors(
        &self,
        modules: &[impl ClosedSource],
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        native: Option<&crate::frontend::phase::Diagnostic>,
    ) -> Result<(), String> {
        assert_eq!(modules.len(), self.files.len());
        if !modules.iter().any(|module| module.project_path().is_some()) {
            return Ok(());
        }
        let mut parameters = Vec::new();
        for (module, &file) in modules.iter().zip(&self.files) {
            if !module.supplies_declarations() {
                continue;
            }
            let source = self.db.program_file(file);
            let index = ty_python_core::semantic_index(&self.db, source);
            let parsed = ruff_db::parsed::parsed_module(&self.db, source.python_file(&self.db))
                .load(&self.db);
            for scope in index.scope_ids() {
                if let ty_python_core::scope::NodeWithScopeKind::Function(function) =
                    scope.node(&self.db)
                {
                    parameters.extend(
                        function
                            .node(&parsed)
                            .parameters
                            .iter()
                            .filter(|p| p.annotation().is_some())
                            .map(|p| (file, p.range())),
                    );
                }
            }
        }
        if parameters.is_empty() {
            return Ok(());
        }
        let declarations = DeclaredParameters::new(&self.db, parameters);
        let mut errors = Vec::new();
        for (index, (module, &file)) in modules.iter().zip(&self.files).enumerate() {
            let Some(path) = module.project_path() else {
                continue;
            };
            if path.extension().is_some_and(|ext| ext == "pyi") {
                continue;
            }
            let checked = CheckedFile::new(
                &self.db,
                file,
                skipped.map_or(&[][..], |ranges| ranges[index].as_slice()),
                overloads.get(index).map_or(&[][..], Vec::as_slice),
                FileChecks::DeclaredArguments,
                None,
                true,
                self.explicit_check,
                Some(declarations),
            );
            for error in file_errors(&self.db, checked) {
                let original = error
                    .range
                    .and_then(|range| native?.matching_contract(path, range, error.rule.as_str()));
                errors.push(original.map_or_else(|| error.rendered.clone(), ToString::to_string));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    fn errors(
        &self,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        checks: FileChecks,
        runtime_provider: Option<File>,
        checked_modules: Option<&[crate::modules::SourceModule]>,
    ) -> Result<(), String> {
        let errors: Vec<_> = self
            .files
            .iter()
            .enumerate()
            .flat_map(|(index, &file)| {
                let checked = CheckedFile::new(
                    &self.db,
                    file,
                    skipped.map_or(&[][..], |ranges| ranges[index].as_slice()),
                    overloads.get(index).map_or(&[][..], Vec::as_slice),
                    checks,
                    runtime_provider,
                    checked_modules.is_some_and(|modules| !modules[index].bundled),
                    self.explicit_check,
                    None,
                );
                file_errors(&self.db, checked)
            })
            .map(|error| error.rendered.as_str())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

/// Reachable-only entry profiles can select different scopes of the same source.
#[salsa::interned]
struct CheckedFile<'db> {
    #[returns(copy)]
    file: File,
    skipped: Vec<TextRange>,
    overloads: Vec<CheckedDiagnostic>,
    #[returns(copy)]
    checks: FileChecks,
    #[returns(copy)]
    runtime_provider: Option<File>,
    #[returns(copy)]
    first_party: bool,
    #[returns(copy)]
    explicit_check: bool,
    #[returns(copy)]
    declarations: Option<DeclaredParameters<'db>>,
}

/// Explicit parameters from the selected Python sources and SDK declarations.
#[salsa::interned]
struct DeclaredParameters<'db> {
    ranges: Vec<(File, TextRange)>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum FileChecks {
    Full,
    DeclaredArguments,
    FallbackReturns,
    FunctionBodyCalls,
    NamedFunctionCalls,
    FallbackCalls,
}

/// Files in which a checked object fallback also enforces call arguments.
#[derive(Clone, Copy, Default)]
pub(crate) enum FallbackCalls {
    #[default]
    None,
    /// Local annotated calls in function bodies when a Python script needs a boxed operation.
    FunctionBodies,
    /// Direct calls to annotated source functions while an import uses runtime classes.
    NamedFunctions,
    Entry(usize),
    All,
    /// A factory must return a callable to decorate the method.
    DecoratorFactories,
}

const CALL_ARGUMENT_RULES: &[&str] = &[
    "invalid-argument-type",
    "missing-argument",
    "no-matching-overload",
    "too-many-positional-arguments",
    "unknown-argument",
    "positional-only-parameter-as-kwarg",
    "parameter-already-assigned",
];

/// Only exclude the exact ty diagnostic already checked by native lowering.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CheckedDiagnostic {
    AbstractClassCall(TextRange),
    UnboundLocalRead(TextRange),
    Decorator(TextRange),
    RuntimeOverload(TextRange),
    OptionalLockContext(TextRange),
    Call(TextRange),
    NonCallable(TextRange),
    InertReturn(TextRange),
    DictDeletion(TextRange),
    DeferredName(TextRange),
    Keyword(TextRange),
    CheckedIndexRead(TextRange),
    MissingArgument(TextRange),
    BuiltinImport(TextRange),
    BuiltinAttribute(TextRange),
    ExcessArgument(TextRange),
    PositionalKeyword(TextRange),
    ArgumentType(TextRange),
    RepeatedKeyword(TextRange),
    NonIterableSpread(TextRange),
    CallableKey(TextRange),
}

impl CheckedDiagnostic {
    pub(crate) fn sort_key(self) -> (TextSize, TextSize, u8) {
        let (range, kind) = match self {
            Self::AbstractClassCall(range) => (range, 9),
            Self::UnboundLocalRead(range) => (range, 10),
            Self::Decorator(range) => (range, 0),
            Self::OptionalLockContext(range) => (range, 7),
            Self::Call(range) => (range, 1),
            Self::NonCallable(range) => (range, 12),
            Self::Keyword(range) => (range, 2),
            Self::CheckedIndexRead(range) => (range, 3),
            Self::RepeatedKeyword(range) => (range, 11),
            Self::DictDeletion(range) => (range, 8),
            Self::MissingArgument(range) => (range, 4),
            Self::BuiltinImport(range) => (range, 5),
            Self::BuiltinAttribute(range) => (range, 14),
            Self::ExcessArgument(range) => (range, 6),
            Self::InertReturn(range) => (range, 13),
            Self::PositionalKeyword(range) => (range, 7),
            Self::DeferredName(range) => (range, 9),
            Self::NonIterableSpread(range) => (range, 10),
            Self::RuntimeOverload(range) => (range, 12),
            Self::CallableKey(range) => (range, 16),
            Self::ArgumentType(range) => (range, 15),
        };
        (range.start(), range.end(), kind)
    }

    fn covers(self, diagnostic: &str, range: TextRange) -> bool {
        match self {
            Self::AbstractClassCall(call) | Self::CallableKey(call) => {
                diagnostic == "call-non-callable" && call == range
            }
            Self::UnboundLocalRead(read) => diagnostic == "unresolved-reference" && read == range,
            Self::Decorator(decorator) => {
                diagnostic == "no-matching-overload" && decorator.contains_range(range)
            }
            Self::Call(call) => diagnostic == "no-matching-overload" && call == range,
            Self::NonCallable(call) => diagnostic == "call-non-callable" && call == range,
            Self::InertReturn(annotation) => diagnostic == "empty-body" && annotation == range,
            Self::DictDeletion(target) => diagnostic == "not-subscriptable" && target == range,
            Self::RuntimeOverload(name) => diagnostic == "invalid-overload" && name == range,
            Self::DeferredName(name) => diagnostic == "unresolved-reference" && name == range,
            Self::OptionalLockContext(expr) => {
                diagnostic == "invalid-context-manager" && expr == range
            }
            Self::Keyword(keyword) => diagnostic == "unknown-argument" && keyword == range,
            Self::RepeatedKeyword(keyword) => {
                diagnostic == "parameter-already-assigned" && keyword == range
            }
            Self::PositionalKeyword(keyword) => {
                diagnostic == "positional-only-parameter-as-kwarg" && keyword == range
            }
            Self::CheckedIndexRead(read) => diagnostic == "not-subscriptable" && read == range,
            Self::NonIterableSpread(value) => diagnostic == "not-iterable" && value == range,
            Self::MissingArgument(call) => diagnostic == "missing-argument" && call == range,
            Self::BuiltinImport(name) => diagnostic == "unresolved-import" && name == range,
            Self::ArgumentType(arg) => diagnostic == "invalid-argument-type" && arg == range,
            Self::BuiltinAttribute(name) => diagnostic == "unresolved-attribute" && name == range,
            Self::ExcessArgument(arg) => {
                diagnostic == "too-many-positional-arguments" && arg == range
            }
        }
    }
}

/// Files made only of imports and scalar bindings have no call or return sites.
/// Other files can still resolve the names these modules export.
fn fallback_data_module(parsed: &ParsedModuleRef) -> bool {
    fn binding(stmt: &Stmt) -> bool {
        let valid_alias = |alias: &ruff_python_ast::Alias| {
            let bound = alias.asname.as_ref().unwrap_or(&alias.name);
            alias.name.as_str() != "*" && bound.split('.').next() != Some("__debug__")
        };
        match stmt {
            Stmt::Import(import) => !import.is_lazy && import.names.iter().all(valid_alias),
            Stmt::ImportFrom(import) => {
                !import.is_lazy
                    && import
                        .module
                        .as_ref()
                        .is_none_or(|module| module.as_str() != "__future__")
                    && import.names.iter().all(valid_alias)
            }
            Stmt::Assign(assign) => {
                (assign.value.is_literal_expr() || assign.value.is_name_expr())
                    && assign.targets.iter().all(|target| {
                        target
                            .as_name_expr()
                            .is_some_and(|name| name.id.as_str() != "__debug__")
                    })
            }
            Stmt::Pass(_) => true,
            Stmt::Expr(expr) => expr.value.is_string_literal_expr(),
            _ => false,
        }
    }

    parsed.has_no_syntax_errors()
        && parsed.syntax().body.iter().all(|stmt| {
            if let Stmt::Try(guard) = stmt {
                let [ruff_python_ast::ExceptHandler::ExceptHandler(handler)] =
                    guard.handlers.as_slice()
                else {
                    return false;
                };
                !guard.is_star
                    && guard.orelse.is_empty()
                    && guard.finalbody.is_empty()
                    && handler.name.is_none()
                    && handler.type_.as_deref().is_some_and(Expr::is_name_expr)
                    && guard.body.iter().all(binding)
                    && handler.body.iter().all(binding)
            } else {
                binding(stmt)
            }
        })
        // Suppression comments can have diagnostics of their own.
        && !parsed
            .tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::Comment)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CheckedError {
    rule: DiagnosticId,
    range: Option<TextRange>,
    rendered: String,
}

/// A per-file result depends on the ty queries it actually read, including any other source
/// consulted to render a diagnostic. Every revision still requests the result for every file.
#[salsa::tracked(returns(ref))]
fn file_errors<'db>(db: &'db dyn Db, checked: CheckedFile<'db>) -> Vec<CheckedError> {
    let file = checked.file(db);
    let checks = checked.checks(db);
    let full = matches!(checks, FileChecks::Full | FileChecks::DeclaredArguments);
    if !full {
        let parsed =
            ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db);
        if fallback_data_module(&parsed) {
            return Vec::new();
        }
    }
    #[cfg(test)]
    FILE_CHECKS.with_borrow_mut(|checks| {
        if let Some(checks) = checks {
            checks.push(file.path(db).to_string());
        }
    });
    let skipped = checked.skipped(db);
    let overloads = checked.overloads(db);
    db.check_file(file)
        .iter()
        .filter(|diagnostic| {
            diagnostic.severity() >= Severity::Error
                && checked
                    .declarations(db)
                    .is_none_or(|sources| declared_parameter(diagnostic, sources.ranges(db)))
                && (full
                    || diagnostic.id().as_str() != "invalid-assignment"
                    || checked.first_party(db)
                        && diagnostic
                            .primary_span()
                            .and_then(|span| span.range())
                            .is_some_and(|range| scalar_initializer(db, file, range)))
                && !diagnostic
                    .primary_span()
                    .and_then(|span| span.range())
                    .is_some_and(|range| {
                        skipped.iter().any(|scope| scope.contains_range(range))
                            || (!full
                                && CALL_ARGUMENT_RULES.contains(&diagnostic.id().as_str())
                                && runtime_argument(
                                    db,
                                    checked,
                                    range,
                                    diagnostic.id().as_str(),
                                ))
                            // ty also reports implicit None returns at the annotation span.
                            || (diagnostic.id().as_str() == "invalid-return-type"
                                && diagnostic.headline_message()
                                    == "Return type does not match returned value"
                                && contextmanager_annotation(db, file, range))
                            || (matches!(
                                diagnostic.id().as_str(),
                                "too-many-positional-arguments" | "missing-argument"
                            ) && native_partial_call(db, file, range, diagnostic.id().as_str()))
                            || overloads
                                .iter()
                                .any(|scope| scope.covers(diagnostic.id().as_str(), range))
                    })
        })
        .map(|diagnostic| CheckedError {
            rule: diagnostic.id(),
            range: diagnostic.primary_span().and_then(|span| {
                (span.file() == &ruff_db::diagnostic::UnifiedFile::Ty(file))
                    .then(|| span.range())
                    .flatten()
            }),
            rendered: diagnostic
                .display(
                    &(db as &dyn ruff_db::Db),
                    &DisplayDiagnosticConfig::new("imre"),
                )
                .to_string(),
        })
        .collect()
}

/// Literal initializers of scalar names are independent of the operation that required boxing.
fn scalar_initializer(db: &dyn Db, file: File, range: TextRange) -> bool {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
    use ty_python_semantic::types::{KnownClass, Type, UnionType};

    let file = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, file.python_file(db)).load(db);
    let nodes = covering_node(parsed.syntax().into(), range);
    let mut ancestors = nodes.ancestors();
    let Some(assignment) = ancestors.find_map(|node| match node {
        AnyNodeRef::StmtAnnAssign(assignment) => Some(assignment),
        _ => None,
    }) else {
        return false;
    };
    if !assignment.target.is_name_expr()
        || !assignment.value.as_deref().is_some_and(|value| {
            value.range() == range
                && matches!(
                    value,
                    Expr::NumberLiteral(_)
                        | Expr::BooleanLiteral(_)
                        | Expr::StringLiteral(_)
                        | Expr::BytesLiteral(_)
                )
        })
    {
        return false;
    }
    // A metaclass can interpret annotated class attributes as schema fields.
    if ancestors
        .find(|node| {
            matches!(
                node,
                AnyNodeRef::StmtFunctionDef(_) | AnyNodeRef::StmtClassDef(_)
            )
        })
        .is_some_and(|node| matches!(node, AnyNodeRef::StmtClassDef(_)))
    {
        return false;
    }
    let model = SemanticModel::new(db, file);
    let Some(mut declared) = assignment.annotation.inferred_type(&model) else {
        return false;
    };
    let env = model.program_environment();
    let mut seen = std::collections::HashSet::new();
    while let Type::TypeAlias(alias) = declared {
        // The expanded value alone can be a scalar recovered from a malformed cycle.
        // Follow the declaration's inferred RHS before accepting its expanded value.
        use ty_python_core::definition::DefinitionKind;
        use ty_python_semantic::types::{KnownInstanceType, TypeDefinition};

        let object = Type::KnownInstance(KnownInstanceType::TypeAliasType(alias));
        let Some(TypeDefinition::TypeAlias(definition)) = object.definition(db, &env) else {
            return false;
        };
        if !seen.insert(definition) {
            return false;
        }
        let DefinitionKind::TypeAlias(node) = definition.kind(db) else {
            return false;
        };
        let parsed = ruff_db::parsed::parsed_module(db, definition.python_file(db)).load(db);
        let model = SemanticModel::new(db, definition.program_file(db));
        let Some(rhs) = node.node(&parsed).value.inferred_type(&model) else {
            return false;
        };
        if !matches!(rhs, Type::TypeAlias(_)) && rhs != alias.value_type(db) {
            return false;
        }
        declared = rhs;
    }
    let scalar = |class: KnownClass| class.to_instance(db, &env);
    [KnownClass::Int, KnownClass::Bool, KnownClass::Float, KnownClass::Str, KnownClass::Bytes]
        .into_iter().any(|class| declared == scalar(class))
        // ty widens a float annotation to float | int.
        || declared == UnionType::from_two_elements(db, &env, scalar(KnownClass::Float), scalar(KnownClass::Int))
}

/// ty identifies the matched source parameter in an argument diagnostic. Use that
/// resolution for plain functions, constructors and methods, including inherited
/// and imported declarations. A runtime builtin has no project or SDK parameter.
fn declared_parameter(diagnostic: &Diagnostic, parameters: &[(File, TextRange)]) -> bool {
    diagnostic.sub_diagnostics().iter().flat_map(|sub| sub.annotations()).any(|label| {
        let span = label.get_span();
        !label.is_primary() && matches!((span.file(), span.range()),
            (ruff_db::diagnostic::UnifiedFile::Ty(owner), Some(range))
            if parameters.iter().any(|(file, parameter)| file == owner && parameter.contains_range(range)))
    })
}

/// Leave argument conversion to Python for vendored code and its compiler-supplied adapter.
fn runtime_argument(db: &dyn Db, checked: CheckedFile<'_>, range: TextRange, rule: &str) -> bool {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
    use ty_python_semantic::types::Type;
    let checks = checked.checks(db);
    let file = db.program_file(checked.file(db));
    let parsed = ruff_db::parsed::parsed_module(db, file.python_file(db)).load(db);
    let root = AnyNodeRef::from(parsed.syntax());
    let local_calls = matches!(
        checks,
        FileChecks::FallbackReturns
            | FileChecks::FunctionBodyCalls
            | FileChecks::NamedFunctionCalls
    );
    if !root.range().contains_range(range) {
        return local_calls;
    }
    let nodes = covering_node(root, range);
    // ty points to the entire call for missing arguments and overload mismatches.
    // An excess argument can itself be a call, in which case its parent owns the error.
    let whole_call = matches!(rule, "missing-argument" | "no-matching-overload");
    let Some(call) = nodes.ancestors().find_map(|node| {
        let AnyNodeRef::ExprCall(call) = node else {
            return None;
        };
        let owns_range = if whole_call {
            call.range() == range
        } else {
            call.arguments.range().contains_range(range)
        };
        owns_range.then_some(call)
    }) else {
        return local_calls;
    };
    if rule == "no-matching-overload"
        && checked.explicit_check(db)
        && checked.first_party(db)
        && crate::frontend::literal_replace_mismatch(call)
    {
        return false;
    }
    if checks == FileChecks::FallbackReturns {
        return true;
    }
    let model = SemanticModel::new(db, file);
    let Some(callee) = call.func.inferred_type(&model) else {
        return local_calls;
    };
    if matches!(callee, Type::FunctionLiteral(_))
        && let Some(ty_python_semantic::types::TypeDefinition::Function(definition)) =
            callee.definition(db, &model.program_environment())
        && (definition.file(db) == file.file(db) || checks == FileChecks::NamedFunctionCalls)
        && let ty_python_core::definition::DefinitionKind::Function(function) = definition.kind(db)
    {
        if checks == FileChecks::NamedFunctionCalls {
            // Dynamic methods retain their Python call semantics. A direct name still
            // resolves to its annotated source declaration, including across an import.
            let owner = definition.file(db);
            if !call.func.is_name_expr() || owner.path(db).is_vendored_path() {
                return true;
            }
            let source =
                ruff_db::parsed::parsed_module(db, db.program_file(owner).python_file(db)).load(db);
            return !function
                .node(&source)
                .parameters
                .iter()
                .any(|p| p.annotation().is_some());
        }
        let function = function.node(&parsed);
        if checks == FileChecks::FunctionBodyCalls {
            // A Python entry binds dynamic calls at runtime. Preserve the contracts
            // typed lowering checks in first-party function bodies before a fallback.
            let in_body = nodes.ancestors().any(|node| {
                matches!(node,
                AnyNodeRef::StmtFunctionDef(caller)
                    if caller.body.iter().any(|stmt| stmt.range().contains_range(call.range())))
            });
            return !(in_body && function.parameters.iter().any(|p| p.annotation().is_some()));
        }
        if rule == "too-many-positional-arguments"
            && call.arguments.args.iter().any(|arg| arg.range() == range)
        {
            let index = ty_python_core::semantic_index(db, file);
            if let Some(scope) = untyped_generator_local(index, function)
                && index.try_expression_scope_id(call.func.as_ref()) == Some(scope)
            {
                return true;
            }
        }
    }
    if local_calls {
        return true;
    }
    let uses_runtime_provider = |ty: Type<'_>| {
        ty.definition(db, &model.program_environment())
            .and_then(|definition| definition.definition())
            .is_some_and(|definition| {
                let file = definition.file(db);
                file.path(db).is_vendored_path() || Some(file) == checked.runtime_provider(db)
            })
    };
    match callee {
        Type::Union(union) => union
            .elements(db)
            .iter()
            .copied()
            .all(uses_runtime_provider),
        ty => uses_runtime_provider(ty),
    }
}

fn resolves_to_stdlib_function(
    model: &SemanticModel<'_>,
    expression: &Expr,
    owner: &str,
    name: &str,
) -> bool {
    use ty_python_semantic::types::{Type, TypeDefinition};
    let db = model.db();
    let Some(ty @ Type::FunctionLiteral(_)) = expression.inferred_type(model) else {
        return false;
    };
    let Some(TypeDefinition::Function(def)) = ty.definition(db, &model.program_environment())
    else {
        return false;
    };
    def.name(db).as_deref() == Some(name)
        && def.file(db).path(db).as_vendored_path().is_some()
        && model
            .resolve_module(Some(owner), 0)
            .and_then(|m| m.file(db))
            == Some(def.file(db))
}

/// An unannotated local function in a generator binds its arguments when iteration reaches the call.
fn untyped_generator_local(
    index: &ty_python_core::SemanticIndex,
    function: &ruff_python_ast::StmtFunctionDef,
) -> Option<ty_python_core::FileScopeId> {
    if !function.decorator_list.is_empty()
        || function.type_params.is_some()
        || function.returns.is_some()
        || function.parameters.iter().any(|p| p.annotation().is_some())
    {
        return None;
    }
    let local =
        index.try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(function))?;
    index
        .scope(local)
        .parent()
        .filter(|scope| scope.is_generator_function(index))
}

/// A context manager wraps a suspended generator. Its preserved annotation
/// does not describe the generator object constructed by the raw call.
fn contextmanager_annotation(db: &dyn Db, file: File, range: TextRange) -> bool {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
    use ty_python_core::scope::NodeWithScopeRef;
    let program = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, program.python_file(db)).load(db);
    let root = AnyNodeRef::from(parsed.syntax());
    if !root.range().contains_range(range) {
        return false;
    }
    let nodes = covering_node(root, range);
    let Some(function) = nodes.ancestors().find_map(|node| match node {
        AnyNodeRef::StmtFunctionDef(f) => Some(f),
        _ => None,
    }) else {
        return false;
    };
    if function.is_async
        || !function
            .returns
            .as_deref()
            .is_some_and(|a| a.range() == range)
    {
        return false;
    }
    let Some(decorator) = function.decorator_list.last() else {
        return false;
    };
    let index = ty_python_core::semantic_index(db, program);
    index
        .node_scope(NodeWithScopeRef::Function(function))
        .is_generator_function(index)
        && resolves_to_stdlib_function(
            &SemanticModel::new(db, program),
            &decorator.expression,
            "contextlib",
            "contextmanager",
        )
}

/// ty 0.0.14 counts functools.Placeholder as occupied and can leave bound
/// tuple fields open. Imre has already checked the exact remaining signature.
/// The zero-argument constructor also has a native runtime TypeError. Match
/// only the diagnosed invocation; unrelated binding errors remain diagnostics.
fn native_partial_call(db: &dyn Db, file: File, range: TextRange, rule: &str) -> bool {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
    use ty_python_semantic::types::{KnownClass, KnownInstanceType, ProgramEnvironment, Type};
    let file = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, file.python_file(db)).load(db);
    let root = AnyNodeRef::from(parsed.syntax());
    if !root.range().contains_range(range) {
        return false;
    }
    let nodes = covering_node(root, range);
    let Some(call) = nodes.ancestors().find_map(|node| match node {
        // An excess argument may itself be a call. Find the call that takes it.
        AnyNodeRef::ExprCall(call)
            if rule != "too-many-positional-arguments"
                || call.arguments.args.iter().any(|arg| arg.range() == range) =>
        {
            Some(call)
        }
        _ => None,
    }) else {
        return false;
    };
    let model = SemanticModel::new(db, file);
    let Some(callee) = call.func.inferred_type(&model) else {
        return false;
    };
    let is_partial = |ty| {
        matches!(
            ty,
            Type::KnownInstance(KnownInstanceType::FunctoolsPartial(_))
        )
    };
    // A branch can choose partials with different bound arguments but the same native ABI.
    if is_partial(callee)
        || matches!(callee, Type::Union(union) if union.elements(db).iter().copied().all(is_partial))
    {
        return rule == "too-many-positional-arguments"
            || (rule == "missing-argument" && call.range() == range);
    }
    if !matches!(callee, Type::ClassLiteral(_)) {
        return false;
    }
    let checked_constructor = match rule {
        "missing-argument" => call.arguments.args.is_empty() && call.range() == range,
        "too-many-positional-arguments" => call
            .arguments
            .args
            .first()
            .and_then(|target| target.inferred_type(&model))
            .is_some_and(is_partial),
        _ => false,
    };
    let env = ProgramEnvironment::from_file(file);
    checked_constructor
        && callee.is_assignable_to(
            db,
            &env,
            KnownClass::FunctoolsPartial.to_subclass_of(db, &env),
        )
}

#[cfg(test)]
thread_local! {
    static FILE_CHECKS: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

struct CachedSession {
    session: Session,
    inputs: crate::SourceInputs,
}

impl CachedSession {
    fn new(session: Session, inputs: &crate::SourceInputs) -> Self {
        Self {
            session,
            inputs: inputs.clone(),
        }
    }

    // A changed search root, import miss or file identity can change name resolution.
    fn same_files(&self, fresh: &Session, inputs: &crate::SourceInputs) -> bool {
        self.session.root == fresh.root
            && self.session.extension_roots == fresh.extension_roots
            && self.inputs.missing == inputs.missing
            && self.inputs.files.len() == inputs.files.len()
            && self
                .inputs
                .files
                .iter()
                .zip(&inputs.files)
                .all(|(old, new)| old.0 == new.0 && old.1 == new.1)
    }

    fn update(&mut self, fresh: &Session, inputs: &crate::SourceInputs) -> Result<(), String> {
        self.session.tuple_spreads.clone_from(&fresh.tuple_spreads);
        for (index, (old, new)) in self.inputs.files.iter_mut().zip(&inputs.files).enumerate() {
            if old.2 == new.2 {
                continue;
            }
            let source = ruff_db::source::source_text(&fresh.db, fresh.files[index]);
            let file = self.session.files[index];
            let path = new.0.to_str().ok_or("module paths must be valid UTF-8")?;
            self.session
                .db
                .system
                .fs()
                .write_file(path, source.as_str())
                .map_err(|error| error.to_string())?;
            // An equal-size edit can even have the same mtime. Use the normalized source
            // that passed preflight as the authoritative Salsa input.
            file.set_source_text_override(&mut self.session.db)
                .to(Some(source));
            old.2.clone_from(&new.2);
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct AnalysisCache {
    closed_diagnostics: Option<CachedSession>,
    diagnostics: Option<CachedSession>,
    hints: Option<CachedSession>,
}

impl AnalysisCache {
    pub(crate) fn restore_hints(
        &mut self,
        graph: &mut crate::modules::SourceGraph,
    ) -> Result<(), String> {
        if let Some(mut hints) = self.hints.take()
            && hints.same_files(&graph.session, &graph.inputs)
        {
            hints.update(&graph.session, &graph.inputs)?;
            // Imre stored statement indices while resolving the fresh source graph.
            // Lower the identical normalized sources using the ASTs owned by this ty database.
            for (module, &file) in graph.modules.iter_mut().zip(&hints.session.files) {
                module.parsed = hints.session.parsed(file);
            }
            graph.session = hints.session.clone();
            self.hints = Some(hints);
        }
        Ok(())
    }

    pub(crate) fn retain_hints(
        &mut self,
        graph: &crate::modules::SourceGraph,
        lowering_succeeded: bool,
    ) {
        if graph.session.hint_reads.take() != 0 {
            self.hints
                .get_or_insert_with(|| CachedSession::new(graph.session.clone(), &graph.inputs));
        } else if lowering_succeeded {
            // An error can stop lowering before the program reaches its first hint.
            self.hints = None;
        }
    }

    pub(crate) fn check_closed_program(
        &mut self,
        fresh: &mut Session,
        inputs: &crate::SourceInputs,
        modules: &[crate::modules::SourceModule],
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        native: Option<&crate::frontend::phase::Diagnostic>,
    ) -> Result<(), String> {
        if let Some(cached) = &mut self.closed_diagnostics
            && cached.same_files(fresh, inputs)
        {
            cached.update(fresh, inputs)?;
            return cached
                .session
                .closed_program_errors(modules, skipped, overloads, native);
        }
        // The optional layout cache may retain a quiet handle to fresh's Salsa storage.
        let mut session = fresh.diagnostic_copy()?;
        let result = session.check_closed_program(modules, skipped, overloads, native);
        self.closed_diagnostics = Some(CachedSession::new(session, inputs));
        result
    }

    pub(crate) fn check_scopes(
        &mut self,
        fresh: &mut Session,
        inputs: &crate::SourceInputs,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
    ) -> Result<(), String> {
        if let Some(cached) = &mut self.diagnostics
            && cached.same_files(fresh, inputs)
        {
            cached.update(fresh, inputs)?;
            return cached
                .session
                .errors(skipped, overloads, FileChecks::Full, None, None);
        }
        if self.hints.is_some() {
            // The quiet database must retain its inference with diagnostics disabled.
            let mut session = fresh.diagnostic_copy()?;
            let result = session.check_scopes(skipped, overloads);
            self.diagnostics = Some(CachedSession::new(session, inputs));
            result
        } else {
            let result = fresh.check_scopes(skipped, overloads);
            self.diagnostics = Some(CachedSession::new(fresh.clone(), inputs));
            result
        }
    }
}

const MAX_WITH_DEPTH: usize = 96;
const MAX_MATCH_FLOW_WORK: usize = 8 * 1024 * 1024;

// A loop transfer retains one semantic-index snapshot until its loop closes.
const MAX_LOOP_FLOW_WORK: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct LoopTransfers {
    count: usize,
    last: TextSize,
}

impl<'a> Visitor<'a> for LoopTransfers {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Break(_) | Stmt::Continue(_) => {
                self.count += 1;
                self.last = self.last.max(stmt.end());
            }
            // A transfer in a nested loop's else belongs to the outer loop.
            Stmt::For(node) => self.visit_body(&node.orelse),
            Stmt::While(node) => self.visit_body(&node.orelse),
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, _expr: &'a Expr) {}
}

/// Check code-scope restrictions shared by typed and object compilation.
pub(crate) fn check_scope_syntax(
    source: &str,
    module: &ruff_python_ast::ModModule,
) -> Result<(), String> {
    if let Some(range) = imre_ast::misplaced_star_import(&module.body) {
        return Err(crate::frontend::diagnostic_message(
            source,
            &range,
            "SyntaxError: import * only allowed at module level",
        ));
    }
    if let Some((range, message)) = imre_ast::async_generators::syntax_error(module) {
        return Err(crate::frontend::diagnostic_message(
            source,
            &range,
            &format!("SyntaxError: {message}"),
        ));
    }
    Ok(())
}

struct SuiteLimits<'t> {
    tokens: &'t Tokens,
    scope_start: TextSize,
    active_managers: usize,
    error: Option<String>,
}

impl SuiteLimits<'_> {
    fn exceeds_flow_work(&self, end: TextSize, exits: usize, budget: usize) -> bool {
        exits > 1
            && self
                .tokens
                .in_range(TextRange::new(self.scope_start, end))
                .len()
                > budget / exits
    }
}

impl<'a> Visitor<'a> for SuiteLimits<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.error.is_some() {
            return;
        }
        // ty saves a copy of the current scope's flow state for each case.
        // Tokens through the last case conservatively bound prior places,
        // including bindings introduced inside the match.
        if let Stmt::Match(node) = stmt
            && self.exceeds_flow_work(node.end(), node.cases.len(), MAX_MATCH_FLOW_WORK)
        {
            self.error = Some("Imre: match statement exceeds the flow analysis limit".into());
            return;
        }
        let loop_body = match stmt {
            Stmt::For(node) => Some(&node.body),
            Stmt::While(node) => Some(&node.body),
            _ => None,
        };
        if let Some(body) = loop_body {
            let mut transfers = LoopTransfers::default();
            transfers.visit_body(body);
            if self.exceeds_flow_work(transfers.last, transfers.count, MAX_LOOP_FLOW_WORK) {
                self.error = Some("Imre: loop transfers exceed the flow analysis limit".into());
                return;
            }
        }
        let outer = self.scope_start;
        if matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
            self.scope_start = stmt.start();
        }
        if let Stmt::With(scope) = stmt {
            // Each manager contributes a protected scope, including managers
            // that share one statement.
            self.active_managers += scope.items.len();
            if self.active_managers > MAX_WITH_DEPTH {
                self.error = Some(format!(
                    "Imre: context manager nesting exceeds {MAX_WITH_DEPTH} levels"
                ));
            } else {
                self.visit_body(&scope.body);
            }
            self.active_managers -= scope.items.len();
        } else {
            walk_stmt(self, stmt);
        }
        self.scope_start = outer;
    }

    fn visit_expr(&mut self, _expr: &'a Expr) {}
}

/// Ruff indexes the AST recursively as soon as the database parses a module.
/// Bound the depth before that happens, including chains with no parentheses.
pub(crate) fn check_source_complexity(source: &str) -> Result<(), &'static str> {
    const LIMIT: usize = 96;
    if source.len() < LIMIT {
        return Ok(());
    }

    #[derive(Default)]
    struct Group {
        operators: usize,
        primary_depth: usize,
        longest_primary: usize,
        longest_item: usize,
        enclosing_operators: usize,
        longest_boolean_operand: usize,
        boolean_operators: usize,
        open_lambda_parameters: usize,
        // Replacement fields are siblings in an f-string or template string.
        interpolated_string: bool,
        // Commas in a comprehension target belong to one assignment target.
        keep_commas: bool,
    }

    impl Group {
        fn operand_depth(&self) -> usize {
            self.operators + self.longest_primary.max(self.primary_depth)
        }

        fn depth(&self) -> usize {
            let operand = self.operand_depth();
            // A Boolean expression has at most an outer "or" and an inner "and".
            // Ruff stores the operands of each operator as siblings.
            let boolean = if self.boolean_operators == 0 {
                operand
            } else {
                2 + self.longest_boolean_operand.max(operand)
            };
            // ty tracks a flow branch at each short circuit, even for a flat AST.
            self.longest_item
                .max(self.enclosing_operators + boolean)
                .max(self.boolean_operators)
        }

        fn next_item(&mut self) {
            self.longest_item = self.depth();
            self.enclosing_operators = 0;
            self.longest_boolean_operand = 0;
            self.boolean_operators = 0;
            self.operators = 0;
            self.primary_depth = 0;
            self.longest_primary = 0;
        }
    }

    use TokenKind::*;
    let mut lexer = lex(source, Mode::Module);
    let mut parents = Vec::new();
    let mut group = Group::default();
    let mut indents = Vec::new();
    let mut elif_depth = 0;
    let mut statement_start = true;
    loop {
        let token = lexer.next_token();
        if statement_start
            && !matches!(
                token,
                Indent | Dedent | Newline | Comment | NonLogicalNewline
            )
        {
            // Each elif adds a nested false branch. Its suite inherits that
            // depth, while a new statement starts a fresh chain at this indent.
            match token {
                Elif => elif_depth += 1,
                Else => {}
                _ => elif_depth = indents.last().copied().unwrap_or(0),
            }
            statement_start = false;
        }
        match token {
            EndOfFile => return Ok(()),
            Indent => indents.push(elif_depth),
            Dedent => elif_depth = indents.pop().unwrap_or(0),
            Newline | Semi if parents.is_empty() => {
                group = Group::default();
                statement_start = token == Newline;
            }
            Lpar | Lsqb | Lbrace | FStringStart | TStringStart => {
                if token == Lbrace && group.interpolated_string {
                    group.next_item();
                }
                group.primary_depth += 1;
                parents.push(std::mem::take(&mut group));
                group.interpolated_string = matches!(token, FStringStart | TStringStart);
            }
            Rpar | Rsqb | Rbrace | FStringEnd | TStringEnd => {
                if let Some(mut parent) = parents.pop() {
                    parent.primary_depth = parent.primary_depth.max(1 + group.depth());
                    group = parent;
                }
            }
            Comma if !group.keep_commas && group.open_lambda_parameters == 0 => group.next_item(),
            And | Or => {
                let depth = group.operand_depth();
                group.longest_boolean_operand = group.longest_boolean_operand.max(depth);
                group.boolean_operators += 1;
                group.operators = 0;
                group.primary_depth = 0;
                group.longest_primary = 0;
            }
            Dot => group.primary_depth += 1,
            kind if kind.as_binary_operator().is_some() => {
                // In f() + g(), the calls are siblings. Count every operator on
                // the expression spine and the deepest of its primary operands.
                group.longest_primary = group.longest_primary.max(group.primary_depth);
                group.primary_depth = 0;
                group.operators += 1;
            }
            // These expressions can enclose Boolean operands across the entire item.
            Lambda => {
                group.open_lambda_parameters += 1;
                group.enclosing_operators += 1;
            }
            Colon if group.open_lambda_parameters > 0 => {
                group.open_lambda_parameters -= 1;
                group.operators += 1;
            }
            For => {
                group.keep_commas = true;
                group.enclosing_operators += 1;
            }
            If | Else | Yield | ColonEqual => group.enclosing_operators += 1,
            Identifier | Int | Float | Complex | String | True | False | None | Ellipsis
            | FStringMiddle | TStringMiddle | Comment | NonLogicalNewline => continue,
            _ => group.operators += 1,
        }
        if indents.len() + elif_depth + parents.len() + group.depth() > LIMIT {
            return Err("Imre: source exceeds the syntax complexity limit");
        }
    }
}

/// ty retains a snapshot of the scope's flow state for every handler in a try.
fn check_handler_count(tokens: &Tokens) -> Result<(), &'static str> {
    let mut outer = Vec::new();
    let mut handlers = 0;
    for token in tokens {
        match token.kind() {
            TokenKind::Indent => outer.push(std::mem::take(&mut handlers)),
            TokenKind::Dedent => handlers = outer.pop().unwrap_or_default(),
            TokenKind::Try => handlers = 0,
            TokenKind::Except => {
                handlers += 1;
                if handlers > 96 {
                    return Err("Imre: try statement exceeds the exception handler limit");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Apply the file-level restriction on form feeds, including formatted-string replacement fields.
pub(crate) fn check_form_feeds(source: &str, tokens: &Tokens) -> Result<(), String> {
    if !source.contains('\x0c') {
        return Ok(());
    }
    use TokenKind::*;
    let mut statement_start = true;
    let mut formatted_strings = 0;
    for token in tokens {
        match token.kind() {
            Newline => {
                statement_start = true;
                continue;
            }
            NonLogicalNewline => {
                // Replacement expressions in formatted strings also check physical indentation.
                statement_start |= formatted_strings > 0;
                continue;
            }
            Indent | Dedent | Comment | EndOfFile => continue,
            FStringStart | TStringStart => formatted_strings += 1,
            FStringEnd | TStringEnd => formatted_strings -= 1,
            _ => {}
        }
        if statement_start {
            let prefix = &source[..usize::from(token.start())];
            let indentation = prefix.rsplit_once('\n').map_or(prefix, |(_, line)| line);
            if indentation.contains('\x0c') {
                let line = prefix.lines().count();
                return Err(format!(
                    "line {line}: form feeds are unsupported in indentation"
                ));
            }
            statement_start = false;
        }
    }
    Ok(())
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl ruff_db::Db for Database {
    fn vendored(&self) -> &VendoredFileSystem {
        &self.vendored
    }

    fn system(&self) -> &dyn System {
        &self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }
}

#[salsa::db]
impl ty_module_resolver::Db for Database {}

#[salsa::db]
impl ty_python_core::Db for Database {
    fn should_check_file(&self, file: File) -> bool {
        !file.path(self).is_vendored_path()
    }
}

#[salsa::db]
impl Db for Database {
    fn check_file(&self, file: File) -> Vec<Diagnostic> {
        check_file_unwrap(self, self.program_file(file))
    }

    fn program_file(&self, file: File) -> ProgramFile<'_> {
        Program::from_settings(self, &self.settings).program_file(self, file)
    }

    fn python_version_with_source(&self, _file: File) -> &PythonVersionWithSource {
        &self.settings.python_version
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        DiagnosticRules::get(self).rules(self)
    }

    fn lint_registry(&self) -> &LintRegistry {
        default_lint_registry()
    }

    fn analysis_settings(&self, _file: File) -> &AnalysisSettings {
        &self.analysis
    }

    fn dependency_metadata(&self, _file: File) -> Option<&DependencyMetadata> {
        None
    }

    fn verbose(&self) -> bool {
        false
    }

    fn is_open_file(&self, _file: File) -> bool {
        false
    }

    fn dyn_clone(&self) -> Box<dyn Db> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod diagnostic_mode_tests {
    use super::*;

    #[test]
    fn named_calls_check_imported_functions_and_leave_method_dispatch_to_the_provider() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        session
            .add(
                &root.join("scores.py"),
                "def take(n: int) -> int: return n\n",
                crate::SourceKind::File,
            )
            .unwrap();
        session
            .add(
                &root.join("main.py"),
                concat!(
                    "from scores import take\n",
                    "class Receiver:\n    def method(self, n: int) -> int: return n\n",
                    "Receiver().method('object')\ntake('x')\n",
                ),
                crate::SourceKind::File,
            )
            .unwrap();
        let errors = session
            .check_fallback_contracts(None, &[], FallbackCalls::NamedFunctions, None, None)
            .unwrap_err();
        assert_eq!(
            errors.matches("error[invalid-argument-type]").count(),
            1,
            "{errors}"
        );
        assert!(errors.contains("function `take`"), "{errors}");
    }

    #[test]
    fn data_module_skips_only_fallback_contracts() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        session
            .add(
                &root.join("main.py"),
                "from missing_dependency import value\n",
                crate::SourceKind::File,
            )
            .unwrap();
        FILE_CHECKS.with_borrow_mut(|checks| *checks = Some(Vec::new()));
        session
            .check_fallback_contracts(None, &[vec![]], FallbackCalls::All, None, None)
            .unwrap();
        let visits = FILE_CHECKS.with_borrow_mut(Option::take).unwrap();
        assert!(visits.is_empty());
        assert!(
            session
                .check_scopes(None, &[])
                .unwrap_err()
                .contains("missing_dependency")
        );
    }

    #[test]
    fn a_proved_builtin_import_keeps_the_adjacent_ty_error() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        session
            .add(
                &root.join("main.py"),
                "import _opcode, missing_peer\n",
                crate::SourceKind::File,
            )
            .unwrap();
        let builtin = TextRange::new(7.into(), 14.into());
        let peer = TextRange::new(16.into(), 28.into());
        let proof = CheckedDiagnostic::BuiltinImport;
        let without_proofs = session.check_scopes(None, &[]).unwrap_err();
        let adjacent = session
            .check_scopes(None, &[vec![proof(builtin)]])
            .unwrap_err();
        let builtin_only = session
            .check_scopes(None, &[vec![proof(peer)]])
            .unwrap_err();
        assert_ne!(adjacent, builtin_only);
        assert_ne!(without_proofs, adjacent);
        session
            .check_scopes(None, &[vec![proof(builtin), proof(peer)]])
            .unwrap();
        // Even a range enclosing both aliases cannot justify either import.
        assert_eq!(
            session
                .check_scopes(None, &[vec![proof(builtin.cover(peer))]])
                .unwrap_err(),
            without_proofs,
        );
    }

    #[test]
    fn enabling_diagnostics_invalidates_quiet_checks() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        session
            .add(
                &root.join("main.py"),
                "def f(x: int) -> None:\n    x()\n",
                crate::SourceKind::File,
            )
            .unwrap();
        let file = session.files[0];
        assert!(
            !session
                .db
                .check_file(file)
                .iter()
                .any(|d| d.severity() >= Severity::Error)
        );
        let message = session.check_scopes(None, &[]).unwrap_err();
        assert!(message.contains("not callable"), "{message}");
        // Entry profiles can skip a scope without changing its source or Salsa revision.
        let whole = TextRange::new(0.into(), 40.into());
        assert!(
            session
                .errors(Some(&[vec![whole]]), &[], FileChecks::Full, None, None)
                .is_ok()
        );
        assert_eq!(
            session
                .errors(None, &[], FileChecks::Full, None, None)
                .unwrap_err(),
            message
        );
    }
}
