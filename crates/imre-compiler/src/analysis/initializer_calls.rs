//! Scalar contracts at module initialization, before either native representation is selected.

use super::{ScalarMatch, Session, concrete_scalar};
use crate::frontend::phase::{Diagnostic, Kind};
use ruff_python_ast::helpers::Truthiness;
use ruff_python_ast::{Expr, ExprCall, Stmt, StmtReturn};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::{collections::HashMap, path::Path};
use ty_python_core::FileScopeId;
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::{Db, HasType, SemanticModel, types::Type};

/// Result of following the reached module prefix with one set of import bindings.
pub(crate) struct InitializerCheck {
    pub(crate) error: Option<Diagnostic>,
    pub(crate) read_import_binding: bool,
}

impl InitializerCheck {
    pub(crate) fn fixed_error(&self) -> Option<&Diagnostic> {
        self.error
            .as_ref()
            .filter(|error| error.kind != Kind::RegisteredExecution)
    }
}

/// Search-path discovery alone cannot prove a call reaches a bootstrap, frozen
/// or native provider's Python shadow. It can already be present in sys.modules.
pub(crate) fn unambiguous_file_module(name: &str) -> bool {
    name != "__main__"
        && imre_ast::providers::provider(name).is_none()
        && !imre_ast::providers::is_frozen(name, || true)
}

/// These statements cannot run application code or modify another module's namespace.
fn passive(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::FunctionDef(f) => {
            f.decorator_list.is_empty()
                && f.type_params.is_none()
                && f.parameters
                    .iter_non_variadic_params()
                    .all(|p| p.default.as_deref().is_none_or(Expr::is_literal_expr))
        }
        Stmt::Assign(a) => a.targets.iter().all(Expr::is_name_expr) && a.value.is_literal_expr(),
        Stmt::Expr(e) => e.value.is_literal_expr(),
        Stmt::If(guard) => {
            guard.test.is_literal_expr()
                && guard.elif_else_clauses.is_empty()
                && matches!(
                    Truthiness::from_expr(&guard.test, |_| false),
                    Truthiness::Falsey | Truthiness::False | Truthiness::None
                )
        }
        Stmt::Pass(_) => true,
        _ => false,
    }
}

pub(crate) fn inert_declaration(statement: &Stmt) -> bool {
    matches!(
        statement,
        Stmt::FunctionDef(_) | Stmt::Pass(_) | Stmt::Expr(_)
    ) && passive(statement)
}

/// Can this loaded definition run without invoking application code on literal arguments?
pub(crate) fn passive_function(function: &ruff_python_ast::StmtFunctionDef) -> bool {
    passive_function_return(function).is_some()
}

/// For an effect-free call, the first return supplies its operand. Falling off the
/// end or executing a bare return supplies None. An open body has no proven result.
fn passive_function_return(
    function: &ruff_python_ast::StmtFunctionDef,
) -> Option<Option<&StmtReturn>> {
    let mut first_return = None;
    let passive_body = function.decorator_list.is_empty()
        && function.type_params.is_none()
        && function
            .parameters
            .iter_non_variadic_params()
            .all(|parameter| {
                parameter
                    .default
                    .as_deref()
                    .is_none_or(Expr::is_literal_expr)
            })
        && function.body.iter().all(|statement| match statement {
            Stmt::Return(ret) => {
                first_return.get_or_insert(ret);
                ret.value.as_deref().is_none_or(Expr::is_literal_expr)
            }
            Stmt::Pass(_) | Stmt::Expr(_) | Stmt::If(_) => passive(statement),
            _ => false,
        });
    passive_body.then_some(first_return)
}

fn after_docstring(body: &[Stmt]) -> &[Stmt] {
    let skip = body
        .first()
        .is_some_and(ruff_python_ast::helpers::is_docstring_stmt);
    &body[usize::from(skip)..]
}

