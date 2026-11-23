//! Check arguments, stores and returns against the selected source declarations.

use super::{
    CheckedError, ClosedFileRoot, Session, annotation_type, changed_module_binding,
    function_layout_writes, return_scope,
};
use ruff_db::files::File;
use ruff_python_ast::{
    Expr, ExprCall, Stmt, StmtReturn, TypeParams,
    visitor::{self, Visitor},
};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::path::Path;
use ty_python_core::{
    SemanticIndex,
    definition::{Definition, DefinitionKind},
    scope::FileScopeId,
};
use ty_python_semantic::{
    Db, HasType, SemanticModel,
    types::{KnownClass, Type, TypeDefinition},
};

impl Session {
    /// Supplement ty's assignability checks on executable first-party operands.
    pub(super) fn exact_operands(
        &self,
        root: &ClosedFileRoot<'_>,
        path: &Path,
        program: &[File],
        returns: &[FileScopeId],
        ty_errors: &[CheckedError],
        errors: &mut Vec<(TextSize, String)>,
    ) {
        let db = &self.db;
        let file = root.file;
        let source = db.program_file(file);
        let parsed = self.parsed(file);
        let mut calls = Calls {
            session: self,
            file,
            model: SemanticModel::new(db, source),
            index: ty_python_core::semantic_index(db, source),
            program,
            returns,
            skipped: root.skipped,
            checked: root.overloads,
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

/// A write through an object or class can invalidate ty's declared descriptor
/// without changing the type it reports for a later lookup.
#[derive(PartialEq, Eq, salsa::SalsaValue)]
struct MemberWrite<'db> {
    receiver: Type<'db>,
    member: Option<String>,
    scope: FileScopeId,
    end: TextSize,
}

#[salsa::tracked(returns(ref))]
fn member_writes(db: &dyn Db, file: File) -> Vec<MemberWrite<'_>> {
    struct Writes<'db> {
        model: SemanticModel<'db>,
        writes: Vec<MemberWrite<'db>>,
        statement_end: TextSize,
    }

    impl<'ast> Visitor<'ast> for Writes<'_> {
        fn visit_stmt(&mut self, stmt: &'ast Stmt) {
            if matches!(stmt, Stmt::AnnAssign(a) if a.value.is_none()) {
                return;
            }
            let outer = std::mem::replace(&mut self.statement_end, stmt.end());
            visitor::walk_stmt(self, stmt);
            self.statement_end = outer;
        }

        fn visit_expr(&mut self, expr: &'ast Expr) {
            use ruff_python_ast::ExprContext;
            let key = |expr: &Expr| {
                expr.as_string_literal_expr()
                    .map(|s| s.value.to_str().to_owned())
            };
            let write = match expr {
                Expr::Attribute(a) if a.ctx != ExprContext::Load => Some((
                    a.value.as_ref(),
                    Some(a.attr.to_string()),
                    self.statement_end,
                )),
                Expr::Subscript(s) if s.ctx != ExprContext::Load => s
                    .value
                    .as_attribute_expr()
                    .filter(|a| a.attr.as_str() == "__dict__")
                    .map(|a| (a.value.as_ref(), key(&s.slice), self.statement_end)),
                Expr::Call(call)
                    if call.arguments.args.len() >= 2
                        && ["setattr", "delattr"].iter().any(|name| {
                            super::resolves_to_stdlib_function(
                                &self.model,
                                &call.func,
                                "builtins",
                                name,
                            )
                        }) =>
                {
                    Some((
                        &call.arguments.args[0],
                        key(&call.arguments.args[1]),
                        call.end(),
                    ))
                }
                _ => None,
            };
            let db = self.model.db();
            if let Some((object, member, end)) = write
                && let Some(scope) = ty_python_core::semantic_index(db, self.model.program_file())
                    .try_expression_scope_id(object)
                && let Some(receiver) = object.inferred_type(&self.model)
                && !ty_python_semantic::types::ide_support::unreachable_ranges(
                    db,
                    self.model.program_file(),
                )
                .iter()
                .any(|r| r.range.contains_range(expr.range()))
            {
                self.writes.push(MemberWrite {
                    receiver,
                    member,
                    scope,
                    end,
                });
            }
            visitor::walk_expr(self, expr);
        }
    }

    let source = db.program_file(file);
    let mut visitor = Writes {
        model: SemanticModel::new(db, source),
        writes: Vec::new(),
        statement_end: TextSize::default(),
    };
    let parsed = ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
    visitor.visit_body(parsed.suite());
    visitor.writes
}

