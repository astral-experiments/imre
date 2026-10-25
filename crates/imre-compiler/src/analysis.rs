//! Hosts ty's real semantic analysis and Ruff's cached parser over one source snapshot.

#[cfg(test)]
mod incremental_tests;
mod inferred_shapes;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruff_db::diagnostic::{Diagnostic, DisplayDiagnosticConfig, Severity};
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
    rules: Arc<RuleSelection>,
    root: PathBuf,
    extension_roots: Vec<PathBuf>,
    used_hints: Cell<bool>,
}

impl Session {
    pub(crate) fn new(root: &Path, extension_roots: &[PathBuf]) -> Result<Self, String> {
        let root = root.to_str().ok_or("module paths must be valid UTF-8")?;
        let system = InMemorySystem::default();
        system
            .fs()
            .create_directory_all(root)
            .map_err(|error| error.to_string())?;
        // The loader reserves this namespace for sealed compiler intrinsics.
        // Give ty its exact public signature without loading the dynamic CPython
        // compatibility implementation or weakening unresolved-import checks.
        let intrinsic = SystemPathBuf::from(root).join("imre");
        for (name, stub) in [
            (
                "hashlib",
                // The bundled OpenSSL stub still names the input `string`. CPython
                // 3.14 calls it `data`; the native constructor takes exact bytes.
                "from _hashlib import HASH as _Hash\nalgorithms_available: set[str]\ndef sha256(data: bytes = b'', *, usedforsecurity: bool = True) -> _Hash: ...\ndef new(name: str, data: bytes = b'', *, usedforsecurity: bool = True) -> _Hash: ...\n",
            ),
            (
                "tomli",
                "from tomllib import load as load, loads as loads\n",
            ),
            (
                "unidiff",
                "class PatchSet:\n    def __init__(self, patch: str) -> None: ...\n",
            ),
            (
                "tomli_w",
                "from typing import Any, BinaryIO\ndef dumps(obj: dict[str, Any]) -> str: ...\ndef dump(obj: dict[str, Any], fp: BinaryIO) -> None: ...\n",
            ),
        ] {
            system
                .fs()
                .write_file_all(SystemPathBuf::from(root).join(format!("{name}.pyi")), stub)
                .map_err(|error| error.to_string())?;
        }
        system
            .fs()
            .write_file_all(
                intrinsic.join("__init__.pyi"),
                "from . import json as json, toml as toml\n",
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
        // CPython's POSIX which() accepts a byte search path independently of
        // the command's type. The cross-platform typeshed overloads omit it.
        system.fs().write_file_all(
            SystemPathBuf::from(root).join("shutil.pyi"),
            "from os import PathLike\nfrom typing import TypeVar\n_S = TypeVar('_S', str, bytes)\ndef which(cmd: _S | PathLike[_S], mode: int = ..., path: str | bytes | PathLike[str] | None = ...) -> _S | None: ...\n",
        ).map_err(|error| error.to_string())?;
        let mut settings = ProgramSettings::empty(&vendored);
        settings.python_version.version = ruff_python_ast::PythonVersion::PY314;
        let mut roots = vec![SystemPathBuf::from(root)];
        for path in extension_roots {
            let path = path.to_str().ok_or("extension paths must be valid UTF-8")?;
            system
                .fs()
                .create_directory_all(path)
                .map_err(|error| error.to_string())?;
            roots.push(SystemPathBuf::from(path));
        }
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
            storage: salsa::Storage::default(),
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
            rules: Arc::new(rules),
            root: PathBuf::from(root),
            extension_roots: extension_roots.to_vec(),
            used_hints: Cell::new(false),
        })
    }

    /// Parses a normalized source once; semantic checks wait until every import is present.
    pub(crate) fn add(&mut self, path: &Path, source: &str) -> Result<ParsedModuleRef, String> {
        let path = path.to_str().ok_or("module paths must be valid UTF-8")?;
        check_source_complexity(source).map_err(|error| format!("{path}: {error}"))?;
        let file = self.write_file(path, source)?;
        let parsed = self.parsed(file);
        if let Some(error) = parsed.errors().first() {
            return Err(format!("{path}: {error}"));
        }
        check_handler_count(parsed.tokens()).map_err(|error| format!("{path}: {error}"))?;
        check_indentation(source, parsed.tokens()).map_err(|error| format!("{path}: {error}"))?;
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
        // ty materializes every field of a starred tuple display before lowering.
        // Imre supports starred assignment targets and list displays.
        if parsed
            .tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::Star)
            && ruff_python_ast::helpers::any_over_body(&parsed.syntax().body, |expr| {
                matches!(expr, Expr::Tuple(tuple) if tuple.ctx == ruff_python_ast::ExprContext::Load
                    && tuple.elts.iter().any(|elt| matches!(elt, Expr::Starred(_))))
            })
        {
            return Err(format!(
                "{path}: Imre: starred elements in tuple displays are unsupported"
            ));
        }
        inferred_shapes::check(&parsed.syntax().body, parsed.tokens().len())
            .map_err(|error| format!("{path}: {error}"))?;
        self.files.push(file);
        Ok(parsed)
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

    /// Start diagnostics with independent rules and Salsa storage. Every file has already
    /// passed the loader's preflight in this snapshot.
    fn diagnostic_copy(&self) -> Result<Self, String> {
        let mut copy = Self::new(&self.root, &self.extension_roots)?;
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

    pub(crate) fn data_attribute(&self, module: usize, expression: &ruff_python_ast::Expr) -> bool {
        use ty_python_semantic::types::Type as Ty;
        self.used_hints.set(true);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(
            expression.inferred_type(&model),
            Some(Ty::NominalInstance(_) | Ty::LiteralValue(_))
        )
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
        self.used_hints.set(true);
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
        self.used_hints.set(true);
        let program = self.db.program_file(self.files[module]);
        let model = SemanticModel::new(&self.db, program);
        let Some(ty @ Ty::NominalInstance(_)) = expression.inferred_type(&model) else {
            return false;
        };
        ty != KnownClass::NoneType.to_instance(&self.db, &ProgramEnvironment::from_file(program))
    }

    /// Get a scalar layout hint for an append operand in its own lexical scope.
    pub(crate) fn scalar_type(
        &self,
        module: usize,
        value: &ruff_python_ast::Expr,
    ) -> Option<crate::ir::Type> {
        use crate::ir::Type;
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};

        self.used_hints.set(true);
        let file = self.db.program_file(self.files[module]);
        let ty = value.inferred_type(&SemanticModel::new(&self.db, file))?;
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

    pub(crate) fn is_never(&self, module: usize, value: &ruff_python_ast::Expr) -> bool {
        self.used_hints.set(true);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(
            value.inferred_type(&model),
            Some(ty_python_semantic::types::Type::Never)
        )
    }

    /// Checks every loaded file, including unused functions and inactive main guards.
    pub(crate) fn check_scopes(
        &mut self,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedOverload>],
    ) -> Result<(), String> {
        DiagnosticRules::get(&self.db)
            .set_rules(&mut self.db)
            .to(self.rules.clone());
        self.errors(skipped, overloads)
    }

    fn errors(
        &self,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedOverload>],
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
                );
                file_errors(&self.db, checked)
            })
            .map(String::as_str)
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
    overloads: Vec<CheckedOverload>,
}

