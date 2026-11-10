//! Scalar contracts at module initialization, before either native representation is selected.

use super::{Session, concrete_scalar};
use crate::frontend::phase::{Diagnostic, Kind};
use ruff_python_ast::helpers::Truthiness;
use ruff_python_ast::{Expr, ExprCall, Stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::{collections::HashSet, path::Path};
use ty_python_core::FileScopeId;
use ty_python_core::definition::{Definition, DefinitionKind};
use ty_python_semantic::{Db, HasType, SemanticModel, types::Type};

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

/// No statement in this function can enter application code when all supplied
/// and defaulted arguments are literals. The caller still establishes its identity.
pub(crate) fn passive_function(function: &ruff_python_ast::StmtFunctionDef) -> bool {
    function.decorator_list.is_empty()
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
            Stmt::Return(ret) => ret.value.as_deref().is_none_or(Expr::is_literal_expr),
            Stmt::Pass(_) | Stmt::Expr(_) | Stmt::If(_) => passive(statement),
            _ => false,
        })
}

fn after_docstring(body: &[Stmt]) -> &[Stmt] {
    let skip = body
        .first()
        .is_some_and(ruff_python_ast::helpers::is_docstring_stmt);
    &body[usize::from(skip)..]
}

/// A standalone call with no earlier operation in its statement. A literal guard may
/// make the entire suite unreachable to ty without changing any argument bindings.
fn statement_call(mut stmt: &Stmt, bound_name: impl Fn(&Expr) -> bool) -> Option<&ExprCall> {
    while let Stmt::If(guard) = stmt {
        let [body] = guard.body.as_slice() else {
            return None;
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
            return None;
        }
        stmt = body;
    }
    let value = match stmt {
        Stmt::Expr(stmt) => &stmt.value,
        Stmt::Assign(stmt) => &stmt.value,
        _ => return None,
    };
    let mut call = value.as_call_expr()?;
    // A name lookup and literal operands can precede another call without
    // invoking the outer callable. Assignment targets follow the entire RHS.
    while call.func.is_name_expr()
        && bound_name(&call.func)
        && let Some(nested) = call
            .arguments
            .args
            .iter()
            .find(|arg| !arg.is_literal_expr())
            .and_then(Expr::as_call_expr)
    {
        call = nested;
    }
    Some(call)
}