struct Calls<'db, 'a> {
    session: &'db Session,
    file: File,
    model: SemanticModel<'db>,
    index: &'db SemanticIndex<'db>,
    program: &'a [File],
    returns: &'a [FileScopeId],
    skipped: &'a [TextRange],
    checked: &'a [super::CheckedDiagnostic],
    unreachable: Vec<TextRange>,
    ty_errors: &'a [CheckedError],
    errors: &'a mut Vec<(TextSize, String)>,
    path: &'a Path,
}

/// Nested writes and opaque calls can replace a selected binding or mutate
/// the function without a new definition reaching the call in ty.
#[salsa::tracked]
fn may_replace<'db>(
    db: &'db dyn Db,
    file: File,
    definition: ty_python_core::definition::Definition<'db>,
    caller: File,
    local: Option<ruff_python_ast::name::Name>,
) -> bool {
    let source = db.program_file(file);
    let owner = definition.file(db);
    let original = definition.name(db);
    let local_namespace = file == owner || file == caller;
    let binds = |namespace: File, name: &str| {
        namespace == owner && original.as_deref() == Some(name)
            || namespace == caller && local.as_deref() == Some(name)
    };
    let index = ty_python_core::semantic_index(db, source);
    if local_namespace
        && index.scope_ids().any(|scope| {
            let id = scope.file_scope_id(db);
            !id.is_global()
                && index.place_table(id).symbols().any(|s| {
                    s.is_bound()
                        && (s.is_global() || s.is_nonlocal())
                        && binds(file, s.name().as_str())
                })
        })
    {
        return true;
    }
    let model = SemanticModel::new(db, source);
    let exposed = |ty| match ty {
        Some(ty @ Type::FunctionLiteral(_)) => matches!(
            ty.definition(db, &model.program_environment()),
            Some(TypeDefinition::Function(f)) if f == definition
        ),
        Some(Type::ModuleLiteral(m)) => m
            .module(db)
            .file(db)
            .is_some_and(|f| f == owner || f == caller),
        _ => false,
    };
    let parsed = ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
    ruff_python_ast::helpers::any_over_body(parsed.suite(), |expr| match expr {
        Expr::Call(call) => {
            imre_ast::evaluated_arguments(&call.arguments)
                .any(|arg| exposed(arg.value().inferred_type(&model)))
                || local_namespace
                    && call.func.inferred_type(&model).is_some_and(|ty| {
                        super::select_stdlib_function(&model, ty, "builtins", |f| {
                            matches!(f.name.as_str(), "globals" | "locals" | "vars" | "exec")
                                .then_some(())
                        })
                        .is_some()
                    })
        }
        Expr::Attribute(attr) if attr.ctx != ruff_python_ast::ExprContext::Load => {
            // A write through a known module or function alias, or through
            // an opaque receiver such as sys.modules[__name__].
            let key = attr.attr.as_str();
            match attr.value.inferred_type(&model) {
                Some(Type::ModuleLiteral(m)) => {
                    m.module(db).file(db).is_some_and(|f| binds(f, key))
                }
                ty @ Some(Type::FunctionLiteral(_)) => key == "__code__" && exposed(ty),
                _ => local_namespace && (key == "__code__" || binds(file, key)),
            }
        }
        Expr::Attribute(attr) => attr.attr.as_str() == "__globals__" && local_namespace,
        _ => false,
    })
}