/// The frontend validates context-manager decorators and the exact calls
/// it lowers with a signature different from typeshed's overloads.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CheckedOverload {
    Decorator(TextRange),
    Call(TextRange),
}

impl CheckedOverload {
    fn covers(self, range: TextRange) -> bool {
        match self {
            Self::Decorator(decorator) => decorator.contains_range(range),
            Self::Call(call) => call == range,
        }
    }
}

/// A per-file result depends on the ty queries it actually read, including any other source
/// consulted to render a diagnostic. Every revision still requests the result for every file.
#[salsa::tracked(returns(ref))]
fn file_errors<'db>(db: &'db dyn Db, checked: CheckedFile<'db>) -> Vec<String> {
    let file = checked.file(db);
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
                && !diagnostic
                    .primary_span()
                    .and_then(|span| span.range())
                    .is_some_and(|range| {
                        skipped.iter().any(|scope| scope.contains_range(range))
                            || (diagnostic.id().as_str() == "no-matching-overload"
                                && overloads.iter().any(|scope| scope.covers(range)))
                    })
        })
        .map(|diagnostic| {
            diagnostic
                .display(
                    &(db as &dyn ruff_db::Db),
                    &DisplayDiagnosticConfig::new("imre"),
                )
                .to_string()
        })
        .collect()
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
        if graph.session.used_hints.take() {
            self.hints
                .get_or_insert_with(|| CachedSession::new(graph.session.clone(), &graph.inputs));
        } else if lowering_succeeded {
            // An error can stop lowering before the program reaches its first hint.
            self.hints = None;
        }
    }

    pub(crate) fn check_scopes(
        &mut self,
        fresh: &mut Session,
        inputs: &crate::SourceInputs,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedOverload>],
    ) -> Result<(), String> {
        if let Some(cached) = &mut self.diagnostics
            && cached.same_files(fresh, inputs)
        {
            cached.update(fresh, inputs)?;
            return cached.session.errors(skipped, overloads);
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
fn check_source_complexity(source: &str) -> Result<(), &'static str> {
    const LIMIT: usize = 96;
    if source.len() < LIMIT {
        return Ok(());
    }

    #[derive(Default)]
    struct Group {
        operators: usize,
        child_depth: usize,
        longest_item: usize,
        // Replacement fields are siblings in an f-string or template string.
        interpolated_string: bool,
        // A comma in lambda parameters or a comprehension target does not
        // separate independent expressions at this delimiter level.
        keep_commas: bool,
    }

    impl Group {
        fn depth(&self) -> usize {
            self.longest_item.max(self.operators + self.child_depth)
        }

        fn next_item(&mut self) {
            self.longest_item = self.depth();
            self.operators = 0;
            self.child_depth = 0;
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
                group.operators += 1;
                parents.push(std::mem::take(&mut group));
                group.interpolated_string = matches!(token, FStringStart | TStringStart);
            }
            Rpar | Rsqb | Rbrace | FStringEnd | TStringEnd => {
                if let Some(mut parent) = parents.pop() {
                    parent.child_depth = parent.child_depth.max(group.depth());
                    group = parent;
                }
            }
            Comma if !group.keep_commas => group.next_item(),
            Lambda | For => {
                group.keep_commas = true;
                group.operators += 1;
            }
            Name | Int | Float | Complex | String | True | False | None | Ellipsis
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

/// Rejects ambiguous indentation while preserving whitespace inside string tokens.
fn check_indentation(source: &str, tokens: &Tokens) -> Result<(), String> {
    let mut offset = 0;
    for (line_number, line) in source.split_inclusive('\n').enumerate() {
        for (index, character) in line.char_indices() {
            if !matches!(character, ' ' | '\t' | '\x0c') {
                break;
            }
            if character != ' ' {
                let position = TextSize::try_from(offset + index)
                    .map_err(|_| "source exceeds the supported size")?;
                let in_string = tokens.at_offset(position).any(|token| {
                    matches!(token.kind(), TokenKind::String | TokenKind::FStringMiddle)
                        && token.range().contains(position)
                });
                if !in_string {
                    return Err(format!(
                        "line {}: indentation must use spaces, not tabs or form feeds",
                        line_number + 1
                    ));
                }
            }
        }
        offset += line.len();
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
    fn enabling_diagnostics_invalidates_quiet_checks() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        session
            .add(&root.join("main.py"), "def f(x: int) -> None:\n    x()\n")
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
        assert!(session.errors(Some(&[vec![whole]]), &[]).is_ok());
        assert_eq!(session.errors(None, &[]).unwrap_err(), message);
    }
}
