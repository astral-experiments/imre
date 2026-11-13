//! Hosts ty's real semantic analysis and Ruff's cached parser over one source snapshot.

mod class_cell;
#[cfg(test)]
mod closed_program_tests;
mod execution;
#[cfg(test)]
mod incremental_tests;
mod inferred_shapes;
mod initializer_calls;
pub(crate) use initializer_calls::{
    InitializerCheck, inert_declaration, passive_function, unambiguous_file_module,
};
mod run_path;
mod shipped_exports;
mod tuple_displays;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruff_db::diagnostic::{Diagnostic, DiagnosticId, DisplayDiagnosticConfig, Severity};
use ruff_db::files::{File, Files, system_path_to_file};
use ruff_db::parsed::ParsedModuleRef;
use ruff_db::system::{InMemorySystem, System, SystemPath, SystemPathBuf};
use ruff_db::vendored::VendoredFileSystem;
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_ast::visitor::{Visitor, walk_stmt};
use ruff_python_ast::{Expr, ExprCall, ExprRef, Stmt};
use ruff_python_parser::{Mode, lexer::lex};
use ruff_text_size::{Ranged, TextRange, TextSize};
use salsa::Setter;
use ty_python_core::program::ProgramSettings;
use ty_python_core::scope::FileScopeId;
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

/// The eager body check reads the lowering filesystem with independent diagnostic rules.
pub(crate) struct RegisteredBodyChecks(Database);

impl RegisteredBodyChecks {
    pub(crate) fn source_error_ranges(
        &self,
        source: &Session,
        module: usize,
    ) -> Option<Vec<Option<TextRange>>> {
        let path = source.files[module].path(&source.db);
        let file = system_path_to_file(&self.0, path.as_str()).ok()?;
        Some(self.0.source_error_ranges(file))
    }
}

impl Database {
    fn source_error_ranges(&self, file: File) -> Vec<Option<TextRange>> {
        self.check_file(file)
            .into_iter()
            .filter(|diagnostic| diagnostic.severity() >= Severity::Error)
            .map(|diagnostic| diagnostic.primary_span().and_then(|span| span.range()))
            .collect()
    }
}

pub(crate) enum AnnotationClass {
    Linked(usize, TextRange),
    Declaration(String),
}

/// A project database shared by ty's checks and Ruff's cached parser.
#[derive(Clone)]
pub(crate) struct Session {
    db: Database,
    files: Vec<File>,
    /// Physical annotation providers with no runtime module or executable body.
    declarations: Vec<File>,
    external_loader_declarations: File,
    execution_names: File,
    tuple_spreads: Vec<HashMap<TextSize, usize>>,
    rules: Arc<RuleSelection>,
    pub(crate) root: PathBuf,
    extension_roots: Vec<PathBuf>,
    hint_reads: Cell<usize>,
    /// Explicit checks can report conversions that executables perform at runtime.
    pub(crate) explicit_check: bool,
}

/// The fixed checker's declaration proof can be consumed by more than one
/// registration. A failed cold lowering does not need that proof until its
/// original bindings are selected.
pub(crate) enum SourceContract {
    Checked(Result<(), String>),
    Pending {
        native: Box<crate::frontend::phase::Diagnostic>,
        result: std::cell::OnceCell<Result<(), String>>,
    },
}

impl SourceContract {
    pub(crate) fn original(&self, graph: &crate::modules::SourceGraph) -> &Result<(), String> {
        match self {
            Self::Checked(result) => result,
            Self::Pending { native, result } => result.get_or_init(|| {
                let (skipped, overloads) = graph.diagnostic_scopes();
                graph.session.closed_program_errors(
                    &graph.modules,
                    skipped.as_deref(),
                    &overloads,
                    Some(native),
                )
            }),
        }
    }
}

/// Source ownership used by the same diagnostics for checked graphs and object registrations.
pub(crate) trait ClosedSource {
    fn project_path(&self) -> Option<&Path>;
    /// The project source and module ID used by the original fixed checker.
    fn checked_source(&self, module: usize) -> Option<(usize, &Path)> {
        Some((module, self.project_path()?))
    }
    fn supplies_declarations(&self) -> bool {
        true
    }
}

impl ClosedSource for crate::modules::SourceModule {
    fn project_path(&self) -> Option<&Path> {
        self.checks_source_contracts().then_some(&self.path)
    }
    fn supplies_declarations(&self) -> bool {
        !self.bundled && !self.is_namespace()
    }
}

impl ClosedSource for (PathBuf, bool, Option<usize>) {
    fn project_path(&self) -> Option<&Path> {
        self.1.then_some(&self.0)
    }

    fn checked_source(&self, _: usize) -> Option<(usize, &Path)> {
        Some((self.2?, self.project_path()?))
    }
}

/// The selected source checker and its physical registration-to-file mapping.
pub(crate) struct RegisteredLayouts {
    session: Session,
    modules: HashMap<usize, usize>,
}

impl std::fmt::Debug for RegisteredLayouts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegisteredLayouts")
            .field("modules", &self.modules)
            .finish_non_exhaustive()
    }
}

impl RegisteredLayouts {
    fn from_winners<'a>(
        session: impl FnOnce() -> Session,
        winners: impl Iterator<Item = (usize, crate::dynamic::RegisteredSource<'a>)>,
    ) -> Option<Self> {
        let modules: HashMap<_, _> = winners
            .filter(|(_, winner)| winner.declares_signatures)
            .filter_map(|(module, winner)| Some((winner.unit?, module)))
            .collect();
        (!modules.is_empty()).then(|| Self {
            session: session(),
            modules,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub(crate) fn take(&mut self, unit: usize) -> Option<OperandTypes<'_>> {
        Some(self.session.operand_types(self.modules.remove(&unit)?))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct OperandTypes<'a> {
    session: &'a Session,
    module: usize,
}

#[derive(Clone, Copy)]
enum ScalarMatch {
    Assignable,
    Exact,
}

impl OperandTypes<'_> {
    pub(crate) fn matches_source(self, text: &str) -> bool {
        self.session.matches_parser_input(self.module, text)
    }

    pub(crate) fn parsed(self) -> ParsedModuleRef {
        self.session.parsed(self.session.files[self.module])
    }

    pub(crate) fn scalar_type(self, operand: &Expr) -> Option<crate::ir::Type> {
        self.checked_scalar(operand.into(), ScalarMatch::Assignable)
    }

    /// The untaken continuation needs the exact result of the original call.
    pub(crate) fn stopped_call_result(self, call: &ExprCall) -> Option<crate::ir::Type> {
        self.checked_scalar(call.into(), ScalarMatch::Exact)
    }

    fn checked_scalar(self, operand: ExprRef<'_>, rule: ScalarMatch) -> Option<crate::ir::Type> {
        let parsed = self.parsed();
        let (original, _) = self.original_operand(&parsed, operand)?;
        self.session
            .inferred_scalar_layout(self.module, original, rule)
    }

    /// Resolve a deferred physical name read to the binding in its module frame.
    pub(crate) fn reads_deferred_global(self, expression: &Expr) -> bool {
        let parsed = self.parsed();
        let Some((Expr::Name(name), scope)) = self.original_operand(&parsed, expression.into())
        else {
            return false;
        };
        let session = self.session;
        let file = session.files[self.module];
        *deferred_global(&session.db, file, scope, name.id.clone())
    }

    pub(crate) fn is_bound_method(self, method: &Expr) -> bool {
        let parsed = self.parsed();
        let Some((original, _)) = self.original_operand(&parsed, method.into()) else {
            return false;
        };
        let session = self.session;
        let file = session.db.program_file(session.files[self.module]);
        let model = SemanticModel::new(&session.db, file);
        use ty_python_semantic::types::Type;
        let bound = |ty| matches!(ty, Type::BoundMethod(_));
        match original.inferred_type(&model) {
            Some(Type::Union(union)) => union.elements(&session.db).iter().copied().all(bound),
            Some(ty) => bound(ty),
            None => false,
        }
    }

    fn original_operand<'p>(
        self,
        parsed: &'p ParsedModuleRef,
        operand: ExprRef<'_>,
    ) -> Option<(&'p Expr, FileScopeId)> {
        use ruff_python_ast::{AnyRootNodeRef, ExprContext, HasNodeIndex};
        let session = self.session;
        let index = ty_python_core::semantic_index(
            &session.db,
            session.db.program_file(session.files[self.module]),
        );
        // Forwarding may preserve a node index while changing the expression it evaluates.
        let scope = index.try_expression_scope_id(&operand)?;
        let AnyRootNodeRef::Expr(original) = parsed.get_by_index(operand.node_index().load())
        else {
            return None;
        };
        if ExprRef::from(original) != operand
            || original
                .as_name_expr()
                .is_some_and(|name| name.ctx != ExprContext::Load)
        {
            return None;
        }
        Some((original, scope))
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
    has_file_body: Option<&dyn Fn(&str) -> bool>,
    checked: Option<(
        &crate::modules::SourceGraph,
        &crate::dynamic::ProviderSources<'_>,
    )>,
    refusal: Option<&crate::frontend::phase::Diagnostic>,
) -> Result<Option<RegisteredLayouts>, String> {
    let sources = sources.into_iter();
    if sources.len() == 0 {
        return Ok(None);
    }
    let (checked, providers) = checked.unzip();
    let declarations = providers
        .into_iter()
        .flat_map(|p| p.values().copied())
        .filter(|(path, _)| path.extension().is_some_and(|ext| ext == "pyi"));
    let entry_operand = checked.zip(refusal).and_then(|(graph, report)| {
        let site = report.entry_operand()?;
        if site.module != graph.entry {
            return None;
        }
        let reached =
            graph
                .session
                .unconditional_module_operand(site.module, site.statement, site.operand);
        reached.then(|| {
            (
                site.module,
                graph.modules[site.module].original_source(),
                report,
            )
        })
    });
    let called_body = checked.zip(refusal).and_then(|(graph, report)| {
        let (id, range) = report.function_site()?;
        let module = &graph.modules[id];
        Some((&module.path, module.original_source(), range, report))
    });
    if let Some(graph) = checked
        && !graph.has_checked_only_import
        // This result belongs to the checked adapter, before a native declaration won.
        && declarations.clone().next().is_none()
        && let Some(contract) = &graph.source_contract
    {
        let mut matched = std::collections::HashSet::new();
        let admitted = sources.clone().all(|winner| {
            let Some(id) = winner.checked_module else {
                return false;
            };
            if winner.is_program != graph.modules[id].checks_source_contracts()
                || !matched.insert(id)
            {
                return false;
            }
            if winner.name == "__main__" {
                return true;
            }
            // A checked bundled declaration can mask a different runtime definition.
            let session = &graph.session;
            let file = session.files[id];
            let model = SemanticModel::new(&session.db, session.db.program_file(file));
            model
                .resolve_module(Some(winner.name), 0)
                .and_then(|m| m.file(&session.db))
                == Some(file)
        });
        if admitted
            && graph.modules.iter().enumerate().all(|(id, module)| {
                module.bundled
                    || module.is_namespace()
                    || module.path.extension().is_some_and(|ext| ext == "pyi")
                    || matched.contains(&id)
            })
        {
            let winners = sources.map(|winner| (winner.checked_module.unwrap(), winner));
            graph.session.check_registered_execution(
                contract.original(graph).clone(),
                winners.clone(),
                has_file_body,
                entry_operand,
                called_body,
                Some(graph),
            )?;
            return Ok(RegisteredLayouts::from_winners(
                || graph.session.clone(),
                winners.filter(|(id, winner)| {
                    let physical = &graph.modules[*id];
                    physical.path == winner.path && physical.original_source() == winner.source
                }),
            ));
        }
    }

    let mut session =
        Session::with_storage(root, extension_roots, salsa::Storage::default(), true)?;
    let mut paths = Vec::with_capacity(sources.len());
    for (path, source) in declarations {
        session.add(path, source, crate::SourceKind::File)?;
        paths.push((path.to_owned(), false, None));
    }
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
                paths.push((module.path.clone(), false, None));
            }
        }
    }
    let first_winner = paths.len();
    for winner in sources.clone() {
        let crate::dynamic::RegisteredSource {
            path,
            source,
            is_program,
            kind,
            ..
        } = winner;
        let path = if kind == crate::SourceKind::Command {
            root.join("<string>.py")
        } else {
            path.to_owned()
        };
        let source = crate::normalize_source(source, kind)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        session.add(&path, &source, kind)?;
        let original = winner.checked_module.filter(|&id| {
            winner.declares_signatures
                && is_program
                && checked.is_some_and(|graph| {
                    let module = &graph.modules[id];
                    module.checks_source_contracts()
                        && module.path == path
                        && module.original_source() == winner.source
                })
        });
        paths.push((path, is_program, original));
    }
    let contracts = session.check_closed_program(&paths, None, &[], refusal);
    let winners = (first_winner..).zip(sources);
    session.check_registered_execution(
        contracts,
        winners.clone(),
        has_file_body,
        entry_operand,
        called_body,
        None,
    )?;
    Ok(RegisteredLayouts::from_winners(|| session, winners))
}