struct InitializerStatement<'a, 'db> {
    source: (usize, &'a Path),
    parsed: &'a ruff_db::parsed::ParsedModuleRef,
    model: &'a SemanticModel<'db>,
    stmt: &'a Stmt,
    guard_start: TextSize,
    imports: &'a [(TextRange, usize)],
    progress: &'a HashMap<usize, Option<TextSize>>,
}

impl Session {
    /// Find the guarded statement and the start of its proven normal path.
    /// Dormant scalar checks can still inspect the statement. Reached checks
    /// require every intervening guard to be both passive and proven true.
    pub(crate) fn guarded_call_statement<'a>(
        &self,
        module: usize,
        mut stmt: &'a Stmt,
    ) -> (&'a Stmt, TextSize) {
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        let mut first_conditional = None;
        while let Stmt::If(guard) = stmt {
            let [body] = guard.body.as_slice() else {
                break;
            };
            let passive_test = guard.test.is_literal_expr()
                || guard.test.as_compare_expr().is_some_and(|test| {
                    test.operands.iter().all(Expr::is_literal_expr)
                        && test.ops.iter().all(|op| {
                            matches!(
                                op,
                                ruff_python_ast::CmpOp::Eq
                                    | ruff_python_ast::CmpOp::NotEq
                                    | ruff_python_ast::CmpOp::Is
                                    | ruff_python_ast::CmpOp::IsNot
                            )
                        })
                });
            if !passive_test || !guard.elif_else_clauses.is_empty() {
                break;
            }
            if first_conditional.is_none()
                && guard.test.inferred_type(&model) != Some(Type::bool_literal(true))
            {
                first_conditional = Some(stmt.start());
            }
            stmt = body;
        }
        (stmt, first_conditional.unwrap_or_else(|| stmt.start()))
    }

    pub(super) fn empty_initializer(&self, module: usize) -> bool {
        after_docstring(self.parsed(self.files[module]).suite())
            .iter()
            .all(Stmt::is_pass_stmt)
    }