impl<'db> Calls<'db, '_> {
    /// ty tracks the callable value at the point of use, including copied and
    /// imported bindings. Only a bound method supplies an implicit first argument.
    fn selected(
        &self,
        model: &SemanticModel<'db>,
        call: &ExprCall,
    ) -> Option<(Definition<'db>, bool)> {
        let db = &self.session.db;
        let callee @ (Type::FunctionLiteral(_) | Type::BoundMethod(_)) =
            call.func.inferred_type(model)?
        else {
            return None;
        };
        let TypeDefinition::Function(definition) =
            callee.definition(db, &model.program_environment())?
        else {
            return None;
        };
        let owner = definition.file(db);
        if !self.program.contains(&owner) {
            return None;
        }
        let DefinitionKind::Function(function) = definition.kind(db) else {
            return None;
        };
        let parsed = self.session.parsed(owner);
        let function = function.node(&parsed);
        if !function.decorator_list.is_empty() {
            let [descriptor] = function.decorator_list.as_slice() else {
                return None;
            };
            let module = self.session.files.iter().position(|&f| f == owner)?;
            if ![KnownClass::Classmethod, KnownClass::Staticmethod]
                .into_iter()
                .any(|kind| {
                    self.session
                        .is_known_class(module, &descriptor.expression, kind)
                })
            {
                return None;
            }
        }
        Some((definition, matches!(callee, Type::BoundMethod(_))))
    }

    /// Check namespace writes at each load, including the load which captured
    /// a descriptor. Later replacement of the member cannot change a saved method.
    fn binding_changed(
        &self,
        model: &SemanticModel<'db>,
        value: &Expr,
        definition: Definition<'db>,
        seen: &mut Vec<Definition<'db>>,
    ) -> bool {
        let db = &self.session.db;
        if changed_module_binding(db, model, value, definition) {
            return true;
        }
        if let Expr::Name(name) = value {
            let Some(binding) = super::shipped_exports::binding_at_use(db, model.file(), value)
                .or_else(|| super::sole_global_name(model, name))
            else {
                return false;
            };
            if seen.contains(&binding) {
                return true;
            }
            seen.push(binding);
            let parsed = self.session.parsed(binding.file(db));
            let origin = match binding.kind(db) {
                DefinitionKind::Function(_) => return binding != definition,
                DefinitionKind::Assignment(a) if a.unpack().is_none() => Some(a.value(&parsed)),
                DefinitionKind::AnnotatedAssignment(a) => a.value(&parsed),
                _ => None,
            };
            return origin
                .is_some_and(|origin| self.binding_changed(model, origin, definition, seen));
        }
        let Expr::Attribute(attr) = value else {
            return false;
        };
        let scope =
            ty_python_core::semantic_index(db, model.program_file()).try_expression_scope_id(value);
        let receiver = attr
            .value
            .inferred_type(model)
            .and_then(|ty| ty.definition(db, &model.program_environment()));
        // An inherited method also depends on the descriptor on the defining class.
        let owner = match definition.scope(db).node(db) {
            ty_python_core::scope::NodeWithScopeKind::Class(class) => {
                let parsed = self.session.parsed(definition.file(db));
                ty_python_core::semantic_index(db, definition.program_file(db))
                    .try_definition(class.node(&parsed))
                    .map(TypeDefinition::StaticClass)
            }
            _ => None,
        };
        self.program.iter().any(|&file| {
            member_writes(db, file).iter().any(|write| {
                (file != model.file() || Some(write.scope) != scope || write.end <= value.end())
                    && write
                        .member
                        .as_deref()
                        .is_none_or(|name| name == attr.attr.as_str())
                    && write
                        .receiver
                        .definition(db, &model.program_environment())
                        .is_some_and(|class| {
                            Some(&class) == receiver.as_ref() || Some(&class) == owner.as_ref()
                        })
            })
        })
    }

    fn changed(
        &self,
        model: &SemanticModel<'db>,
        call: &ExprCall,
        definition: Definition<'db>,
    ) -> bool {
        self.program
            .iter()
            .any(|&file| function_layout_writes(&self.session.db, file).contains(&definition))
            || self.binding_changed(model, &call.func, definition, &mut Vec::new())
    }

    /// Body proofs require both the callable binding and its code to survive.
    fn stable(
        &self,
        model: &SemanticModel<'db>,
        call: &ExprCall,
        definition: Definition<'db>,
    ) -> bool {
        !self.changed(model, call, definition)
            && !self.program.iter().any(|&file| {
                *may_replace(
                    &self.session.db,
                    file,
                    definition,
                    model.file(),
                    call.func.as_name_expr().map(|n| n.id.clone()),
                )
            })
    }

    /// A passive first-party function that returns a parameter forwards this
    /// call's operand. An opaque earlier initializer need not prevent checking
    /// the result when the selected definition and argument binding are stable.
    fn forwarded_type(&self, value: &Expr) -> Option<Type<'db>> {
        let call = value.as_call_expr()?;
        let (definition, method) = self.selected(&self.model, call)?;
        let db = &self.session.db;
        let DefinitionKind::Function(function) = definition.kind(db) else {
            return None;
        };
        let file = definition.file(db);
        let parsed = self.session.parsed(file);
        let function = function.node(&parsed);
        if function.is_async || function.returns.is_some() {
            return None;
        }
        let (_, returned) =
            super::initializer_calls::passive_function_return(function, |value| {
                self.session.parameter_read(file, &parsed, value)
            })??;
        let (_, operand) =
            crate::frontend::fixed_display_call_arguments(call, &function.parameters, method)?
                .into_iter()
                .find(|(p, _)| p.name().range() == returned)?;
        if self.session.indexed_generator(file, function) != Some(false)
            || !self.stable(&self.model, call, definition)
        {
            return None;
        }
        let actual = operand.inferred_type(&self.model)?;
        if actual.is_unknown() {
            self.forwarded_type(operand)
        } else {
            Some(actual)
        }
    }

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
        model: &SemanticModel<'db>,
        expr: &Expr,
        active: &mut Vec<(usize, TextSize)>,
    ) -> bool {
        if expr.inferred_type(model) == Some(Type::Never) {
            return true;
        }
        if let Expr::Attribute(attr) = expr {
            return self.cannot_return_in(model, &attr.value, active);
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
        let Some((definition, _)) = self.selected(model, call) else {
            return false;
        };
        let DefinitionKind::Function(function) = definition.kind(db) else {
            return false;
        };
        let owner = definition.file(db);
        let Some(module) = self.session.files.iter().position(|&file| file == owner) else {
            return false;
        };
        let parsed = self.session.parsed(owner);
        let function = function.node(&parsed);
        if function.is_async || self.session.indexed_generator(owner, function) != Some(false) {
            return false;
        }
        // Only inspect possible replacements if this body could stop evaluation.
        let stable = || self.stable(model, call, definition);
        if self.session.nonreturning_function(module, function) {
            return stable();
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
        result && stable()
    }

    /// Return whether the call has a uniquely selected, fully bound source convention.
    /// Supplied operands are visited and checked in the shared binder's evaluation order.
    fn check_call(&mut self, call: &ExprCall) -> bool {
        let db = &self.session.db;
        let Some((definition, method)) = self.selected(&self.model, call) else {
            return false;
        };
        let DefinitionKind::Function(function) = definition.kind(db) else {
            return false;
        };
        let owner = definition.file(db);
        let parsed = self.session.parsed(owner);
        let function = function.node(&parsed);
        let Some(bound) =
            crate::frontend::fixed_display_call_arguments(call, &function.parameters, method)
        else {
            return false;
        };
        let owner_model = SemanticModel::new(db, db.program_file(owner));
        let before = self.errors.len();
        self.visit_expr(&call.func);
        if self.failed(&call.func, before) || self.cannot_return(&call.func) {
            return true;
        }
        for (parameter, value) in bound {
            if self.check_argument(
                value,
                &owner_model,
                parameter.parameter.annotation.as_deref(),
                Some((function, parameter.name().as_str())),
                call,
                definition,
            ) {
                break;
            }
        }
        true
    }

    /// Visit an operand before checking its scalar contract. A failing operand
    /// or one that never returns stops evaluation of the remaining arguments.
    fn check_argument(
        &mut self,
        value: &Expr,
        owner: &SemanticModel<'db>,
        annotation: Option<&Expr>,
        parameter: Option<(&ruff_python_ast::StmtFunctionDef, &str)>,
        call: &ExprCall,
        definition: Definition<'db>,
    ) -> bool {
        let db = &self.session.db;
        if self.excluded(value) {
            return false;
        }
        if let Some(annotation) = annotation {
            if let Expr::StringLiteral(text) = annotation
                && let Some((parsed, inner)) = owner.enter_string_annotation(text)
            {
                return self.check_argument(
                    value,
                    &inner,
                    Some(&parsed.syntax().body),
                    parameter,
                    call,
                    definition,
                );
            }
            if let Some(Type::TypeAlias(alias)) = annotation.inferred_type(owner)
                && annotation_type(db, owner, annotation).is_some()
                && let Some(stop) = super::with_alias_value(
                    db,
                    &owner.program_environment(),
                    alias,
                    |_, model, expr, _| {
                        self.check_argument(value, model, Some(expr), parameter, call, definition)
                    },
                )
            {
                return stop;
            }
            if let Some(children) = super::stores::display_operands(owner, annotation, value) {
                return children.into_iter().any(|(item, slot)| {
                    self.check_argument(item, owner, Some(slot), None, call, definition)
                });
            }
        }
        let before = self.errors.len();
        self.visit_expr(value);
        if self.failed(value, before) || self.cannot_return(value) {
            return true;
        }
        if let Some(annotation) = annotation
            && let Some(actual) = value.inferred_type(&self.model)
            && let Some(expected) = annotation_type(db, owner, annotation)
            && actual.is_assignable_to(db, &self.model.program_environment(), expected)
            && let Some(actual) = if actual.is_unknown() {
                self.forwarded_type(value)
            } else {
                Some(actual)
            }
            && !Session::written_scalar_union_accepts(owner, annotation, actual)
            && let Some(error) =
                Session::scalar_contract_error(&self.model, expected, value, actual, || true)
            && !parameter.is_some_and(|(function, name)| {
                Session::sign_argument(owner, function, name, expected, actual)
                    && match call.func.as_ref() {
                        Expr::Name(name) => {
                            super::resolved_name_definition(
                                &self.model,
                                name,
                                super::ImportAliasResolution::ResolveAliases,
                            ) == Some(definition)
                        }
                        Expr::Attribute(_) => true,
                        _ => false,
                    }
            })
            && !self.changed(&self.model, call, definition)
        {
            self.errors
                .push((value.start(), error.with_path(self.path).to_string()));
            return true;
        }
        false
    }

    /// Check the result only after evaluating its operand, including nested calls.
    fn check_return(&mut self, ret: &StmtReturn) {
        let Some(value) = ret.value.as_deref() else {
            return;
        };
        if self.excluded(value) {
            return;
        }
        let before = self.errors.len();
        self.visit_expr(value);
        if self.failed(value, before)
            || self.cannot_return(value)
            || self
                .checked
                .contains(&super::CheckedDiagnostic::NativeReturn(value.range()))
        {
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
        if let Some(error) =
            super::stores::value_error(&self.model, &self.model, annotation, value, &[])
        {
            self.errors
                .push((value.start(), error.with_path(self.path).to_string()));
        }
    }

    fn check_stores(&mut self, node: ruff_python_ast::AnyNodeRef<'_>, before: usize) {
        super::stores::for_each_store(self.index, node, |store| {
            let Some(value) = store.value else { return };
            if self.excluded(value)
                || self.failed(value, before)
                || self.failed(store.target, before)
                || matches!(node, ruff_python_ast::AnyNodeRef::StmtAnnAssign(stmt)
                    if self.checked.iter().any(|note| matches!(note,
                         super::CheckedDiagnostic::NativeOperand { operand, annotation, .. }
                         if *operand == value.range() && *annotation == stmt.annotation.range())))
                || self
                    .checked
                    .contains(&super::CheckedDiagnostic::FrameScalarStore(value.range()))
            {
                return;
            }
            if let Some(error) = store.error(&self.model, self.program)
                && !self.cannot_return(value)
            {
                self.errors
                    .push((value.start(), error.with_path(self.path).to_string()));
            }
        });
    }
}

impl<'ast> Visitor<'ast> for Calls<'_, '_> {
    fn visit_annotation(&mut self, _: &'ast Expr) {}
    fn visit_type_params(&mut self, _: &'ast TypeParams) {}
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let Stmt::Return(ret) = stmt {
            self.check_return(ret);
        } else if !matches!(stmt, Stmt::TypeAlias(_)) {
            let before = self.errors.len();
            visitor::walk_stmt(self, stmt);
            self.check_stores(stmt.into(), before);
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
        let before = self.errors.len();
        visitor::walk_expr(self, expr);
        if matches!(expr, Expr::Named(_)) {
            self.check_stores(expr.into(), before);
        }
    }
}