#[salsa::tracked]
fn deferred_global(
    db: &dyn Db,
    file: File,
    scope: FileScopeId,
    name: ruff_python_ast::name::Name,
) -> bool {
    let index = ty_python_core::semantic_index(db, db.program_file(file));
    let Some(symbol) = index.place_table(scope).symbol_id(name.as_str()) else {
        return false;
    };
    index.symbol_resolves_to_global_scope(symbol, scope)
        && index
            .place_table(FileScopeId::global())
            .symbol_by_name(name.as_str())
            .is_some_and(|symbol| symbol.is_bound())
        && index
            .ancestor_scopes(scope)
            .any(|(_, scope)| !scope.is_eager())
}

impl Session {
    /// The class namespace supplies a name before Python falls back to its module.
    pub(crate) fn reads_outer_name(&self, module: usize, expression: &Expr) -> bool {
        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        index
            .try_expression_scope_id(expression)
            .zip(index.try_expression_use_id(expression.into()))
            .is_some_and(|(scope, use_id)| {
                !index
                    .use_def_map(scope)
                    .bindings_at_use(use_id)
                    .any(|binding| binding.binding.definition().is_some())
            })
    }

    /// A plain store can still initialize a name declared by an earlier annotation.
    pub(crate) fn unannotated_module_store(&self, module: usize, target: &Expr) -> bool {
        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        let Some([definition]) = index.try_definitions(target.into()) else {
            return false;
        };
        !index
            .use_def_map(FileScopeId::global())
            .declarations_at_binding(*definition)
            .any(|item| item.declaration.definition().is_some())
    }

    /// A single definition in the enclosing scope can be captured by value
    /// when no nested function can write the binding through a nonlocal cell.
    pub(crate) fn stable_enclosing_binding(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
        name: &str,
    ) -> bool {
        use ty_python_core::scope::NodeWithScopeRef;

        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        let Some(inner) = index.try_node_scope(NodeWithScopeRef::Function(function)) else {
            return false;
        };
        let Some((outer, _)) = index.ancestor_scopes(inner).nth(1) else {
            return false;
        };
        let places = index.place_table(outer);
        let Some(symbol) = places.symbol_id(name) else {
            return false;
        };
        places.symbol(symbol).is_local()
            && index
                .use_def_map(outer)
                .definitions_with_usage()
                .filter(|(_, def, _)| def.place(&self.db).as_symbol() == Some(symbol))
                .count()
                == 1
            && !index.scope_ids().any(|scope| {
                let scope = scope.file_scope_id(&self.db);
                index
                    .place_table(scope)
                    .symbol_by_name(name)
                    .is_some_and(|s| s.is_nonlocal() && s.is_bound())
                    && index.ancestor_scopes(scope).any(|(id, _)| id == outer)
            })
    }

    /// This scope has only local values and the supplied fixed globals, with no member or import bindings.
    pub(crate) fn function_uses_fixed_names(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
        eager_displays: bool,
        fixed_global: impl Fn(&str) -> bool,
    ) -> bool {
        self.function_reads_only(module, function, eager_displays, |_| true, fixed_global)
    }

    /// Check a fixed body with separate rules for reads of local and global names.
    pub(crate) fn function_reads_only(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
        eager_displays: bool,
        allowed_local_read: impl Fn(&str) -> bool,
        fixed_global: impl Fn(&str) -> bool,
    ) -> bool {
        use ty_python_core::scope::{NodeWithScopeKind, NodeWithScopeRef};

        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        let Some(scope) = index.try_node_scope(NodeWithScopeRef::Function(function)) else {
            return false;
        };
        let fixed = |scope| {
            let places = index.place_table(scope);
            places.members().next().is_none()
                && places.symbols().all(|symbol| {
                    if symbol.is_local() {
                        !symbol.is_used() || allowed_local_read(symbol.name().as_str())
                    } else {
                        fixed_global(symbol.name().as_str())
                    }
                })
                && !index
                    .use_def_map(scope)
                    .definitions_with_usage()
                    .any(|(_, definition, _)| definition.kind(&self.db).is_import())
        };
        !function.is_async
            && !scope.is_generator_function(index)
            && fixed(scope)
            && index.child_scopes(scope).all(|(id, child)| {
                eager_displays
                    && matches!(
                        child.node(),
                        NodeWithScopeKind::ListComprehension(_)
                            | NodeWithScopeKind::SetComprehension(_)
                            | NodeWithScopeKind::DictComprehension(_)
                    )
                    && index.child_scopes(id).next().is_none()
                    && fixed(id)
            })
    }

    /// Query the already indexed declarations in this physical scope.
    pub(crate) fn has_annotated_empty_list(
        &self,
        module: usize,
        function: Option<&ruff_python_ast::StmtFunctionDef>,
        matches_annotation: impl Fn(&Expr) -> bool,
    ) -> bool {
        use ty_python_core::definition::DefinitionKind;
        use ty_python_core::scope::NodeWithScopeRef;
        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        let scope = match function {
            Some(function) => {
                let Some(scope) = index.try_node_scope(NodeWithScopeRef::Function(function)) else {
                    return false;
                };
                scope
            }
            None => FileScopeId::global(),
        };
        let places = index.place_table(scope);
        let parsed = self.parsed(self.files[module]);
        let definitions = index.use_def_map(scope);
        places
            .symbols()
            .filter(|symbol| symbol.is_declared())
            .any(|symbol| {
                let id = places.symbol_id(symbol.name()).expect("indexed symbol");
                definitions
                    .reachable_symbol_declarations(id)
                    .filter_map(|item| item.declaration.definition())
                    .any(|def| {
                        matches!(def.kind(&self.db), DefinitionKind::AnnotatedAssignment(a)
                    if matches!(a.value(&parsed), Some(Expr::List(list)) if list.elts.is_empty())
                        && matches_annotation(a.annotation(&parsed)))
                    })
            })
    }

    /// A module operand follows the statement's normal continuation if they have
    /// the same use-def condition. Calls before the statement may themselves raise.
    pub(crate) fn unconditional_module_operand(
        &self,
        module: usize,
        statement: ruff_text_size::TextSize,
        operand: TextRange,
    ) -> bool {
        self.unconditional_operand(
            module,
            ty_python_core::FileScopeId::global(),
            Some(statement),
            operand,
        )
    }

