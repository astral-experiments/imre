//! Enforce exact scalar argument representations when a first-party graph needs fallback.

use super::{
    CheckedError, DeclaredSignatures, Session, annotation_type, changed_module_binding,
    function_layout_writes, resolved_name_definition,
};
use ruff_python_ast::{
    Expr, ExprCall, Stmt, TypeParams,
    visitor::{self, Visitor},
};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::path::Path;
use ty_python_core::{SemanticIndex, definition::DefinitionKind};
use ty_python_semantic::{
    Db, HasType, SemanticModel,
    types::{Type, TypeDefinition, ide_support::ImportAliasResolution},
};

impl Session {
    /// Check executable first-party calls only when admitting fallback. The ordinary
    /// checker owns Python assignability errors; this pass adds the stricter scalar
    /// representation contract, for example `take(True)` where `take` expects `int`.
    pub(super) fn exact_scalar_calls(
        &self,
        module: usize,
        path: &Path,
        declarations: DeclaredSignatures<'_>,
        skipped: &[TextRange],
        ty_errors: &[CheckedError],
        errors: &mut Vec<(TextSize, String)>,
    ) {
        let db = &self.db;
        let source = db.program_file(self.files[module]);
        let parsed = self.parsed(self.files[module]);
        let mut calls = Calls {
            session: self,
            model: SemanticModel::new(db, source),
            index: ty_python_core::semantic_index(db, source),
            declarations,
            skipped,
            unreachable: ty_python_semantic::types::ide_support::unreachable_ranges(db, source)
                .iter()
                .map(|r| r.range)
                .collect(),
            ty_errors,
            errors,
            path,
        };
        calls.visit_body(parsed.suite());
    }
}

struct Calls<'db, 'a> {
    session: &'db Session,
    model: SemanticModel<'db>,
    index: &'db SemanticIndex<'db>,
    declarations: DeclaredSignatures<'db>,
    skipped: &'a [TextRange],
    unreachable: Vec<TextRange>,
    ty_errors: &'a [CheckedError],
    errors: &'a mut Vec<(TextSize, String)>,
    path: &'a Path,
}

impl Calls<'_, '_> {
    fn excluded(&self, expr: &Expr) -> bool {
        let range = expr.range();
        self.skipped
            .iter()
            .chain(&self.unreachable)
            .any(|r| r.contains_range(range))
            || self
                .index
                .try_expression_scope_id(expr)
                .is_some_and(|scope| self.index.is_in_type_checking_block(scope, range))
    }

    /// An operand must finish evaluation before its enclosing argument can be checked.
    fn failed(&self, expr: &Expr, before: usize) -> bool {
        self.errors.len() != before
            || self
                .ty_errors
                .iter()
                .any(|error| error.range.is_some_and(|r| expr.range().contains_range(r)))
    }

    /// Return whether the call has a uniquely selected, fully bound source convention.
    /// Supplied operands are visited and checked in the shared binder's evaluation order.
    fn check_call(&mut self, call: &ExprCall) -> bool {
        let db = &self.session.db;
        let Some(name) = call.func.as_name_expr() else {
            return false;
        };
        let Some(definition) =
            resolved_name_definition(&self.model, name, ImportAliasResolution::ResolveAliases)
        else {
            return false;
        };
        if self
            .declarations
            .call_shapes(db)
            .binary_search(&definition)
            .is_err()
        {
            return false;
        }
        let Some(callee @ Type::FunctionLiteral(_)) = call.func.inferred_type(&self.model) else {
            return false;
        };
        if !matches!(callee.definition(db, &self.model.program_environment()), Some(TypeDefinition::Function(selected)) if selected == definition)
            || changed_module_binding(db, &self.model, call, name, definition)
        {
            return false;
        }
        let DefinitionKind::Function(function) = definition.kind(db) else {
            return false;
        };
        let owner = definition.file(db);
        let parsed = self.session.parsed(owner);
        let function = function.node(&parsed);
        let Some(bound) = crate::frontend::bind_source_arguments(call, &function.parameters) else {
            return false;
        };
        if bound.missing.is_some() {
            return false;
        }
        let owner_model = SemanticModel::new(db, db.program_file(owner));
        let before = self.errors.len();
        self.visit_expr(&call.func);
        if self.failed(&call.func, before) {
            return true;
        }
        for (parameter, value) in bound.supplied {
            let before = self.errors.len();
            self.visit_expr(value);
            if self.failed(value, before) {
                return true;
            }
            let Some(actual) = value.inferred_type(&self.model) else {
                continue;
            };
            if actual == Type::Never {
                return true;
            }
            let Some(annotation) = parameter.parameter.annotation.as_deref() else {
                continue;
            };
            let Some(expected) = annotation_type(db, &owner_model, annotation) else {
                continue;
            };
            if actual.is_assignable_to(db, &self.model.program_environment(), expected)
                && !Session::written_scalar_union_accepts(&owner_model, annotation, actual)
                && let Some(error) =
                    Session::scalar_contract_error(&self.model, expected, value, actual, || true)
                && !self
                    .declarations
                    .program_files(db)
                    .iter()
                    .any(|&file| function_layout_writes(db, file).contains(&definition))
            {
                self.errors
                    .push((value.start(), error.with_path(self.path).to_string()));
                return true;
            }
        }
        true
    }
}

impl<'ast> Visitor<'ast> for Calls<'_, '_> {
    fn visit_annotation(&mut self, _: &'ast Expr) {}
    fn visit_type_params(&mut self, _: &'ast TypeParams) {}
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if !matches!(stmt, Stmt::TypeAlias(_)) {
            visitor::walk_stmt(self, stmt);
        }
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if self.excluded(expr) {
            return;
        }
        if let Expr::Call(call) = expr
            && self.check_call(call)
        {
            return;
        }
        visitor::walk_expr(self, expr);
    }
}