impl Session {
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
    ) -> Option<Diagnostic> {
        self.checked_initializer_calls(entry, runs_as_main, application_module, None)
    }

    pub(super) fn checked_initializer_calls<'a>(
        &self,
        entry: (usize, &'a Path),
        runs_as_main: bool,
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
        captures: Option<&mut super::execution::CapturedImports>,
    ) -> Option<Diagnostic> {
        // These semantic queries establish a diagnostic. Only native lowering's
        // layout queries should retain a hint session for subsequent compilations.
        let hints = self.hint_reads.get();
        let error = self.reached_initializer_error(entry, captures, |name| {
            if name == "__main__" {
                runs_as_main.then_some(entry)
            } else {
                application_module(name)
            }
        });
        self.hint_reads.set(hints);
        error
    }

    fn reached_initializer_error<'a>(
        &self,
        entry: (usize, &'a Path),
        mut captures: Option<&mut super::execution::CapturedImports>,
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<Diagnostic> {
        // The startup module is already in sys.modules. Its namespace is visible
        // to imports before its initializer finishes, including from a child.
        let mut resident = (entry.0, TextSize::default());
        let mut entered = HashSet::from([entry.0]);
        let mut pending = vec![(entry, 0, Vec::new())];
        while let Some(((module, path), start, mut imports)) = pending.pop() {
            let file = self.files[module];
            let parsed = self.parsed(file);
            let model = SemanticModel::new(&self.db, self.db.program_file(file));
            for (i, stmt) in after_docstring(parsed.suite())
                .iter()
                .enumerate()
                .skip(start)
            {
                if module == resident.0 {
                    resident.1 = stmt.start();
                }
                // Each import in a comma-separated list must leave the following binding intact.
                if let Stmt::Import(import) = stmt
                    && !import.is_lazy
                    && import.names.len() > 1
                    && import.names.iter().all(|alias| {
                        self.initializer_module(
                            alias.name.as_str(),
                            &model,
                            entry.0,
                            &application_module,
                        )
                        .is_some_and(|(owner, _)| self.passive_import_load(stmt, owner, resident))
                    })
                {
                    continue;
                }
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
                    self.initializer_import(stmt, &model, entry.0, &application_module)
                {
                    if self.passive_import_load(stmt, owner.0, resident) {
                        if let Some(captures) = captures.as_deref_mut()
                            && let Stmt::ImportFrom(import) = stmt
                        {
                            self.capture_function_sources(
                                import,
                                owner,
                                resident,
                                captures.entry(module).or_default(),
                            );
                        }
                        imports.push((stmt.range(), owner.0));
                        pending.push(((module, path), i + 1, imports));
                    }
                    if entered.insert(owner.0) {
                        pending.push((owner, 0, Vec::new()));
                    }
                    break;
                }
                if let Some(stmt) = stmt.as_expr_stmt()
                    && let Some(error) =
                        self.external_source_execution((module, path), &parsed, &model, &stmt.value)
                {
                    return Some(error.classified(Kind::ExecutionOperand(module)));
                }
                if let Some(error) = self.initializer_statement_error(
                    (module, path),
                    &parsed,
                    &model,
                    stmt,
                    &imports,
                    resident,
                ) {
                    return Some(error);
                }
                if !passive(stmt) {
                    break;
                }
            }
        }
        None
    }

    fn passive_import_load(&self, stmt: &Stmt, owner: usize, resident: (usize, TextSize)) -> bool {
        let file = self.files[owner];
        // A from-import can query __path__ or a missing member during initialization.
        let passive_load = stmt.as_import_from_stmt().is_none_or(|import| {
            let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
            index
                .place_table(FileScopeId::global())
                .symbol_id("__getattr__")
                .is_none()
                && import.names.iter().all(|a| {
                    self.initialized_function(owner, a.name.as_str(), resident)
                        .is_some()
                })
        });
        passive_load && (owner == resident.0 || self.parsed(file).suite().iter().all(passive))
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
        self.initializer_import(statement, &model, entry, &|name| {
            (name == module_name).then_some((owner, path))
        })
        .is_some()
            && self.passive_import_load(statement, owner, (entry, statement.start()))
    }

    fn initializer_import<'a>(
        &self,
        stmt: &Stmt,
        model: &SemanticModel<'_>,
        entry: usize,
        application_module: &impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<(usize, &'a Path)> {
        let name = match stmt {
            Stmt::Import(import) if !import.is_lazy && import.names.len() == 1 => {
                import.names[0].name.as_str()
            }
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
        resident: (usize, TextSize),
        captures: &mut super::execution::FunctionImports,
    ) {
        let file = self.files[owner.0];
        let parsed = self.parsed(file);
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        for alias in &import.names {
            let Some(binding) = self.initialized_function(owner.0, alias.name.as_str(), resident)
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

    fn initializer_statement_error(
        &self,
        source: (usize, &Path),
        parsed: &ruff_db::parsed::ParsedModuleRef,
        model: &SemanticModel<'_>,
        stmt: &Stmt,
        imports: &[(TextRange, usize)],
        resident: (usize, TextSize),
    ) -> Option<Diagnostic> {
        let (module, path) = source;
        let file = self.files[module];
        let call = match stmt {
            Stmt::For(loop_) if !loop_.is_async => loop_.iter.as_call_expr()?,
            _ => statement_call(stmt, |name| {
                self.reaching_binding(module, name).is_some()
                    || name.as_name_expr().is_some_and(|n| {
                        imre_ast::providers::BUILTIN_FUNCTIONS.contains(&n.id.as_str())
                            && self.is_unbound_module_read(module, name)
                    })
            })?,
        };
        let (name, attr) = match call.func.as_ref() {
            name @ Expr::Name(_) => (name, None),
            Expr::Attribute(attr) if !attr.attr.starts_with("__") => {
                (&*attr.value, Some(attr.attr.as_str()))
            }
            _ => return None,
        };
        let binding = self.reaching_binding(module, name)?;
        if binding.file(&self.db) != file {
            return None;
        }
        let member = match (binding.kind(&self.db), attr) {
            (DefinitionKind::Function(_), None) => None,
            (DefinitionKind::Import(import), Some(attr)) => {
                Some((import.import(parsed).range(), attr))
            }
            (DefinitionKind::ImportFrom(import), None) => Some((
                import.import(parsed).range(),
                import.alias(parsed).name.as_str(),
            )),
            _ => return None,
        };
        let (def, owner_module) = if let Some((site, name)) = member {
            let owner = imports.iter().find(|(r, _)| *r == site)?.1;
            (self.initialized_function(owner, name, resident)?, owner)
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
        if matches!(stmt, Stmt::For(_)) && (owner == file || function.is_async || !generator()?) {
            return None;
        }
        let bound = crate::frontend::fixed_call_arguments(call, &function.parameters)?;
        // Every supplied argument must be passive, including the ones following a mismatch.
        let operands = bound
            .into_iter()
            .map(|(p, value)| {
                self.literal_origin(module, parsed, value)
                    .map(|origin| (p, value, origin))
            })
            .collect::<Option<Vec<_>>>()?;
        let owner_model = SemanticModel::new(&self.db, self.db.program_file(owner));
        for (p, value, origin) in operands {
            if p.parameter.annotation.is_some()
                && let Some(expected) = p.parameter.inferred_type(&owner_model)
                && let Some(error) =
                    self.scalar_argument_error(module, model, expected, value, origin)
            {
                return Some(error.with_path(path));
            }
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
        Some(error.classified(Kind::RegisteredExecution))
    }

    fn initialized_function(
        &self,
        module: usize,
        name: &str,
        resident: (usize, TextSize),
    ) -> Option<Definition<'_>> {
        // Exact modules return ordinary fields directly. Known ModuleType methods
        // are non-data descriptors; other special attributes need runtime lookup.
        if module == resident.0
            && name.starts_with("__")
            && !imre_ast::providers::MODULE_METHODS.contains(&name)
        {
            return None;
        }
        let owner = self.files[module];
        let global = FileScopeId::global();
        let index = ty_python_core::semantic_index(&self.db, self.db.program_file(owner));
        let symbol = index.place_table(global).symbol_id(name)?;
        let mut bindings = index
            .use_def_map(global)
            .end_of_scope_symbol_bindings(symbol);
        let def = bindings.next()?.binding.definition()?;
        if bindings.next().is_some() || def.file(&self.db) != owner {
            return None;
        }
        let DefinitionKind::Function(function) = def.kind(&self.db) else {
            return None;
        };
        let parsed = self.parsed(owner);
        let range = function.node(&parsed).range();
        if module == resident.0 && range.end() > resident.1 {
            return None;
        }
        parsed
            .suite()
            .iter()
            .any(|s| s.range() == range)
            .then_some(def)
    }

    fn literal_origin<'a>(
        &self,
        module: usize,
        parsed: &'a ruff_db::parsed::ParsedModuleRef,
        value: &'a Expr,
    ) -> Option<&'a Expr> {
        if value.is_literal_expr() {
            return Some(value);
        }
        let def = self.reaching_binding(module, value)?;
        if def.file(&self.db) != self.files[module] || !value.is_name_expr() {
            return None;
        }
        let DefinitionKind::Assignment(assignment) = def.kind(&self.db) else {
            return None;
        };
        let literal = assignment.value(parsed);
        (assignment.unpack().is_none() && literal.is_literal_expr()).then_some(literal)
    }

    pub(super) fn scalar_argument_error<'db>(
        &'db self,
        module: usize,
        model: &SemanticModel<'db>,
        expected: Type<'db>,
        operand: &Expr,
        origin: &Expr,
    ) -> Option<Diagnostic> {
        let env = model.program_environment();
        let actual = origin.inferred_type(model)?;
        if !concrete_scalar(&self.db, &env, expected)
            || actual.is_assignable_to(&self.db, &env, expected)
        {
            return None;
        }
        let actual = self.scalar_type(module, origin).map_or_else(
            || actual.display(&self.db, &env).to_string(),
            |t| t.to_string(),
        );
        let message = format!(
            "expected {}, got {actual}; annotations are enforced contracts",
            expected.display(&self.db, &env)
        );
        let text = ruff_db::source::source_text(&self.db, self.files[module]);
        Some(Diagnostic::at(text.as_str(), operand, message).classified(Kind::DeclaredArgument))
    }
}