    /// A call can complete without executing an explicit return in this function.
    pub(crate) fn call_may_return_implicitly(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
    ) -> bool {
        use ty_python_core::reachability_constraints::ScopedReachabilityConstraintId;

        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        let Some(scope) =
            index.try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(function))
        else {
            return true;
        };
        function.is_async
            || !function.decorator_list.is_empty()
            || scope.is_generator_function(index)
            || index.use_def_map(scope).end_of_scope_reachability()
                != ScopedReachabilityConstraintId::ALWAYS_FALSE
    }

    pub(crate) fn has_reachable_return(
        &self,
        module: usize,
        returns: &[&ruff_python_ast::StmtReturn],
    ) -> bool {
        let file = self.db.program_file(self.files[module]);
        let excluded = ty_python_semantic::types::ide_support::unreachable_ranges(&self.db, file);
        returns
            .iter()
            .any(|ret| !excluded.iter().any(|r| r.range.contains_range(ret.range())))
    }

    pub(crate) fn unconditional_function_operand(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
        operand: TextRange,
    ) -> bool {
        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        let Some(scope) =
            index.try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(function))
        else {
            return false;
        };
        !scope.is_generator_function(index)
            && self.unconditional_operand(module, scope, None, operand)
    }

    fn unconditional_operand(
        &self,
        module: usize,
        scope: ty_python_core::FileScopeId,
        statement: Option<ruff_text_size::TextSize>,
        operand: TextRange,
    ) -> bool {
        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        let flow = index.use_def_map(scope);
        let condition = |range: TextRange| {
            flow.range_reachability()
                .filter(|(scope, _)| scope.contains_range(range))
                .min_by_key(|(scope, _)| scope.len())
                // The use-def map omits its default, always-reachable intervals.
                .map_or(ty_python_core::reachability_constraints::ScopedReachabilityConstraintId::ALWAYS_TRUE, |(_, condition)| condition)
        };
        let required = statement.map_or(
            ty_python_core::reachability_constraints::ScopedReachabilityConstraintId::ALWAYS_TRUE,
            |start| condition(TextRange::at(start, 1.into())),
        );
        condition(operand) == required
            && !ty_python_semantic::types::ide_support::unreachable_ranges(&self.db, file)
                .iter()
                .any(|excluded| excluded.range.contains_range(operand))
    }

    /// Check both execution boundaries against the winners of the same registered program.
    fn check_registered_execution<'a>(
        &self,
        contracts: Result<(), String>,
        sources: impl Iterator<Item = (usize, crate::dynamic::RegisteredSource<'a>)> + Clone,
        has_file_body: Option<&dyn Fn(&str) -> bool>,
        entry_operand: Option<(usize, &str, &crate::frontend::phase::Diagnostic)>,
        called_body: Option<(
            &std::path::PathBuf,
            &str,
            TextRange,
            &crate::frontend::phase::Diagnostic,
        )>,
        original: Option<&crate::modules::SourceGraph>,
    ) -> Result<(), String> {
        let mut captures = execution::CapturedImports::default();
        if let Some((entry, start)) = sources.clone().find(|(_, source)| {
            source.declares_signatures
                && match source.initial {
                    None => false,
                    Some(crate::dynamic::EntryStart::Immediate) => true,
                    Some(crate::dynamic::EntryStart::After(path)) => sources
                        .clone()
                        .find(|(_, candidate)| candidate.path == path)
                        .is_some_and(|(id, _)| self.empty_initializer(id)),
                }
        }) {
            // runpy imports the parent package before executing the selected body
            // as __main__. Only an inert parent can be passed without running code.
            let (entry, start) = if !start.main_body && self.empty_initializer(entry) {
                sources
                    .clone()
                    .find(|(_, s)| s.main_body && s.declares_signatures)
                    .unwrap_or((entry, start))
            } else {
                (entry, start)
            };
            let closed = original.and_then(|graph| {
                let closed = graph.closed_initializer.as_ref()?;
                let module = &graph.modules[graph.entry];
                (entry == graph.entry
                    && start.checked_module == Some(entry)
                    && start.path == module.path
                    && start.source == module.source
                    && start.source == module.original_source())
                .then_some(closed)
            });
            let checked;
            let initializer = if let Some(closed) = closed {
                // No import was queried on this prefix, so no imported function was captured.
                closed
            } else {
                let selected: HashMap<_, _> = sources
                    .clone()
                    .map(|(id, source)| {
                        (
                            source.name,
                            initializer_calls::InitializerSource {
                                module: id,
                                path: source.path,
                                declares_signatures: source.declares_signatures,
                            },
                        )
                    })
                    .collect();
                checked = self.checked_initializer_calls(
                    (entry, start.path),
                    start.main_body,
                    |name| selected.get(name).copied(),
                    Some(&mut captures),
                );
                &checked
            };
            if let Some(error) = &initializer.error {
                if !matches!(
                    error.kind,
                    crate::frontend::phase::Kind::ExecutionOperand(_)
                ) {
                    contracts?;
                }
                return Err(error.to_string());
            }
            if let Some((path, source, range, report)) = called_body
                && let Some((id, function)) = initializer.entered_function
                && function == range
                && sources.clone().any(|(owner, winner)| {
                    owner == id
                        && winner.declares_signatures
                        && winner.path == path
                        && winner.source == source
                })
            {
                contracts?;
                return Err(report.to_string());
            }
            if let Some((owner, original, error)) = entry_operand
                && start.main_body
                && start.checked_module == Some(owner)
                && start.source == original
            {
                contracts?;
                return Err(error.to_string());
            }
        }
        contracts?;
        let ordinary = self.check_execution_with_imports(
            sources
                .clone()
                .filter(|(_, source)| source.check_execution)
                .map(|(id, source)| (id, source.path, source.has_open_exec)),
            captures,
        );
        let files = self.check_run_path_callers(sources, has_file_body);
        match (ordinary, files) {
            (Err(ordinary), Err(files)) => Err(format!("{ordinary}\n{files}")),
            (result, Ok(())) | (Ok(()), result) => result,
        }
    }
}

impl Session {
    pub(crate) fn registered_syntax<'a>(
        slot: &'a mut Option<Self>,
        root: &Path,
        roots: &[PathBuf],
    ) -> Result<&'a mut Self, String> {
        Ok(match slot {
            Some(session) => session,
            slot @ None => slot.insert(Self::with_storage(
                root,
                roots,
                salsa::Storage::default(),
                true,
            )?),
        })
    }

    /// Make the actual selected provider visible before resolving candidate callees.
    /// Sync parent directories too: an earlier level may have looked up a missing package.
    pub(crate) fn seed(&mut self, path: &Path, source: &str) -> Result<(), String> {
        let source = crate::normalize_source(source, crate::SourceKind::DecodedFile)?;
        let text = imre_ast::indentation::prepare(&source);
        let origin = SystemPath::new(path.to_str().ok_or("module paths must be valid UTF-8")?);
        let fs = self.db.system.fs();
        let missing: Vec<_> = origin
            .parent()
            .into_iter()
            .flat_map(SystemPath::ancestors)
            .take_while(|dir| !fs.is_directory(dir))
            .collect();
        fs.write_file_all(origin, text.text.as_bytes())
            .map_err(|error| error.to_string())?;
        for directory in missing.into_iter().rev() {
            File::sync_path(&mut self.db, directory);
        }
        File::sync_path(&mut self.db, origin);
        Ok(())
    }

    /// Virtual files keep immutable parser inputs outside the module search path.
    fn virtual_source(&self, key: &str, text: &str) -> File {
        let key = ruff_db::system::SystemVirtualPath::new(key);
        self.db
            .files
            .try_virtual_file(key)
            .unwrap_or_else(|| {
                self.db.system.fs().write_virtual_file(key, text.as_bytes());
                self.db.files.virtual_file(&self.db, key)
            })
            .file()
    }

    pub(crate) fn selected_file(
        &self,
        path: &Path,
    ) -> Result<(File, ParsedModuleRef, ruff_db::source::SourceText), String> {
        let origin = path.to_str().ok_or("module paths must be valid UTF-8")?;
        let file = system_path_to_file(&self.db, origin).map_err(|e| e.to_string())?;
        Ok((
            file,
            self.parsed(file),
            ruff_db::source::source_text(&self.db, file),
        ))
    }
}