    /// Follow the selected initializer only while its imported sources, callable and
    /// operand evaluations are known. A registered module can still lose to sys.modules.
    pub(crate) fn initializer_call_error<'a>(
        &self,
        entry: (usize, &'a Path),
        runs_as_main: bool,
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> InitializerCheck {
        self.checked_initializer_calls(entry, runs_as_main, application_module, None)
    }

    pub(super) fn checked_initializer_calls<'a>(
        &self,
        entry: (usize, &'a Path),
        runs_as_main: bool,
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
        captures: Option<&mut super::execution::CapturedImports>,
    ) -> InitializerCheck {
        // These semantic queries establish a diagnostic. Only native lowering's
        // layout queries should retain a hint session for subsequent compilations.
        let hints = self.hint_reads.get();
        let read_import_binding = std::cell::Cell::new(false);
        let error = self.reached_initializer_error(entry, captures, |name| {
            read_import_binding.set(true);
            if name == "__main__" {
                runs_as_main.then_some(entry)
            } else {
                application_module(name)
            }
        });
        self.hint_reads.set(hints);
        InitializerCheck {
            error,
            read_import_binding: read_import_binding.get(),
        }
    }

    fn reached_initializer_error<'a>(
        &self,
        entry: (usize, &'a Path),
        mut captures: Option<&mut super::execution::CapturedImports>,
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<Diagnostic> {
        // A suspended initializer exposes the declarations preceding its import.
        // Once it completes, all of its final declarations may be read.
        let mut progress = HashMap::from([(entry.0, Some(TextSize::default()))]);
        let mut pending = vec![(entry, 0, Vec::new(), None::<((usize, &Path), usize)>)];
        'initializers: while let Some(((module, path), mut start, mut imports, binding)) =
            pending.pop()
        {
            let file = self.files[module];
            let parsed = self.parsed(file);
            let model = SemanticModel::new(&self.db, self.db.program_file(file));
            let suite = after_docstring(parsed.suite());
            let mut member = 0;
            if let Some((owner, loaded)) = binding {
                let stmt = &suite[start - 1];
                let before = progress[&owner.0];
                if !self.passive_import_binding(stmt, owner.0, before) {
                    return None;
                }
                if let Some(captures) = captures.as_deref_mut()
                    && let Stmt::ImportFrom(import) = stmt
                {
                    self.capture_function_sources(
                        import,
                        owner,
                        before,
                        captures.entry(module).or_default(),
                    );
                }
                let site = if let Stmt::Import(import) = stmt {
                    if loaded + 1 < import.names.len() {
                        start -= 1;
                        member = loaded + 1;
                    }
                    import.names[loaded].range()
                } else {
                    stmt.range()
                };
                imports.push((site, owner.0));
            }
            for (i, stmt) in suite.iter().enumerate().skip(start) {
                progress.insert(module, Some(stmt.start()));
                // The builtin namespace is resident when Python starts. Importing it
                // in a passive prefix cannot execute an application's module body.
                if let Stmt::Import(import) = stmt
                    && import
                        .names
                        .iter()
                        .all(|alias| alias.name.as_str() == "builtins")
                    && model
                        .resolve_module(Some("builtins"), 0)
                        .and_then(|m| m.file(&self.db))
                        .is_some_and(|file| file.path(&self.db).is_vendored_path())
                {
                    continue;
                }
                if let Some(owner) =
                    self.initializer_import(stmt, member, &model, entry.0, &application_module)
                {
                    pending.push(((module, path), i + 1, imports, Some((owner, member))));
                    if let std::collections::hash_map::Entry::Vacant(slot) = progress.entry(owner.0)
                    {
                        slot.insert(Some(TextSize::default()));
                        pending.push((owner, 0, Vec::new(), None));
                    }
                    continue 'initializers;
                }
                if let Some(stmt) = stmt.as_expr_stmt()
                    && let Some(error) =
                        self.external_source_execution((module, path), &parsed, &model, &stmt.value)
                {
                    return Some(error.classified(Kind::ExecutionOperand(module)));
                }
                let (body, guard_start) = self.guarded_call_statement(module, stmt);
                let statement = InitializerStatement {
                    source: (module, path),
                    parsed: &parsed,
                    model: &model,
                    stmt,
                    guard_start,
                    imports: &imports,
                    progress: &progress,
                };
                let call = match stmt {
                    Stmt::For(loop_) if !loop_.is_async => loop_.iter.as_call_expr(),
                    _ => match body {
                        Stmt::Expr(stmt) => stmt.value.as_call_expr(),
                        Stmt::Assign(stmt) => stmt.value.as_call_expr(),
                        _ => None,
                    },
                };
                if let Some(call) = call
                    && let Some(Err(error)) =
                        self.initializer_call(&statement, call, stmt.is_for_stmt())
                {
                    return Some(error);
                }
                if !passive(stmt) {
                    return None;
                }
            }
            progress.insert(module, None);
        }
        None
    }

    fn passive_import_binding(&self, stmt: &Stmt, owner: usize, before: Option<TextSize>) -> bool {
        let file = self.files[owner];
        // A from-import can query __path__ or a missing member during initialization.
        stmt.as_import_from_stmt().is_none_or(|import| {
            let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
            index
                .place_table(FileScopeId::global())
                .symbol_id("__getattr__")
                .is_none()
                && import.names.iter().all(|a| {
                    self.initialized_function(owner, a.name.as_str(), before)
                        .is_some()
                })
        })
    }

    pub(crate) fn passive_selected_import(
        &self,
        entry: usize,
        statement: &Stmt,
        owner: usize,
        module_name: &str,
        path: &Path,
    ) -> bool {
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[entry]));
        if statement
            .as_import_stmt()
            .is_some_and(|import| import.names.len() != 1)
        {
            return false;
        }
        self.initializer_import(statement, 0, &model, entry, &|name| {
            (name == module_name).then_some((owner, path))
        })
        .is_some()
            && (owner == entry || self.parsed(self.files[owner]).suite().iter().all(passive))
            && self.passive_import_binding(
                statement,
                owner,
                (owner == entry).then_some(statement.start()),
            )
    }

    fn initializer_import<'a>(
        &self,
        stmt: &Stmt,
        member: usize,
        model: &SemanticModel<'_>,
        entry: usize,
        application_module: &impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<(usize, &'a Path)> {
        let name = match stmt {
            Stmt::Import(import) if !import.is_lazy => import.names.get(member)?.name.as_str(),
            Stmt::ImportFrom(import) if !import.is_lazy && import.level == 0 => {
                import.module.as_ref()?.as_str()
            }
            _ => return None,
        };
        self.initializer_module(name, model, entry, application_module)
    }

    fn initializer_module<'a>(
        &self,
        name: &str,
        model: &SemanticModel<'_>,
        entry: usize,
        application_module: &impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<(usize, &'a Path)> {
        let (id, path) = application_module(name)?;
        // The launcher name can also reimport the same source in a new namespace.
        if id == entry {
            return (name == "__main__").then_some((id, path));
        }
        // A package can execute parent initialization. A preloaded provider may already
        // occupy sys.modules even when search-path discovery found a Python file.
        if name.contains('.') || !unambiguous_file_module(name) {
            return None;
        }
        let owner = model.resolve_module(Some(name), 0)?.file(&self.db)?;
        (self.files[id] == owner
            && path.extension().is_some_and(|ext| ext == "py")
            && path.file_stem().is_some_and(|s| s == name))
        .then_some((id, path))
    }

