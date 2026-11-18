//! Enforce exact scalar argument and return representations when a graph needs fallback.

use super::{
    CheckedError, DeclaredSignatures, Session, annotation_type, changed_module_binding,
    function_layout_writes, nested_global_bindings, resolved_name_definition, return_scope,
};
use ruff_db::files::File;
use ruff_python_ast::{
    Expr, ExprCall, Stmt, StmtReturn, TypeParams,
    visitor::{self, Visitor},
};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::{collections::HashSet, path::Path};
use ty_python_core::{
    SemanticIndex,
    definition::{Definition, DefinitionKind},
    scope::FileScopeId,
};
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
        returns: &[FileScopeId],
        skipped: &[TextRange],
        ty_errors: &[CheckedError],
        errors: &mut Vec<(TextSize, String)>,
    ) {
        let db = &self.db;
        let source = db.program_file(self.files[module]);
        let parsed = self.parsed(self.files[module]);
        let mut calls = Calls {
            session: self,
            file: self.files[module],
            model: SemanticModel::new(db, source),
            index: ty_python_core::semantic_index(db, source),
            declarations,
            returns,
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
    file: File,
    model: SemanticModel<'db>,
    index: &'db SemanticIndex<'db>,
    declarations: DeclaredSignatures<'db>,
    returns: &'a [FileScopeId],
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

    /// Follow only stable first-party calls whose body cannot supply a value. The
    /// annotation of a function which always raises can still be `int`, so ty's
    /// expression type alone does not establish that a later operand is reached.
    fn cannot_return(&self, expr: &Expr) -> bool {
        self.cannot_return_in(&self.model, expr, &mut Vec::new())
    }

    fn cannot_return_in(
        &self,
        model: &SemanticModel<'_>,
        expr: &Expr,
        active: &mut Vec<(usize, TextSize)>,
    ) -> bool {
        let never = expr.inferred_type(model) == Some(Type::Never);
        if never {
            // A mutable first-party function can stop honoring its Never annotation.
            // Other Never expressions retain the semantic checker's flow fact.
            let selected_source = expr.as_call_expr().is_some_and(|call| {
                matches!(call.func.inferred_type(model), Some(callee @ Type::FunctionLiteral(_))
                    if matches!(callee.definition(&self.session.db, &model.program_environment()),
                        Some(TypeDefinition::Function(definition)) if self.declarations.call_shapes(&self.session.db).binary_search(&definition).is_ok()))
            });
            if !selected_source {
                return true;
            }
        }
        let Expr::Call(call) = expr else {
            return false;
        };
        if self.cannot_return_in(model, &call.func, active)
            || imre_ast::evaluated_arguments(&call.arguments)
                .any(|arg| self.cannot_return_in(model, arg.value(), active))
        {
            return true;
        }
        // An arbitrary argument can replace the function's code or global
        // binding before invocation. Literal arguments cannot run such a hook.
        if !imre_ast::evaluated_arguments(&call.arguments)
            .all(|arg| !arg.is_variadic() && arg.value().is_literal_expr())
        {
            return false;
        }
        let db = &self.session.db;
        let Some(name) = call.func.as_name_expr() else {
            return false;
        };
        let Some(definition) =
            resolved_name_definition(model, name, ImportAliasResolution::ResolveAliases)
        else {
            return false;
        };
        if self
            .declarations
            .call_shapes(db)
            .binary_search(&definition)
            .is_err()
            || !matches!(call.func.inferred_type(model), Some(callee @ Type::FunctionLiteral(_)) if matches!(callee.definition(db, &model.program_environment()), Some(TypeDefinition::Function(selected)) if selected == definition))
            || changed_module_binding(db, model, call, name, definition)
            || nested_global_bindings(db, model.file()).contains(name.id.as_str())
            || self.declarations.program_files(db).iter().any(|&file| {
                function_layout_writes(db, file).contains(&definition)
                    || escaped_functions(db, file).contains(&definition)
            })
        {
            return false;
        }
        if never {
            return true;
        }
        let DefinitionKind::Function(function) = definition.kind(db) else {
            return false;
        };
        let owner = definition.file(db);
        let Some(module) = self.session.files.iter().position(|&file| file == owner) else {
            return false;
        };
        let parsed = self.session.parsed(owner);
        let function = function.node(&parsed);
        if self.session.nonreturning_function(module, function) {
            return true;
        }
        if self.session.call_may_return_implicitly(module, function) {
            return false;
        }
        let [Stmt::Return(returned)] = function.body.as_slice() else {
            return false;
        };
        let Some(value) = returned.value.as_deref() else {
            return false;
        };
        let key = (module, function.start());
        if active.contains(&key) {
            return false;
        }
        active.push(key);
        let owner_model = SemanticModel::new(db, db.program_file(owner));
        let result = self.cannot_return_in(&owner_model, value, active);
        active.pop();
        result
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
            if self.cannot_return(value) {
                return true;
            }
            let Some(actual) = value.inferred_type(&self.model) else {
                continue;
            };
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

    /// Supplement assignability for scalar literal returns after checking the operand.
    /// Effectful operands need a sound nonreturning/evaluation proof before widening this cut.
    fn check_return(&mut self, ret: &StmtReturn) {
        let Some(value) = ret.value.as_deref() else {
            return;
        };
        if self.excluded(value) {
            return;
        }
        let before = self.errors.len();
        self.visit_expr(value);
        if self.failed(value, before) || !value.is_literal_expr() {
            return;
        }
        let db = &self.session.db;
        let Some(scope) = return_scope(db, self.file, value.range()) else {
            return;
        };
        if self.returns.binary_search(&scope).is_err() {
            return;
        }
        let ty_python_core::scope::NodeWithScopeKind::Function(node) =
            self.index.scope(scope).node()
        else {
            return;
        };
        let parsed = self.session.parsed(self.file);
        let function = node.node(&parsed);
        let Some(annotation) = function.returns.as_deref() else {
            return;
        };
        if let Some(expected) = annotation_type(db, &self.model, annotation)
            && let Some(actual) = value.inferred_type(&self.model)
            && actual.is_assignable_to(db, &self.model.program_environment(), expected)
            && !Session::written_scalar_union_accepts(&self.model, annotation, actual)
            && let Some(error) =
                Session::scalar_contract_error(&self.model, expected, value, actual, || true)
        {
            self.errors
                .push((value.start(), error.with_path(self.path).to_string()));
        }
    }
}

impl<'ast> Visitor<'ast> for Calls<'_, '_> {
    fn visit_annotation(&mut self, _: &'ast Expr) {}
    fn visit_type_params(&mut self, _: &'ast TypeParams) {}
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let Stmt::Return(ret) = stmt {
            self.check_return(ret);
        } else if !matches!(stmt, Stmt::TypeAlias(_)) {
            visitor::walk_stmt(self, stmt);
        }
    }
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if self.excluded(expr) {
            return;
        }
        if let Expr::Call(call) = expr {
            if self.check_call(call) {
                return;
            }
            self.visit_expr(&call.func);
            if self.cannot_return(&call.func) {
                return;
            }
            for argument in imre_ast::evaluated_arguments(&call.arguments) {
                let value = argument.value();
                self.visit_expr(value);
                if self.cannot_return(value) {
                    break;
                }
            }
            return;
        }
        visitor::walk_expr(self, expr);
    }
}

/// Functions exposed as values can have their code replaced through an alias,
/// container, or callback. Direct named calls do not expose the function object.
#[salsa::tracked(returns(ref))]
fn escaped_functions<'db>(db: &'db dyn Db, file: File) -> HashSet<Definition<'db>> {
    struct Escapes<'db> {
        model: SemanticModel<'db>,
        functions: HashSet<Definition<'db>>,
    }
    impl<'ast> Visitor<'ast> for Escapes<'_> {
        fn visit_annotation(&mut self, _: &'ast Expr) {}
        fn visit_type_params(&mut self, _: &'ast TypeParams) {}
        fn visit_expr(&mut self, expr: &'ast Expr) {
            if let Expr::Call(call) = expr {
                if !call.func.is_name_expr() {
                    self.visit_expr(&call.func);
                }
                for argument in imre_ast::evaluated_arguments(&call.arguments) {
                    self.visit_expr(argument.value());
                }
                return;
            }
            // Reading a function's code/defaults is passive; a direct store is
            // already tracked by function_layout_writes.
            if let Expr::Attribute(attribute) = expr
                && attribute.value.is_name_expr()
                && matches!(
                    attribute.attr.as_str(),
                    "__code__" | "__defaults__" | "__kwdefaults__"
                )
            {
                return;
            }
            if expr.is_name_expr()
                && let Some(ty @ Type::FunctionLiteral(_)) = expr.inferred_type(&self.model)
                && let Some(TypeDefinition::Function(definition)) =
                    ty.definition(self.model.db(), &self.model.program_environment())
            {
                self.functions.insert(definition);
            }
            visitor::walk_expr(self, expr);
        }
    }
    let source = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
    let mut escapes = Escapes {
        model: SemanticModel::new(db, source),
        functions: HashSet::new(),
    };
    escapes.visit_body(parsed.suite());
    escapes.functions
}

#[cfg(test)]
mod tests {
    #[test]
    fn nonreturning_proof_requires_an_unescaped_stable_function() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.py");
        let prefix = "buffer = bytearray()\nbuffer.append(97)\nprint(buffer)\ndef take(first: int, second: int): pass\ndef die() -> int:\n    raise RuntimeError()\ndef replacement() -> int: return 1\n";
        for mutation in [
            "die.__code__ = replacement.__code__\n",
            "alias = die\nalias.__code__ = replacement.__code__\n",
            "aliases = [die]\naliases[0].__code__ = replacement.__code__\n",
            "setattr(die, '__code__', replacement.__code__)\n",
            "def mutate():\n    global die\n    die = replacement\nmutate()\n",
        ] {
            std::fs::write(&path, format!("{prefix}{mutation}take(die(), True)\n")).unwrap();
            let error = crate::check_file(&path, None).unwrap_err();
            assert!(
                error.contains("expected int, got bool"),
                "{mutation}: {error}"
            );
        }
        for passive in ["", "saved_code = die.__code__\n"] {
            std::fs::write(&path, format!("{prefix}{passive}take(die(), True)\n")).unwrap();
            crate::check_file(&path, None).unwrap();
        }
    }
}