/// Validate an initializer at its registration origin. Its checked file can also supply
/// the parser while the caller prepares a runtime-provided initializer.
pub(crate) fn check_native_syntax(
    root: &Path,
    scratch: &mut Option<Session>,
    checked: Option<&crate::modules::SourceGraph>,
    (name, path, source): &crate::dynamic::ModuleSource<'_>,
    keep_parser: bool,
    is_alternative: impl Fn(&Path) -> bool,
    module_errors: &mut HashMap<PathBuf, Box<crate::dynamic::ModuleSyntaxError>>,
) -> Result<Option<ParsedModuleRef>, String> {
    let input = imre_ast::indentation::prepare(source);
    let cached = checked.and_then(|graph| {
        let session = &graph.session;
        let module = if name == "__main__" {
            Some(graph.entry)
        } else {
            graph.python_file(name)
        };
        let file = if let Some(id) = module {
            (graph.modules[id].path == *path).then_some(session.files[id])?
        } else {
            session.declaration_file(path)?
        };
        session
            .file_matches_parser_input(file, &input.text)
            .then_some((session, file))
    });
    let (session, file) = if let Some(cached) = cached {
        cached
    } else {
        let session = Session::registered_syntax(scratch, root, &[])?;
        let origin = path.to_str().ok_or("module paths must be valid UTF-8")?;
        let file = session.write_file(origin, &input.text)?;
        (&*session, file)
    };
    let parsed = keep_parser.then(|| session.parsed(file));
    if let Some(prior) = module_errors.get_mut(path) {
        if prior.validate_at_origin && !is_alternative(path) {
            if let Some(error) = session.semantic_syntax_error(file) {
                return Err(Diagnostic::invalid_syntax(file, &error, &error)
                    .display(
                        &(&session.db as &dyn ruff_db::Db),
                        &DisplayDiagnosticConfig::new("imre"),
                    )
                    .to_string());
            }
            return Err(crate::frontend::phase::Diagnostic::at(
                &input.text,
                &prior.range,
                format!("SyntaxError: {}", prior.message),
            )
            .with_path(path)
            .to_string());
        }
        if !prior.parse_error
            && let Some((range, message)) = session.module_binding_error(file, prior.range.start())
        {
            **prior = crate::dynamic::ModuleSyntaxError::new(&input.text, range, message, false);
        }
        return Ok(None);
    }
    if let Some(error) = session.semantic_syntax_error(file) {
        if is_alternative(path) {
            use ruff_python_parser::semantic_errors::SemanticSyntaxErrorKind;
            let (range, message) = session
                .module_binding_error(file, TextSize::new(u32::MAX))
                .filter(|(range, _)| range.start() <= error.range.end())
                .unwrap_or_else(|| {
                    (
                        error.range,
                        match &error.kind {
                            SemanticSyntaxErrorKind::GlobalParameter(name) => {
                                format!("name '{name}' is parameter and global")
                            }
                            SemanticSyntaxErrorKind::NonlocalParameter(name) => {
                                format!("name '{name}' is parameter and nonlocal")
                            }
                            _ => error.to_string(),
                        },
                    )
                });
            module_errors.insert(
                path.clone(),
                Box::new(crate::dynamic::ModuleSyntaxError::new(
                    &input.text,
                    range,
                    message,
                    false,
                )),
            );
        } else {
            return Err(Diagnostic::invalid_syntax(file, &error, &error)
                .display(
                    &(&session.db as &dyn ruff_db::Db),
                    &DisplayDiagnosticConfig::new("imre"),
                )
                .to_string());
        }
        return Ok(None);
    }
    Ok(parsed.filter(|parsed| parsed.has_no_syntax_errors() && input.first_issue(None).is_none()))
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
                "def capture_start() -> None: ...\ndef capture_take() -> str: ...\ndef display(value: object, /) -> None: ...\ndef exception_name(exc: BaseException, /) -> str: ...\ndef execute(test: object, debug: bool, /) -> None: ...\n",
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
        // Ask the same type checker whether each execution builtin's literal name can
        // reach a namespace lookup, including names narrowed by the caller's guards.
        let execution_names = intrinsic.join(".execution-names.pyi");
        system
            .fs()
            .write_file_all(&execution_names, "(\"exec\", \"eval\")\n")
            .map_err(|error| error.to_string())?;
        let external_loader_declarations = stdlib_stubs.join("_frozen_importlib_external.pyi");
        // Based on ty_vendored 0.0.15 (licenses/typeshed.txt). Retain the cache
        // helpers' object parameters when importing updated typeshed declarations.
        system
            .fs()
            .write_file_all(
                &external_loader_declarations,
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
        let execution_names =
            system_path_to_file(&db, &execution_names).map_err(|error| error.to_string())?;
        let external_loader_declarations = system_path_to_file(&db, &external_loader_declarations)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            db,
            files: Vec::new(),
            declarations: Vec::new(),
            external_loader_declarations,
            execution_names,
            tuple_spreads: Vec::new(),
            rules: Arc::new(rules),
            root: PathBuf::from(root),
            extension_roots: extension_roots.to_vec(),
            hint_reads: Cell::new(0),
            explicit_check: false,
        })
    }

    /// Make a physical import available to ty without adding a module to the program.
    pub(crate) fn add_declaration_source(
        &mut self,
        path: &Path,
        source: &str,
    ) -> Result<(), String> {
        let path = path.to_str().ok_or("module paths must be valid UTF-8")?;
        let file = self.write_file(path, source)?;
        self.declarations.push(file);
        Ok(())
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
        let (parsed, spreads) = self.checked_parse(file, path, source, &input, kind)?;
        self.files.push(file);
        self.tuple_spreads.push(spreads);
        Ok(parsed)
    }

    fn checked_parse(
        &self,
        file: File,
        path: &str,
        source: &str,
        input: &imre_ast::indentation::Source<'_>,
        kind: crate::SourceKind,
    ) -> Result<(ParsedModuleRef, HashMap<TextSize, usize>), String> {
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
        if kind != crate::SourceKind::Command {
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
        Ok((parsed, spreads))
    }

    fn semantic_syntax_error(
        &self,
        file: File,
    ) -> Option<ruff_python_parser::semantic_errors::SemanticSyntaxError> {
        ty_python_core::semantic_index(&self.db, self.db.program_file(file))
            .semantic_syntax_errors()
            .iter()
            .find(|error| !class_cell::binds_nonlocal(&self.db, file, error))
            .cloned()
    }

    pub(crate) fn has_valid_semantic_syntax(&self, module: usize) -> bool {
        self.semantic_syntax_error(self.files[module]).is_none()
    }

    /// Binding errors can precede a misplaced future import or an unreachable pattern.
    /// Ask the symbol table which declaration conflicts with an earlier use.
    fn module_binding_error(&self, file: File, before: TextSize) -> Option<(TextRange, String)> {
        use ruff_python_ast::find_node::covering_node;
        use ruff_python_parser::semantic_errors::SemanticSyntaxErrorKind;

        let parsed = self.parsed(file);
        let root = parsed.syntax().into();
        let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
        index
            .semantic_syntax_errors()
            .iter()
            .filter_map(|error| {
                let (name, start, directive) = match &error.kind {
                    SemanticSyntaxErrorKind::LoadBeforeGlobalDeclaration { name, start } => {
                        (name, *start, "global")
                    }
                    SemanticSyntaxErrorKind::LoadBeforeNonlocalDeclaration { name, start } => {
                        (name, *start, "nonlocal")
                    }
                    SemanticSyntaxErrorKind::AwaitOutsideAsyncFunction(
                        ruff_python_parser::semantic_errors::AwaitOutsideAsyncFunctionKind::Await,
                    ) if error.range.start() < before => {
                        let enclosed = covering_node(root, error.range)
                            .find_first(|node| {
                                matches!(node, ruff_python_ast::AnyNodeRef::StmtFunctionDef(_))
                            })
                            .is_ok();
                        return Some((
                            error.range,
                            if enclosed {
                                "'await' outside async function"
                            } else {
                                "'await' outside function"
                            }
                            .to_owned(),
                        ));
                    }
                    SemanticSyntaxErrorKind::GlobalParameter(name)
                    | SemanticSyntaxErrorKind::NonlocalParameter(name)
                        if error.range.start() < before =>
                    {
                        let statement = covering_node(root, error.range)
                            .find_first(|node| {
                                matches!(
                                    node,
                                    ruff_python_ast::AnyNodeRef::StmtGlobal(_)
                                        | ruff_python_ast::AnyNodeRef::StmtNonlocal(_)
                                )
                            })
                            .ok()?
                            .node();
                        let directive =
                            if matches!(statement, ruff_python_ast::AnyNodeRef::StmtGlobal(_)) {
                                "global"
                            } else {
                                "nonlocal"
                            };
                        return Some((
                            statement.range(),
                            format!("name '{name}' is parameter and {directive}"),
                        ));
                    }
                    SemanticSyntaxErrorKind::YieldOutsideFunction(kind)
                        if error.range.start() < before =>
                    {
                        return Some((error.range, format!("'{kind}' outside function")));
                    }
                    _ => return None,
                };
                if start >= before {
                    return None;
                }
                let declaration = covering_node(root, TextRange::empty(start))
                    .find_first(|node| {
                        matches!(
                            node,
                            ruff_python_ast::AnyNodeRef::StmtGlobal(_)
                                | ruff_python_ast::AnyNodeRef::StmtNonlocal(_)
                        )
                    })
                    .ok()?
                    .node();
                let names = match declaration {
                    ruff_python_ast::AnyNodeRef::StmtGlobal(stmt) => &stmt.names,
                    ruff_python_ast::AnyNodeRef::StmtNonlocal(stmt) => &stmt.names,
                    _ => return None,
                };
                let identifier = names.iter().find(|n| n.start() == start)?;
                let scope = index.expression_scope_id(identifier);
                let place = index.place_table(scope).symbol_id(name)?.into();
                let bound = index
                    .use_def_map(scope)
                    .definitions_with_usage()
                    .any(|(_, def, _)| {
                        def.place(&self.db) == place
                            && def.focus_range(&self.db, &parsed).start() < start
                    });
                let use_kind = if bound {
                    "assigned to before"
                } else {
                    "used prior to"
                };
                Some((
                    declaration.range(),
                    format!("name '{name}' is {use_kind} {directive} declaration"),
                ))
            })
            .min_by_key(|(range, _)| range.start())
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

    fn write_file(&mut self, path: &str, source: &str) -> Result<File, String> {
        self.db
            .system
            .fs()
            .write_file_all(path, source)
            .map_err(|error| error.to_string())?;
        // Earlier package resolution may have cached a directory before this child existed.
        File::sync_path(&mut self.db, path.as_ref());
        system_path_to_file(&self.db, path).map_err(|error| error.to_string())
    }

    pub(crate) fn matches_parser_input(&self, module: usize, text: &str) -> bool {
        self.file_matches_parser_input(self.files[module], text)
    }

    /// Match the parser input of a selected physical file, including a declaration-only module.
    fn declaration_file(&self, path: &Path) -> Option<File> {
        self.declarations.iter().copied().find(|file| {
            file.path(&self.db)
                .as_system_path()
                .is_some_and(|origin| origin.as_std_path() == path)
        })
    }

    /// Borrow the declaration's indexed syntax while its physical parser input still matches.
    pub(crate) fn declaration_parser(&self, path: &Path, source: &str) -> Option<ParsedModuleRef> {
        let file = self.declaration_file(path)?;
        let text = crate::normalize_source(source, crate::SourceKind::DiskFile).ok()?;
        let input = imre_ast::indentation::prepare(&text);
        self.file_matches_parser_input(file, &input.text)
            .then(|| self.parsed(file))
            .filter(|parsed| parsed.has_no_syntax_errors())
    }

    fn file_matches_parser_input(&self, file: File, text: &str) -> bool {
        ruff_db::source::source_text(&self.db, file).as_str() == text
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

    /// The physical line and its starting byte offset in the checked source.
    pub(crate) fn source_line_start(
        &self,
        module: usize,
        source: &str,
        offset: TextSize,
    ) -> (u32, TextSize) {
        let index = ruff_db::source::line_index(&self.db, self.files[module]);
        let line = index.line_index(offset);
        (line.get() as u32, index.line_start(line, source))
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

    /// Use the existing parsed files for a check, then restore the caller's inference rules.
    pub(crate) fn with_diagnostic_rules<T>(&mut self, check: impl FnOnce(&mut Self) -> T) -> T {
        let input = DiagnosticRules::get(&self.db);
        let previous = input.rules(&self.db).clone();
        let result = check(self);
        input.set_rules(&mut self.db).to(previous);
        result
    }

    /// Start diagnostics with independent rules and Salsa storage. Every file has already
    /// passed the loader's preflight in this snapshot.
    pub(crate) fn diagnostic_copy(&self) -> Result<Self, String> {
        self.copy_for_diagnostics(None, None)
    }

    /// Check a prospective function against the source typing rules before an
    /// unresolved signature can acquire a new registered entry.
    pub(crate) fn registered_body_checks(&self) -> Result<RegisteredBodyChecks, String> {
        let mut db = Database {
            storage: salsa::Storage::default(),
            files: Files::default(),
            ..self.db.clone()
        };
        db.settings.search_paths.try_register_static_roots(&db);
        DiagnosticRules::new(&db, Self::registered_body_rules());
        // Source rewrites belong to the owning Salsa database. Imports without
        // an override already read their selected bytes from the shared filesystem.
        for &original in self.files.iter().chain(&self.declarations) {
            if let Some(text) = original.source_text_override(&self.db) {
                let file = system_path_to_file(&db, original.path(&self.db).as_str())
                    .map_err(|error| error.to_string())?;
                file.set_source_text_override(&mut db)
                    .to(Some(text.clone()));
            }
        }
        Ok(RegisteredBodyChecks(db))
    }

    fn registered_body_rules() -> Arc<RuleSelection> {
        Arc::new(RuleSelection::from_registry(default_lint_registry()))
    }

    pub(crate) fn registered_source_error_ranges(
        &mut self,
        module: usize,
    ) -> Vec<Option<TextRange>> {
        self.with_diagnostic_rules(|session| {
            DiagnosticRules::get(&session.db)
                .set_rules(&mut session.db)
                .to(Self::registered_body_rules());
            session.db.source_error_ranges(session.files[module])
        })
    }

    pub(crate) fn copy_for_diagnostics(
        &self,
        script_entry: Option<File>,
        registrations: Option<&crate::dynamic::RegisteredSources<'_>>,
    ) -> Result<Self, String> {
        let mut copy = Self::with_storage(
            &self.root,
            registrations.map_or(&self.extension_roots, |sources| sources.roots),
            salsa::Storage::default(),
            script_entry.is_some(),
        )?;
        copy.explicit_check = self.explicit_check;
        if let Some(registrations) = registrations {
            // Declarations can be re-exported from another selected Python module.
            for winner in registrations
                .winners
                .iter()
                .filter(|s| s.checked_module.is_none())
            {
                copy.seed(winner.path, winner.source)?;
            }
        }
        for (id, &file) in self.files.iter().enumerate() {
            if let Some(&(path, body)) = registrations.and_then(|r| r.provider_sources.get(&id)) {
                // Keep the module index used by checked callers, and the original
                // filename and search root used by the Python import.
                let path = path.to_str().ok_or("module paths must be valid UTF-8")?;
                let input = imre_ast::indentation::prepare(body);
                let file = copy.write_file(path, &input.text)?;
                copy.files.push(file);
                continue;
            }
            let path = file
                .path(&self.db)
                .as_system_path()
                .expect("first-party source");
            let source = ruff_db::source::source_text(&self.db, file);
            let file = if Some(file) == script_entry {
                let entry = path.parent().unwrap().join("<__main__>.py");
                copy.write_file(entry.as_str(), source.as_str())?
            } else {
                copy.write_file(path.as_str(), source.as_str())?
            };
            copy.files.push(file);
        }
        for &file in &self.declarations {
            let path = file
                .path(&self.db)
                .as_system_path()
                .expect("physical declaration");
            let source = ruff_db::source::source_text(&self.db, file);
            copy.add_declaration_source(path.as_std_path(), source.as_str())?;
        }
        Ok(copy)
    }

    pub(crate) fn is_generator_definition(
        &self,
        module: usize,
        function: &ruff_python_ast::StmtFunctionDef,
    ) -> bool {
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        index
            .try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(function))
            .is_some_and(|scope| scope.is_generator_function(index))
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

    pub(crate) fn bound_module_names(&self, module: usize) -> Vec<String> {
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        index
            .place_table(ty_python_core::scope::FileScopeId::global())
            .symbols()
            .filter(|symbol| symbol.is_bound())
            .map(|symbol| symbol.name().to_string())
            .collect()
    }

    /// Every write reaching this symbol must be a plain top-level function declaration.
    pub(crate) fn module_function_declarations(&self, module: usize, name: &str) -> bool {
        let file = self.files[module];
        let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
        let scope = FileScopeId::global();
        let Some(symbol) = index.place_table(scope).symbol_id(name) else {
            return false;
        };
        let parsed = self.parsed(file);
        index
            .use_def_map(scope)
            .reachable_symbol_bindings(symbol)
            .all(|binding| {
                binding.binding.is_undefined_or(|def| {
                    let ty_python_core::definition::DefinitionKind::Function(def) =
                        def.kind(&self.db)
                    else {
                        return false;
                    };
                    let f = def.node(&parsed);
                    crate::frontend::fixed_frame_entry(f)
                        && f.parameters.kwonlyargs.is_empty()
                        && f.parameters
                            .iter_non_variadic_params()
                            .all(|p| p.default.is_none())
                        && parsed
                            .syntax()
                            .body
                            .binary_search_by_key(&f.start(), Ranged::start)
                            .is_ok()
                })
            })
    }

    /// Only an assignment through an explicit global can replace the module's binding.
    pub(crate) fn nested_global_writes(
        &self,
        module: usize,
        tokens: &Tokens,
    ) -> std::collections::HashSet<String> {
        if !tokens.iter().any(|token| token.kind() == TokenKind::Global) {
            return Default::default();
        }
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        index
            .scope_ids()
            .map(|scope| scope.file_scope_id(&self.db))
            .filter(|scope| !scope.is_global())
            .flat_map(|scope| index.place_table(scope).symbols())
            .filter(|symbol| symbol.is_global() && symbol.is_bound())
            .map(|symbol| symbol.name().to_string())
            .collect()
    }

    pub(crate) fn explicitly_global(&self, module: usize, target: &Expr) -> bool {
        let Expr::Name(name) = target else {
            return false;
        };
        let index =
            ty_python_core::semantic_index(&self.db, self.db.program_file(self.files[module]));
        index.try_expression_scope_id(target).is_some_and(|scope| {
            let table = index.place_table(scope);
            table
                .symbol_id(name.id.as_str())
                .is_some_and(|id| table.symbol(id).is_global())
        })
    }

    fn sole_binding(
        &self,
        module: usize,
        expression: &Expr,
    ) -> Option<ty_python_core::definition::Definition<'_>> {
        self.hint_reads.set(self.hint_reads.get() + 1);
        self.reaching_binding(module, expression)
    }

    fn reaching_binding(
        &self,
        module: usize,
        expression: &Expr,
    ) -> Option<ty_python_core::definition::Definition<'_>> {
        self.file_binding(self.files[module], expression)
    }

    fn file_binding(
        &self,
        file: File,
        expression: &Expr,
    ) -> Option<ty_python_core::definition::Definition<'_>> {
        shipped_exports::binding_at_use(&self.db, file, expression)
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

    /// A sole local import writes a Python binding without invalidating another checked layout.
    pub(crate) fn sole_local_import(
        &self,
        module: usize,
        statement: &ruff_python_ast::Stmt,
    ) -> bool {
        let aliases = match statement {
            ruff_python_ast::Stmt::Import(import) => &import.names,
            ruff_python_ast::Stmt::ImportFrom(import) => &import.names,
            _ => return false,
        };
        let file = self.db.program_file(self.files[module]);
        let index = ty_python_core::semantic_index(&self.db, file);
        aliases.iter().all(|alias| {
            let Some(binding) = index.try_definition(alias) else {
                return false;
            };
            let scope = binding.file_scope(&self.db);
            let place = binding.place(&self.db);
            place
                .as_symbol()
                .is_some_and(|id| index.place_table(scope).symbol(id).is_local())
                && index
                    .use_def_map(scope)
                    .definitions_with_usage()
                    .all(|(_, other, _)| other == binding || other.place(&self.db) != place)
        })
    }

    /// Resolve a module alias at the expression that reads it.
    pub(crate) fn module_name_at(&self, module: usize, expression: &Expr) -> Option<&str> {
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        let ty_python_semantic::types::Type::ModuleLiteral(owner) =
            expression.inferred_type(&model)?
        else {
            return None;
        };
        Some(owner.module(&self.db).name(&self.db).as_str())
    }

    pub(crate) fn is_stdlib_module(&self, module: usize, expression: &Expr, name: &str) -> bool {
        self.hint_reads.set(self.hint_reads.get() + 1);
        self.module_name_at(module, expression) == Some(name)
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

    /// A missing descriptor on an exact builtin literal raises at the Python lookup site.
    pub(crate) fn missing_literal_attribute(
        &self,
        module: usize,
        attribute: &ruff_python_ast::ExprAttribute,
    ) -> bool {
        use ty_python_semantic::types::{
            Type as Ty, ide_support::static_member_type_for_attribute,
        };
        // Some protocol slots have no declaration in typeshed, even on exact builtins.
        if ruff_python_ast::helpers::is_dunder(attribute.attr.as_str()) {
            return false;
        }
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(attribute.value.inferred_type(&model), Some(Ty::LiteralValue(value)) if !value.is_enum())
            && static_member_type_for_attribute(&model, attribute).is_none()
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

    /// Resolve an indexed class reference using the annotation's declaring scope.
    pub(crate) fn annotation_class(
        &self,
        module: usize,
        annotation: &Expr,
    ) -> Option<AnnotationClass> {
        use ruff_python_ast::HasNodeIndex;
        use ty_python_semantic::types::{Type, TypeDefinition};

        // Recursively parsed string contents have no node in the indexed module.
        annotation.node_index().load().as_u32()?;
        match annotation {
            Expr::StringLiteral(literal)
                if ruff_python_stdlib::identifiers::is_identifier(literal.value.to_str()) => {}
            Expr::Name(_) | Expr::Attribute(_) if !self.declarations.is_empty() => {}
            _ => return None,
        }
        self.hint_reads.set(self.hint_reads.get() + 1);
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        let ty @ Type::NominalInstance(_) = annotation.inferred_type(&model)? else {
            return None;
        };
        let class @ TypeDefinition::StaticClass(_) =
            ty.definition(&self.db, &model.program_environment())?
        else {
            return None;
        };
        let location = class.focus_range(&self.db)?;
        let file = location.file();
        if let Some(owner) = self.files.iter().position(|f| *f == file) {
            return Some(AnnotationClass::Linked(owner, location.range()));
        }
        if !self.declarations.contains(&file) || !self.parsed(file).has_no_syntax_errors() {
            return None;
        }
        let path = file.path(&self.db).as_system_path()?;
        let source = ruff_db::source::source_text(&self.db, file);
        let position = ruff_db::source::line_index(&self.db, file)
            .line_column(location.range().start(), source.as_str());
        Some(AnnotationClass::Declaration(format!(
            "{path}:{}:{}",
            position.line, position.column
        )))
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

    /// An unknown inferred type can also come from an invalid operation. Only a dynamic
    /// parameter that still supplies this name proves the checked frame's late-bound operand.
    pub(crate) fn is_dynamic_receiver(&self, module: usize, expression: &Expr) -> bool {
        use ty_python_core::definition::DefinitionKind;
        use ty_python_semantic::types::ide_support::{
            ImportAliasResolution, ResolvedDefinition, definitions_for_name,
        };
        use ty_python_semantic::types::{DynamicType, Type as Ty};

        let Some(name) = expression.as_name_expr() else {
            return false;
        };
        self.hint_reads.set(self.hint_reads.get() + 1);
        let file = self.db.program_file(self.files[module]);
        if ty_python_core::semantic_index(&self.db, file)
            .try_expression_scope_id(expression)
            .is_none()
        {
            return false;
        }
        let model = SemanticModel::new(&self.db, file);
        if !matches!(
            expression.inferred_type(&model),
            Some(Ty::Dynamic(DynamicType::Any | DynamicType::Unknown))
        ) {
            return false;
        }
        let definitions = definitions_for_name(
            &model,
            name.id.as_str(),
            name.into(),
            ImportAliasResolution::PreserveAliases,
        );
        matches!(
            definitions.as_slice(),
            [ResolvedDefinition::Definition(definition)]
                if matches!(definition.kind(&self.db), DefinitionKind::Parameter(_))
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

    fn inferred_layout_type(
        &self,
        module: usize,
        value: &Expr,
    ) -> Option<ty_python_semantic::types::Type<'_>> {
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
        match inferred {
            Ok(ty) => ty,
            Err(payload) if payload.is::<LayoutHintLimit>() => None,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub(crate) fn operand_types(&self, module: usize) -> OperandTypes<'_> {
        OperandTypes {
            session: self,
            module,
        }
    }

    /// Get a scalar layout hint for an operand in its own lexical scope.
    pub(crate) fn scalar_type(&self, module: usize, value: &Expr) -> Option<crate::ir::Type> {
        self.inferred_scalar_layout(module, value, ScalarMatch::Assignable)
    }

    fn inferred_scalar_layout(
        &self,
        module: usize,
        value: &Expr,
        rule: ScalarMatch,
    ) -> Option<crate::ir::Type> {
        use ty_python_semantic::types::ProgramEnvironment;
        let ty = self.inferred_layout_type(module, value)?;
        let env = ProgramEnvironment::from_file(self.db.program_file(self.files[module]));
        self.scalar_layout(&env, ty, rule)
    }

    fn scalar_layout<'db>(
        &'db self,
        env: &ty_python_semantic::types::ProgramEnvironment<'db>,
        ty: ty_python_semantic::types::Type<'db>,
        rule: ScalarMatch,
    ) -> Option<crate::ir::Type> {
        use crate::ir::Type;
        use ty_python_semantic::types::{KnownClass, Type as Ty};

        if matches!(rule, ScalarMatch::Exact) && ty.is_none(&self.db) {
            return Some(Type::None);
        }
        if !matches!(
            ty,
            Ty::LiteralValue(_) | Ty::NominalInstance(_) | Ty::Union(_)
        ) {
            return None;
        }
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
            let mut instance = class.to_instance(&self.db, env);
            if matches!(rule, ScalarMatch::Exact) {
                return ty == instance;
            }
            if matches!(class, KnownClass::Float) {
                instance = ty_python_semantic::types::UnionType::from_two_elements(
                    &self.db,
                    env,
                    instance,
                    KnownClass::Int.to_instance(&self.db, env),
                );
            }
            ty.is_assignable_to(&self.db, env, instance)
        })
        .map(|(_, ty)| ty)
    }

    /// A fixed field needs every possible result to fit its storage. Adding a proven
    /// string subtype to `str` leaves `str`, including a truth-narrowed intersection.
    pub(crate) fn has_string_layout(&self, module: usize, value: &Expr) -> bool {
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty, UnionType};
        let Some(ty) = self.inferred_layout_type(module, value) else {
            return false;
        };
        if matches!(ty, Ty::Never | Ty::Divergent(_)) {
            return false;
        }
        let env = ProgramEnvironment::from_file(self.db.program_file(self.files[module]));
        let text = KnownClass::Str.to_instance(&self.db, &env);
        ty == text || UnionType::from_two_elements(&self.db, &env, ty, text) == text
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
        roots: impl Iterator<Item = usize> + Clone,
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
        self.errors(roots.clone(), skipped, overloads, checks, checked_modules)?;

        let entry = match calls {
            FallbackCalls::Entry(id) => Some(id),
            _ => None,
        };
        let probe_modules = checked_modules.filter(|_| {
            matches!(
                checks,
                FileChecks::FallbackReturns | FileChecks::NamedFunctionCalls
            )
        });
        let probes = roots.filter_map(|id| {
            let guards = &probe_modules?[id].optional_imports;
            (Some(id) != entry && !guards.is_empty()).then_some((id, Some(guards)))
        });
        // An entry guard owns calls throughout its entry. A deferred import owns
        // only calls inside its try statement, whose operands fixed lowering skipped.
        let mut owners = entry
            .map(|id| (id, None))
            .into_iter()
            .chain(probes)
            .peekable();
        if checks == FileChecks::FallbackReturns && owners.peek().is_some() {
            self.set_fallback_rules(true, false, checked_modules.is_some());
        }
        let mut errors = Vec::new();
        for (id, guards) in owners {
            let checked = CheckedFile::new(
                &self.db,
                self.files[id],
                skipped.map_or(&[][..], |ranges| ranges[id].as_slice()),
                overloads[id].as_slice(),
                FileChecks::FallbackCalls,
                checked_modules.is_some_and(|modules| !modules[id].bundled),
                self.explicit_check,
                None,
            );
            for error in file_errors(&self.db, checked) {
                if guards.is_none_or(|guards| {
                    CALL_ARGUMENT_RULES.contains(&error.rule.as_str())
                        && error.range.is_some_and(|range| {
                            guards.keys().any(|guard| guard.contains_range(range))
                        })
                }) {
                    errors.push(error.rendered.as_str());
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
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

    /// Resume layout inference on this same set of parsed files. A cached hint
    /// database stays quiet; only a cold, completed scopes check changed its rules.
    pub(crate) fn resume_layout_queries(&mut self) {
        let rules = DiagnosticRules::get(&self.db);
        if rules.rules(&self.db).enabled().next().is_some() {
            rules.set_rules(&mut self.db).to(Arc::default());
        }
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
        self.errors(
            0..self.files.len(),
            skipped,
            overloads,
            FileChecks::Full,
            None,
        )
    }

    /// Check explicit source signatures before choosing a representation.
    pub(crate) fn check_closed_program(
        &mut self,
        modules: &[impl ClosedSource],
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        native: Option<&crate::frontend::phase::Diagnostic>,
    ) -> Result<(), String> {
        self.set_closed_program_rules();
        self.closed_program_errors(modules, skipped, overloads, native)
    }

    pub(crate) fn set_closed_program_rules(&mut self) {
        // Native lowering and the ordinary fallback checker own the rest of the type
        // contract. Check the same declared signatures when lowering stops at a
        // representation choice, or registration selects a different source graph.
        let mut rules = RuleSelection::default();
        for name in [
            "invalid-argument-type",
            "invalid-return-type",
            "invalid-assignment",
        ] {
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

    /// ty omits diagnostics in unreachable statements. A direct call can still
    /// name a single declared function there, even when its callee type is Never.
    fn dormant_call_errors(
        &self,
        module: usize,
        path: &Path,
        declarations: DeclaredSignatures<'_>,
        errors: &mut Vec<String>,
    ) {
        use ty_python_semantic::types::ide_support::{
            ImportAliasResolution, ResolvedDefinition, definitions_for_name, unreachable_ranges,
        };
        let db = &self.db;
        let file = self.files[module];
        let source = db.program_file(file);
        let ranges = unreachable_ranges(db, source);
        if ranges.is_empty() {
            return;
        }
        let parsed = self.parsed(file);
        let model = SemanticModel::new(db, source);
        let mut check = |call: &ruff_python_ast::ExprCall| -> Option<()> {
            let name = call.func.as_name_expr()?;
            if !ranges.iter().any(|r| r.range.contains_range(call.range()))
                || call.func.inferred_type(&model)? != ty_python_semantic::types::Type::Never
            {
                return None;
            }
            let bindings = definitions_for_name(
                &model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            );
            let [ResolvedDefinition::Definition(definition)] = bindings.as_slice() else {
                return None;
            };
            let ty_python_core::definition::DefinitionKind::Function(function) =
                definition.kind(db)
            else {
                return None;
            };
            let owner = definition.file(db);
            let body = self.parsed(owner);
            let function = function.node(&body);
            // Decorators can replace the callable and its Python binding convention.
            if !function.decorator_list.is_empty() || function.type_params.is_some() {
                return None;
            }
            let owner_model = SemanticModel::new(db, db.program_file(owner));
            for (index, declared) in function.parameters.iter_non_variadic_params().enumerate() {
                let param = &declared.parameter;
                if declarations
                    .parameters(db)
                    .contains(&(owner, declared.range()))
                    && let Some(value) = crate::frontend::known_declared_argument(
                        &call.arguments,
                        &function.parameters,
                        index,
                        param.name.as_str(),
                    )
                    && let Some(expected) = param.inferred_type(&owner_model)
                    && let Some(actual) = value.inferred_type(&model)
                    && let Some(error) = self.scalar_argument_error(&model, expected, value, actual)
                {
                    errors.push(error.with_path(path).to_string());
                }
            }
            Some(())
        };
        ruff_python_ast::helpers::any_over_body(&parsed.syntax().body, |expr| {
            if let Expr::Call(call) = expr {
                check(call);
            }
            false
        });
    }

    fn closed_program_errors(
        &self,
        modules: &[impl ClosedSource],
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        native: Option<&crate::frontend::phase::Diagnostic>,
    ) -> Result<(), String> {
        use ty_python_core::definition::DefinitionKind;

        assert_eq!(modules.len(), self.files.len());
        if !modules.iter().any(|module| module.project_path().is_some()) {
            return Ok(());
        }
        let mut parameters = Vec::new();
        let mut result_scopes = Vec::new();
        let mut class_files = Vec::new();
        for (module, &file) in modules.iter().zip(&self.files) {
            if !module.supplies_declarations() {
                continue;
            }
            let source = self.db.program_file(file);
            let index = ty_python_core::semantic_index(&self.db, source);
            let parsed = ruff_db::parsed::parsed_module(&self.db, source.python_file(&self.db))
                .load(&self.db);
            for scope in index.scope_ids() {
                use ty_python_core::scope::NodeWithScopeKind;
                let owner = scope.node(&self.db);
                if matches!(owner, NodeWithScopeKind::Class(_)) && class_files.last() != Some(&file)
                {
                    class_files.push(file);
                }
                if let NodeWithScopeKind::Function(function) = owner {
                    let function = function.node(&parsed);
                    // A generator's annotation describes the iterator, including a separate
                    // completion value. Leave that contract with the generator checker.
                    if module.project_path().is_some()
                        && function.returns.is_some()
                        && !scope.file_scope_id(&self.db).is_generator_function(index)
                    {
                        result_scopes.push((file, scope.file_scope_id(&self.db)));
                    }
                    parameters.extend(
                        function
                            .parameters
                            .iter()
                            .filter(|p| p.annotation().is_some())
                            .map(|p| (file, p.range())),
                    );
                }
            }
        }
        let has_signatures =
            !parameters.is_empty() || !result_scopes.is_empty() || !class_files.is_empty();
        result_scopes.sort_unstable();
        class_files.sort_unstable();
        let declarations =
            DeclaredSignatures::new(&self.db, parameters, result_scopes, class_files);
        let mut errors = Vec::new();
        for (index, (module, &file)) in modules.iter().zip(&self.files).enumerate() {
            let Some(path) = module.project_path() else {
                continue;
            };
            if path.extension().is_some_and(|ext| ext == "pyi") {
                continue;
            }
            if !has_signatures {
                let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
                if !index.scope_ids().any(|scope| {
                    index
                        .use_def_map(scope.file_scope_id(&self.db))
                        .definitions_with_usage()
                        .any(|(_, def, _)| {
                            matches!(def.kind(&self.db), DefinitionKind::AnnotatedAssignment(_))
                        })
                }) {
                    continue;
                }
            }
            if !declarations.parameters(&self.db).is_empty() {
                self.dormant_call_errors(index, path, declarations, &mut errors);
            }
            let checked = CheckedFile::new(
                &self.db,
                file,
                skipped.map_or(&[][..], |ranges| ranges[index].as_slice()),
                overloads.get(index).map_or(&[][..], Vec::as_slice),
                FileChecks::DeclaredSignatures,
                true,
                self.explicit_check,
                Some(declarations),
            );
            for error in file_errors(&self.db, checked) {
                let original = native.and_then(|report| {
                    let (owner, annotation) = error.declaration?;
                    let owner = self.files.iter().position(|&file| file == owner)?;
                    let (declaration, _) = modules[owner].checked_source(owner)?;
                    let (_, path) = module.checked_source(index)?;
                    report.matching_declared_operand(path, error.range?, declaration, annotation)
                });
                errors.push(original.map_or_else(|| error.rendered.clone(), ToString::to_string));
            }
        }
        if errors.is_empty() {
            native.map_or(Ok(()), |report| {
                let selected = modules
                    .iter()
                    .enumerate()
                    .filter_map(|(id, module)| module.checked_source(id));
                report.check_declared_operand(selected)
            })
        } else {
            Err(errors.join("\n"))
        }
    }

    fn errors(
        &self,
        roots: impl Iterator<Item = usize>,
        skipped: Option<&[Vec<TextRange>]>,
        overloads: &[Vec<CheckedDiagnostic>],
        checks: FileChecks,
        checked_modules: Option<&[crate::modules::SourceModule]>,
    ) -> Result<(), String> {
        let errors: Vec<_> = roots
            .flat_map(|index| {
                let file = self.files[index];
                let checked = CheckedFile::new(
                    &self.db,
                    file,
                    skipped.map_or(&[][..], |ranges| ranges[index].as_slice()),
                    overloads.get(index).map_or(&[][..], Vec::as_slice),
                    checks,
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
    first_party: bool,
    #[returns(copy)]
    explicit_check: bool,
    #[returns(copy)]
    declarations: Option<DeclaredSignatures<'db>>,
}

/// Source and SDK parameters, and first-party function scopes with declared results.
#[salsa::interned]
struct DeclaredSignatures<'db> {
    parameters: Vec<(File, TextRange)>,
    result_scopes: Vec<(File, FileScopeId)>,
    class_files: Vec<File>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum FileChecks {
    Full,
    DeclaredSignatures,
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
    declaration: Option<(File, TextRange)>,
    rendered: String,
}

/// A per-file result depends on the ty queries it actually read, including any other source
/// consulted to render a diagnostic. Every revision still requests the result for every file.
#[salsa::tracked(returns(ref))]
fn file_errors<'db>(db: &'db dyn Db, checked: CheckedFile<'db>) -> Vec<CheckedError> {
    let file = checked.file(db);
    let checks = checked.checks(db);
    let full = matches!(checks, FileChecks::Full | FileChecks::DeclaredSignatures);
    let parse =
        || ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db);
    let fallback = (!full).then(parse);
    if fallback.as_ref().is_some_and(fallback_data_module) {
        return Vec::new();
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
                && fallback.as_ref().is_none_or(|parsed| {
                    diagnostic.id().as_str() != "invalid-assignment"
                        || checked.first_party(db)
                            && diagnostic
                                .primary_span()
                                .and_then(|span| span.range())
                                .and_then(|range| scalar_initializer(db, file, parsed, range))
                                .is_some()
                })
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
                            || (checks != FileChecks::DeclaredSignatures
                                && diagnostic.id().is_lint_named("invalid-return-type")
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
        .filter_map(|diagnostic| {
            let range = primary_range(diagnostic, file);
            let declaration = match checked.declarations(db) {
                Some(sources) if diagnostic.id().is_lint_named("invalid-argument-type") => {
                    let parameter = declared_parameter(diagnostic, sources.parameters(db));
                    if parameter.is_none()
                        && !has_declared_nominal_target(
                            db,
                            file,
                            diagnostic,
                            sources.class_files(db),
                        )
                    {
                        return None;
                    }
                    parameter
                }
                Some(sources) if diagnostic.id().is_lint_named("invalid-assignment") => {
                    let scalar =
                        range.and_then(|range| scalar_initializer(db, file, &parse(), range));
                    if scalar.is_none()
                        && !has_declared_nominal_target(
                            db,
                            file,
                            diagnostic,
                            sources.class_files(db),
                        )
                    {
                        return None;
                    }
                    scalar.map(|annotation| (file, annotation))
                }
                Some(sources) if diagnostic.id().is_lint_named("invalid-return-type") => {
                    let scope = return_scope(db, file, range?)?;
                    sources
                        .result_scopes(db)
                        .binary_search(&(file, scope))
                        .ok()?;
                    None
                }
                Some(_) => return None,
                None => None,
            };
            Some(CheckedError {
                rule: diagnostic.id(),
                range,
                declaration,
                rendered: diagnostic
                    .display(
                        &(db as &dyn ruff_db::Db),
                        &DisplayDiagnosticConfig::new("imre"),
                    )
                    .to_string(),
            })
        })
        .collect()
}

/// Literal initializers of scalar names are independent of the operation that required boxing.
fn scalar_initializer(
    db: &dyn Db,
    file: File,
    parsed: &ParsedModuleRef,
    range: TextRange,
) -> Option<TextRange> {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};

    let nodes = covering_node(parsed.syntax().into(), range);
    let mut ancestors = nodes.ancestors();
    let assignment = ancestors.find_map(|node| match node {
        AnyNodeRef::StmtAnnAssign(assignment) => Some(assignment),
        _ => None,
    })?;
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
        return None;
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
        return None;
    }
    let model = SemanticModel::new(db, db.program_file(file));
    let declared = annotation_type(db, &model, &assignment.annotation)?;
    concrete_scalar(db, &model.program_environment(), declared)
        .then(|| assignment.annotation.range())
}

fn annotation_type<'db>(
    db: &'db dyn Db,
    model: &SemanticModel<'db>,
    annotation: &Expr,
) -> Option<ty_python_semantic::types::Type<'db>> {
    use ty_python_semantic::types::Type;

    let mut declared = annotation.inferred_type(model)?;
    let env = model.program_environment();
    let mut seen = std::collections::HashSet::new();
    while let Type::TypeAlias(alias) = declared {
        let (definition, value) = with_alias_value(db, &env, alias, |definition, _, _, value| {
            (definition, value)
        })?;
        if !seen.insert(definition) {
            return None;
        }
        declared = value;
    }
    Some(declared)
}

fn with_alias_value<'db, T>(
    db: &'db dyn Db,
    env: &ty_python_semantic::types::ProgramEnvironment<'db>,
    alias: ty_python_semantic::types::TypeAliasType<'db>,
    visit: impl FnOnce(
        ty_python_core::definition::Definition<'db>,
        &SemanticModel<'db>,
        &Expr,
        ty_python_semantic::types::Type<'db>,
    ) -> T,
) -> Option<T> {
    use ty_python_core::definition::DefinitionKind;
    use ty_python_semantic::types::{KnownInstanceType, Type, TypeDefinition};

    let object = Type::KnownInstance(KnownInstanceType::TypeAliasType(alias));
    let TypeDefinition::TypeAlias(definition) = object.definition(db, env)? else {
        return None;
    };
    let DefinitionKind::TypeAlias(node) = definition.kind(db) else {
        return None;
    };
    let parsed = ruff_db::parsed::parsed_module(db, definition.python_file(db)).load(db);
    let model = SemanticModel::new(db, definition.program_file(db));
    let expression = node.node(&parsed).value.as_ref();
    let value = expression.inferred_type(&model)?;
    // A malformed cycle may recover a scalar during expansion. Keep its declared RHS authoritative.
    if !matches!(value, Type::TypeAlias(_)) && value != alias.value_type(db) {
        return None;
    }
    Some(visit(definition, &model, expression, value))
}

fn source_class_annotation(
    db: &dyn Db,
    file: File,
    range: TextRange,
    class_files: &[File],
) -> bool {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};

    let program = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, program.python_file(db)).load(db);
    let nodes = covering_node(parsed.syntax().into(), range);
    let Some(annotation) = nodes.ancestors().find_map(|node| match node {
        AnyNodeRef::Parameter(parameter) => parameter.annotation.as_deref(),
        AnyNodeRef::StmtAnnAssign(assignment) => Some(assignment.annotation.as_ref()),
        _ => None,
    }) else {
        return false;
    };
    if annotation.range() != range {
        return false;
    }
    contains_source_class(
        db,
        &SemanticModel::new(db, program),
        annotation,
        class_files,
        &mut std::collections::HashSet::new(),
    )
}

fn contains_source_class<'db>(
    db: &'db dyn Db,
    model: &SemanticModel<'db>,
    annotation: &Expr,
    class_files: &[File],
    aliases: &mut std::collections::HashSet<ty_python_core::definition::Definition<'db>>,
) -> bool {
    use ruff_python_ast::helpers::any_over_expr;
    use ty_python_semantic::types::{Type, TypeDefinition};

    let env = model.program_environment();
    let source_class = |ty: Type<'_>| {
        matches!(ty.definition(db, &env), Some(TypeDefinition::StaticClass(class))
            if class_files.binary_search(&class.file(db)).is_ok())
    };
    any_over_expr(annotation, |operand| {
        if let Expr::StringLiteral(text) = operand {
            return model
                .enter_string_annotation(text)
                .is_some_and(|(parsed, inner)| {
                    contains_source_class(db, &inner, &parsed.syntax().body, class_files, aliases)
                });
        }
        if !matches!(operand, Expr::Name(_) | Expr::Attribute(_)) {
            return false;
        }
        match operand.inferred_type(model) {
            Some(Type::TypeAlias(alias)) => {
                with_alias_value(db, &env, alias, |definition, model, value, _| {
                    aliases.insert(definition)
                        && contains_source_class(db, model, value, class_files, aliases)
                })
                .unwrap_or(false)
            }
            Some(Type::Union(union)) => union.elements(db).iter().copied().any(source_class),
            Some(ty) => source_class(ty),
            None => false,
        }
    })
}

/// Resolve the source declaration for the operand that ty actually rejected.
fn has_declared_nominal_target(
    db: &dyn Db,
    file: File,
    diagnostic: &Diagnostic,
    class_files: &[File],
) -> bool {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};

    if class_files.is_empty() {
        return false;
    }
    let assignment = diagnostic.id().is_lint_named("invalid-assignment");
    if assignment
        && diagnostic
            .annotations()
            .iter()
            .filter(|label| !label.is_primary())
            .any(|label| {
                let span = label.get_span();
                let ruff_db::diagnostic::UnifiedFile::Ty(owner) = span.file() else {
                    return false;
                };
                span.range()
                    .is_some_and(|range| source_class_annotation(db, *owner, range, class_files))
            })
    {
        return true;
    }
    let Some(range) = primary_range(diagnostic, file) else {
        return false;
    };
    let program = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, program.python_file(db)).load(db);
    let nodes = covering_node(parsed.syntax().into(), range);
    let model = SemanticModel::new(db, program);
    nodes
        .ancestors()
        .find_map(|node| match node {
            AnyNodeRef::StmtAssign(statement) if assignment => {
                Some(statement.targets.iter().any(|target| {
                    if range != target.range()
                        && range != statement.range()
                        && range != statement.value.range()
                    {
                        return false;
                    }
                    let place = match target {
                        Expr::Attribute(_) => target,
                        Expr::Subscript(item) => item.value.as_ref(),
                        _ => return false,
                    };
                    declared_nominal_place(db, &model, place, class_files)
                }))
            }
            AnyNodeRef::ExprCall(call)
                if !assignment && call.arguments.range().contains_range(range) =>
            {
                Some(call.func.as_attribute_expr().is_some_and(|method| {
                    declared_nominal_place(db, &model, &method.value, class_files)
                }))
            }
            _ => None,
        })
        .unwrap_or(false)
}

fn property_accessor<'db>(
    db: &'db dyn Db,
    definition: ty_python_core::definition::Definition<'db>,
    context: ruff_python_ast::ExprContext,
) -> Option<ty_python_core::definition::Definition<'db>> {
    use ruff_python_ast::ExprContext;
    use ty_python_core::definition::DefinitionKind;
    use ty_python_semantic::types::{Type, TypeDefinition};

    let source = db.program_file(definition.file(db));
    let parsed = ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
    let model = SemanticModel::new(db, source);
    let ty = match definition.kind(db) {
        DefinitionKind::Function(function) => function.node(&parsed).inferred_type(&model),
        DefinitionKind::Assignment(assignment) => assignment.value(&parsed).inferred_type(&model),
        DefinitionKind::AnnotatedAssignment(assignment) => {
            assignment.value(&parsed)?.inferred_type(&model)
        }
        _ => None,
    };
    let Type::PropertyInstance(property) = ty? else {
        return None;
    };
    let accessor = match context {
        ExprContext::Load => property.getter(db),
        ExprContext::Store => property.setter(db),
        _ => None,
    }?;
    match accessor.definition(db, &model.program_environment()) {
        Some(TypeDefinition::Function(function)) => Some(function),
        _ => None,
    }
}