    fn capture_function_sources(
        &self,
        import: &ruff_python_ast::StmtImportFrom,
        owner: (usize, &Path),
        before: Option<TextSize>,
        captures: &mut super::execution::FunctionImports,
    ) {
        let file = self.files[owner.0];
        let parsed = self.parsed(file);
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        for alias in &import.names {
            let Some(binding) = self.initialized_function(owner.0, alias.name.as_str(), before)
            else {
                continue;
            };
            let DefinitionKind::Function(definition) = binding.kind(&self.db) else {
                continue;
            };
            let function = definition.node(&parsed);
            if !function.is_async && function.decorator_list.is_empty() {
                let source = imre_ast::code_flags::is_generator(&function.body)
                    .then(|| self.generator_execution_error(owner, &parsed, &model, function))
                    .flatten();
                captures.insert(alias.range(), (owner.0, source));
            }
        }
    }

    /// An error belongs to the first failing invocation. An effect-free return resumes the
    /// enclosing argument list. An unknown outcome stops this initializer proof.
    fn initializer_call<'db>(
        &'db self,
        site: &InitializerStatement<'_, 'db>,
        call: &ExprCall,
        iterated: bool,
    ) -> Option<Result<Type<'db>, Diagnostic>> {
        let (module, path) = site.source;
        let (parsed, model, stmt) = (site.parsed, site.model, site.stmt);
        let file = self.files[module];
        let (name, attr) = match call.func.as_ref() {
            name @ Expr::Name(_) => (name, None),
            Expr::Attribute(attr) if !attr.attr.starts_with("__") => {
                (&*attr.value, Some(attr.attr.as_str()))
            }
            _ => return None,
        };
        let binding = self.reaching_binding(module, name);
        let loaded = attr.is_none()
            && (binding.is_some()
                || name.as_name_expr().is_some_and(|n| {
                    imre_ast::providers::BUILTIN_FUNCTIONS.contains(&n.id.as_str())
                        && self.is_unbound_module_read(module, name)
                }));
        let mut actuals = Vec::new();
        for argument in imre_ast::evaluated_arguments(&call.arguments) {
            if argument.is_variadic() {
                return None;
            }
            let value = argument.value();
            if let Some(origin) = self.assigned_value(file, parsed, value, Expr::is_name_expr)
                && origin.is_literal_expr()
            {
                actuals.push(origin.inferred_type(model).unwrap_or_else(Type::unknown));
            } else {
                if !loaded || iterated {
                    return None;
                }
                let required = if site.guard_start != stmt.start()
                    && value.as_call_expr().is_some_and(|nested| {
                        nested.func.inferred_type(model) == Some(Type::unknown())
                    }) {
                    site.guard_start
                } else {
                    stmt.start()
                };
                if !self.unconditional_module_operand(module, required, value.range()) {
                    // Dormant source retains its existing positional contracts, without a returned value.
                    let nested = call
                        .arguments
                        .args
                        .iter()
                        .find(|arg| !arg.is_literal_expr())?
                        .as_call_expr()?;
                    return self.initializer_call(site, nested, false)?.err().map(Err);
                }
                match self.initializer_call(site, value.as_call_expr()?, false)? {
                    Ok(actual) => actuals.push(actual),
                    error @ Err(_) => return Some(error),
                }
            }
        }
        let binding = binding.filter(|binding| binding.file(&self.db) == file)?;
        let member = match (binding.kind(&self.db), attr) {
            (DefinitionKind::Function(_), None) => None,
            (DefinitionKind::Import(import), Some(attr)) => {
                Some((import.alias(parsed).range(), attr))
            }
            (DefinitionKind::ImportFrom(import), None) => Some((
                import.import(parsed).range(),
                import.alias(parsed).name.as_str(),
            )),
            _ => return None,
        };
        let (def, owner_module) = if let Some((import_site, name)) = member {
            let owner = site.imports.iter().find(|(r, _)| *r == import_site)?.1;
            (
                self.initialized_function(owner, name, site.progress[&owner])?,
                owner,
            )
        } else {
            (binding, module)
        };
        let DefinitionKind::Function(function) = def.kind(&self.db) else {
            return None;
        };
        let owner = def.file(&self.db);
        let owner_ast = self.parsed(owner);
        let function = function.node(&owner_ast);
        if !function.decorator_list.is_empty() || function.type_params.is_some() {
            return None;
        }
        let generator = || {
            let index = ty_python_core::semantic_index(&self.db, self.db.program_file(owner));
            index
                .try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(function))
                .map(|scope| scope.is_generator_function(index))
        };
        if iterated && (owner == file || function.is_async || !generator()?) {
            return None;
        }
        let bound = crate::frontend::fixed_call_arguments(call, &function.parameters)?;
        let owner_model = SemanticModel::new(&self.db, self.db.program_file(owner));
        for ((p, value), actual) in bound.into_iter().zip(actuals) {
            if p.parameter.annotation.is_some()
                && let Some(expected) = p.parameter.inferred_type(&owner_model)
                && let Some(error) = self.scalar_argument_error(model, expected, value, actual)
            {
                return Some(Err(error.with_path(path)));
            }
        }
        if !iterated
            && !function.is_async
            && let Some(ret) = passive_function_return(function)
            && !generator()?
        {
            let actual = match ret.and_then(|ret| ret.value.as_deref()) {
                Some(value) => value.inferred_type(&owner_model)?,
                None => Type::none(&self.db, &owner_model.program_environment()),
            };
            return Some(Ok(actual));
        }
        // Read the body only after the real initializer and its passive arguments have
        // selected this function. Literal guards above are also checked for scalar contracts.
        if owner == file || function.is_async {
            return None;
        }
        let source = (
            owner_module,
            owner.path(&self.db).as_system_path()?.as_std_path(),
        );
        let error = match stmt {
            Stmt::For(_) => {
                self.generator_execution_error(source, &owner_ast, &owner_model, function)?
                    .report
            }
            Stmt::Assign(_) | Stmt::Expr(_) => {
                let first = after_docstring(&function.body)
                    .iter()
                    .find(|stmt| !passive(stmt))?;
                let expression = match first {
                    Stmt::Expr(stmt) => &*stmt.value,
                    Stmt::Assign(stmt) => &*stmt.value,
                    Stmt::Return(stmt) => stmt.value.as_deref()?,
                    _ => return None,
                };
                let execution =
                    self.external_source_execution(source, &owner_ast, &owner_model, expression)?;
                if generator()? {
                    return None;
                }
                execution
            }
            _ => return None,
        };
        Some(Err(error.classified(Kind::RegisteredExecution)))
    }

    fn initialized_function(
        &self,
        module: usize,
        name: &str,
        before: Option<TextSize>,
    ) -> Option<Definition<'_>> {
        // Exact modules return ordinary fields directly. Known ModuleType methods
        // are non-data descriptors; other special attributes need runtime lookup.
        if before.is_some()
            && name.starts_with("__")
            && !imre_ast::providers::MODULE_METHODS.contains(&name)
        {
            return None;
        }
        let owner = self.files[module];
        let def = self.final_module_binding(owner, name)?;
        let DefinitionKind::Function(function) = def.kind(&self.db) else {
            return None;
        };
        let parsed = self.parsed(owner);
        let range = function.node(&parsed).range();
        if before.is_some_and(|at| range.end() > at) {
            return None;
        }
        parsed
            .suite()
            .iter()
            .any(|s| s.range() == range)
            .then_some(def)
    }

    pub(super) fn scalar_argument_error<'db>(
        &'db self,
        model: &SemanticModel<'db>,
        expected: Type<'db>,
        operand: &Expr,
        actual: Type<'db>,
    ) -> Option<Diagnostic> {
        let env = model.program_environment();
        if !concrete_scalar(&self.db, &env, expected)
            || actual.is_assignable_to(&self.db, &env, expected)
        {
            return None;
        }
        let actual = self
            .scalar_layout(&env, actual, ScalarMatch::Assignable)
            .map_or_else(
                || actual.display(&self.db, &env).to_string(),
                |t| t.to_string(),
            );
        let message = format!(
            "expected {}, got {actual}; annotations are enforced contracts",
            expected.display(&self.db, &env)
        );
        let text = ruff_db::source::source_text(&self.db, model.file());
        Some(Diagnostic::at(text.as_str(), operand, message).classified(Kind::DeclaredArgument))
    }
}
