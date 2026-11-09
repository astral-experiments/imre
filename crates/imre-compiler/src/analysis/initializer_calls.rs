//! Scalar contracts at module initialization, before either native representation is selected.

use super::{Session, concrete_scalar};
use crate::frontend::phase::{Diagnostic, Kind};
use ruff_db::files::File;
use ruff_python_ast::helpers::Truthiness;
use ruff_python_ast::{Expr, ExprCall, Stmt};
use ruff_text_size::{Ranged, TextRange};
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
    /// Follow the selected initializer only while its imported sources, callable and
    /// operand evaluations are known. A registered module can still lose to sys.modules.
    pub(crate) fn initializer_call_error<'a>(
        &self,
        entry: (usize, &'a Path),
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<Diagnostic> {
        // These semantic queries establish a diagnostic. Only native lowering's
        // layout queries should retain a hint session for subsequent compilations.
        let hints = self.hint_reads.get();
        let error = self.reached_initializer_error(entry, application_module);
        self.hint_reads.set(hints);
        error
    }

    fn reached_initializer_error<'a>(
        &self,
        entry: (usize, &'a Path),
        application_module: impl Fn(&str) -> Option<(usize, &'a Path)>,
    ) -> Option<Diagnostic> {
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
                if let Some(owner) = self.initializer_import(stmt, &model, &application_module) {
                    let file = self.files[owner.0];
                    // Loading from a module may query __path__ or a missing member.
                    let passive_load = stmt.as_import_from_stmt().is_none_or(|import| {
                        let index =
                            ty_python_core::semantic_index(&self.db, self.db.program_file(file));
                        index
                            .place_table(FileScopeId::global())
                            .symbol_id("__getattr__")
                            .is_none()
                            && import
                                .names
                                .iter()
                                .all(|a| self.initialized_function(file, a.name.as_str()).is_some())
                    });
                    if self.parsed(file).suite().iter().all(passive) && passive_load {
                        imports.push((stmt.range(), file));
                        pending.push(((module, path), i + 1, imports));
                    }
                    if entered.insert(owner.0) {
                        pending.push((owner, 0, Vec::new()));
                    }
                    break;
                }
                if let Some((site, reason)) =
                    self.external_source_execution(module, &parsed, &model, stmt)
                {
                    let text = ruff_db::source::source_text(&self.db, file);
                    return Some(
                        Diagnostic::at(text.as_str(), &site, reason)
                            .with_path(path)
                            .classified(Kind::ExecutionOperand(module)),
                    );
                }
                if let Some(error) =
                    self.initializer_statement_error(module, &parsed, &model, stmt, &imports)
                {
                    return Some(error.with_path(path));
                }
                if !passive(stmt) {
                    break;
                }
            }
        }
        None
    }

    fn initializer_import<'a>(
        &self,
        stmt: &Stmt,
        model: &SemanticModel<'_>,
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
        // A package can execute parent initialization. A preloaded provider may already
        // occupy sys.modules even when search-path discovery found a Python file.
        if name.contains('.') || !unambiguous_file_module(name) {
            return None;
        }
        let owner = model.resolve_module(Some(name), 0)?.file(&self.db)?;
        let (id, path) = application_module(name)?;
        (self.files[id] == owner
            && path.extension().is_some_and(|ext| ext == "py")
            && path.file_stem().is_some_and(|s| s == name))
        .then_some((id, path))
    }

    fn initializer_statement_error(
        &self,
        module: usize,
        parsed: &ruff_db::parsed::ParsedModuleRef,
        model: &SemanticModel<'_>,
        stmt: &Stmt,
        imports: &[(TextRange, File)],
    ) -> Option<Diagnostic> {
        let file = self.files[module];
        let call = statement_call(stmt, |name| {
            self.reaching_binding(module, name).is_some()
                || name.as_name_expr().is_some_and(|n| {
                    imre_ast::providers::BUILTIN_FUNCTIONS.contains(&n.id.as_str())
                        && self.is_unbound_module_read(module, name)
                })
        })?;
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
        let def = if let Some((site, name)) = member {
            let owner = imports.iter().find(|(r, _)| *r == site)?.1;
            self.initialized_function(owner, name)?
        } else {
            binding
        };
        let DefinitionKind::Function(function) = def.kind(&self.db) else {
            return None;
        };
        let owner = def.file(&self.db);
        let owner_ast = self.parsed(owner);
        let function = function.node(&owner_ast);
        if !function.decorator_list.is_empty()
            || function.type_params.is_some()
            || !function.parameters.iter().any(|p| p.annotation().is_some())
        {
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
                return Some(error);
            }
        }
        None
    }

    fn initialized_function(&self, owner: File, name: &str) -> Option<Definition<'_>> {
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