fn declared_nominal_place<'db>(
    db: &'db dyn Db,
    model: &SemanticModel<'db>,
    receiver: &Expr,
    class_files: &[File],
) -> bool {
    let source_class = |file, annotation: &Expr| {
        contains_source_class(
            db,
            &SemanticModel::new(db, file),
            annotation,
            class_files,
            &mut std::collections::HashSet::new(),
        )
    };
    let declared = |definition: ty_python_core::definition::Definition<'db>| {
        use ty_python_core::definition::DefinitionKind;

        let source = definition.program_file(db);
        let parsed = ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
        let annotation = match definition.kind(db) {
            DefinitionKind::AnnotatedAssignment(field) => Some(field.annotation(&parsed)),
            DefinitionKind::Parameter(parameter) => parameter.annotation(&parsed),
            _ => None,
        };
        annotation.is_some_and(|annotation| source_class(source, annotation))
    };
    if let Expr::Attribute(attribute) = receiver {
        return ty_python_semantic::definitions_for_attribute(model, attribute)
            .iter()
            .any(|declaration| {
                let ty_python_semantic::ResolvedDefinition::Definition(definition) = declaration
                else {
                    return false;
                };
                if let Some(accessor) = property_accessor(db, *definition, attribute.ctx)
                    && let ty_python_core::definition::DefinitionKind::Function(function) =
                        accessor.kind(db)
                {
                    let source = db.program_file(accessor.file(db));
                    let parsed =
                        ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
                    let function = function.node(&parsed);
                    let annotation = match attribute.ctx {
                        ruff_python_ast::ExprContext::Load => function.returns.as_deref(),
                        ruff_python_ast::ExprContext::Store => function
                            .parameters
                            .posonlyargs
                            .iter()
                            .chain(&function.parameters.args)
                            .nth(1)
                            .map(|parameter| &parameter.parameter)
                            .or(function.parameters.vararg.as_deref())
                            .and_then(|parameter| parameter.annotation.as_deref()),
                        _ => None,
                    };
                    return annotation.is_some_and(|annotation| source_class(source, annotation));
                }
                declared(*definition)
            });
    }
    let program = model.program_file();
    let index = ty_python_core::semantic_index(db, program);
    let Some(scope) = index.try_expression_scope_id(receiver) else {
        return false;
    };
    let Some(use_id) = index.try_expression_use_id(receiver.into()) else {
        return false;
    };
    let map = index.use_def_map(scope);
    map.bindings_at_use(use_id)
        .filter_map(|entry| entry.binding.definition())
        .any(|binding| {
            declared(binding)
                || map
                    .declarations_at_binding(binding)
                    .filter_map(|entry| entry.declaration.definition())
                    .any(declared)
        })
}

fn concrete_scalar<'db>(
    db: &'db dyn Db,
    env: &ty_python_semantic::types::ProgramEnvironment<'db>,
    declared: ty_python_semantic::types::Type<'db>,
) -> bool {
    use ty_python_semantic::types::{KnownClass, UnionType};
    let scalar = |class: KnownClass| class.to_instance(db, env);
    [KnownClass::Int, KnownClass::Bool, KnownClass::Float, KnownClass::Str, KnownClass::Bytes]
        .into_iter().any(|class| declared == scalar(class))
        // ty widens a float annotation to float | int.
        || declared == UnionType::from_two_elements(db, env, scalar(KnownClass::Float), scalar(KnownClass::Int))
}

fn primary_range(diagnostic: &Diagnostic, file: File) -> Option<TextRange> {
    let span = diagnostic.primary_span()?;
    (span.file() == &ruff_db::diagnostic::UnifiedFile::Ty(file))
        .then(|| span.range())
        .flatten()
}

pub(crate) fn explicit_returns(
    function: &ruff_python_ast::StmtFunctionDef,
) -> Vec<&ruff_python_ast::StmtReturn> {
    let mut exits = ruff_python_ast::helpers::ReturnStatementVisitor::default();
    exits.visit_body(&function.body);
    exits.returns
}

/// Resolve an explicit return site to its indexed function. An implicit fallthrough
/// points at the annotation, outside any return statement.
fn return_scope(db: &dyn Db, file: File, range: TextRange) -> Option<FileScopeId> {
    use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
    use ty_python_core::scope::NodeWithScopeRef;

    let program = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, program.python_file(db)).load(db);
    let body = parsed.suite();
    let stmt = body.get(body.partition_point(|stmt| stmt.end() <= range.start()))?;
    if !stmt.range().contains_range(range) {
        return None;
    }
    let nodes = covering_node(AnyNodeRef::from(stmt), range);
    let mut ancestors = nodes.ancestors();
    ancestors.find(|node| matches!(node, AnyNodeRef::StmtReturn(_)))?;
    let function = ancestors.find_map(|node| match node {
        AnyNodeRef::StmtFunctionDef(function) => Some(function),
        _ => None,
    })?;
    Some(
        ty_python_core::semantic_index(db, program)
            .node_scope(NodeWithScopeRef::Function(function)),
    )
}

/// ty identifies the matched source parameter in an argument diagnostic. Use that
/// resolution for plain functions, constructors and methods, including inherited
/// and imported declarations. A runtime builtin has no project or SDK parameter.
fn declared_parameter(
    diagnostic: &Diagnostic,
    parameters: &[(File, TextRange)],
) -> Option<(File, TextRange)> {
    diagnostic
        .sub_diagnostics()
        .iter()
        .flat_map(|sub| sub.annotations())
        .find_map(|label| {
            let span = label.get_span();
            let ruff_db::diagnostic::UnifiedFile::Ty(owner) = span.file() else {
                return None;
            };
            let range = span.range()?;
            if label.is_primary() {
                return None;
            }
            parameters
                .iter()
                .copied()
                .find(|(file, parameter)| file == owner && parameter.contains_range(range))
        })
}

/// Leave argument conversion on vendored callables to Python.
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
            .is_some_and(|definition| definition.file(db).path(db).is_vendored_path())
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
    expression
        .inferred_type(model)
        .is_some_and(|ty| is_stdlib_function_type(model, ty, owner, name))
}

fn is_stdlib_function_type<'db>(
    model: &SemanticModel<'db>,
    ty: ty_python_semantic::types::Type<'db>,
    owner: &str,
    name: &str,
) -> bool {
    use ty_python_semantic::types::{Type, TypeDefinition};
    let db = model.db();
    if !matches!(ty, Type::FunctionLiteral(_)) {
        return false;
    }
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
            && self.inputs.declaration_files == inputs.declaration_files
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
    ) -> Result<Option<Session>, String> {
        if let Some(mut hints) = self.hints.take()
            && hints.same_files(&graph.session, &graph.inputs)
        {
            hints.update(&graph.session, &graph.inputs)?;
            // Imre stored statement indices while resolving the fresh source graph.
            // Lower the identical normalized sources using the ASTs owned by this ty database.
            for (module, &file) in graph.modules.iter_mut().zip(&hints.session.files) {
                module.parsed = hints.session.parsed(file);
            }
            // The discarded fresh session can check source rules without writing to a cached snapshot.
            let fresh = std::mem::replace(&mut graph.session, hints.session.clone());
            self.hints = Some(hints);
            return Ok(Some(fresh));
        }
        Ok(None)
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
            return cached.session.errors(
                0..cached.session.files.len(),
                skipped,
                overloads,
                FileChecks::Full,
                None,
            );
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
    if let Some(error) = imre_ast::async_generators::check(module.into(), source.contains(":=")) {
        return Err(crate::frontend::diagnostic_message(
            source,
            &error.range,
            &format!("SyntaxError: {}", error.message),
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
    fn deferred_global_checks_physical_reads_and_refreshes_after_edits() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        let source = "def read():\n    left\n    right\nleft = 1\nright = 2\n";
        let path = root.join("main.py");
        let parsed = session.add(&path, source, crate::SourceKind::File).unwrap();
        let body = &parsed.suite()[0].as_function_def_stmt().unwrap().body;
        let left = body[0].as_expr_stmt().unwrap().value.as_ref();
        let right = body[1].as_expr_stmt().unwrap().value.as_ref();
        let reads = session.operand_types(0);
        assert!(reads.reads_deferred_global(left));
        assert!(reads.reads_deferred_global(right));

        let mut forwarded = left.as_name_expr().unwrap().clone();
        forwarded.id = right.as_name_expr().unwrap().id.clone();
        assert!(!reads.reads_deferred_global(&Expr::Name(forwarded)));

        session
            .write_file(path.to_str().unwrap(), &source.replace("left = 1\n", ""))
            .unwrap();
        let reads = session.operand_types(0);
        assert!(!reads.reads_deferred_global(left));
        assert!(reads.reads_deferred_global(right));
    }

    #[test]
    fn adding_a_package_child_refreshes_an_earlier_import_lookup() {
        let root = Path::new("/project");
        let mut session = Session::new(root, &[]).unwrap();
        session
            .add(
                &root.join("pkg/__init__.py"),
                "from .child import value\n",
                crate::SourceKind::File,
            )
            .unwrap();
        let missing = session.check_scopes(None, &[]).unwrap_err();
        assert!(missing.contains("unresolved-import"), "{missing}");
        session
            .add(
                &root.join("pkg/child.py"),
                "value = 37\n",
                crate::SourceKind::File,
            )
            .unwrap();
        session.check_scopes(None, &[]).unwrap();
    }

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
            .check_fallback_contracts(
                None,
                &[],
                FallbackCalls::NamedFunctions,
                0..session.files.len(),
                None,
            )
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
            .check_fallback_contracts(
                None,
                &[vec![]],
                FallbackCalls::All,
                0..session.files.len(),
                None,
            )
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
                .errors(
                    0..session.files.len(),
                    Some(&[vec![whole]]),
                    &[],
                    FileChecks::Full,
                    None
                )
                .is_ok()
        );
        assert_eq!(
            session
                .errors(0..session.files.len(), None, &[], FileChecks::Full, None)
                .unwrap_err(),
            message
        );
    }
}
