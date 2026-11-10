//! Resolve first-class execution operations using the declarations of their operands.

use super::{Session, is_stdlib_function_type};
use crate::frontend::phase::Diagnostic;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprCall, Stmt, StmtFunctionDef};
use ruff_text_size::{Ranged, TextRange};
use std::{collections::HashMap, path::Path};
use ty_python_core::definition::{DefinitionKind, FunctionDefinitionKind};
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{Db, HasType, SemanticModel};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExecutionIntrinsic {
    Exec,
    Eval,
}

impl ExecutionIntrinsic {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Exec => "exec",
            Self::Eval => "eval",
        }
    }

    pub(super) fn resolve(model: &SemanticModel<'_>, expression: &Expr) -> Option<Self> {
        Self::from_type(model, expression.inferred_type(model)?)
    }

    fn from_type<'db>(model: &SemanticModel<'db>, ty: Type<'db>) -> Option<Self> {
        [Self::Exec, Self::Eval]
            .into_iter()
            .find(|intrinsic| is_stdlib_function_type(model, ty, "builtins", intrinsic.name()))
    }

    pub(super) fn possible(model: &SemanticModel<'_>, expression: &Expr) -> Option<Self> {
        let ty = expression.inferred_type(model)?;
        match ty {
            Type::Union(union) => union
                .elements(model.db())
                .iter()
                .find_map(|ty| Self::from_type(model, *ty)),
            _ => Self::resolve(model, expression),
        }
    }

    pub(super) fn escape_message(self) -> String {
        format!(
            "this argument may select builtins.{} and the consumer can execute unlinked source. Pass a named Python function, or select one from a table of functions.",
            self.name()
        )
    }
}

// The linked runtime can execute a function or loader's code object. Text
// and the result of a passive compile have no such native entry on this build.
fn text_operand(model: &SemanticModel<'_>, operand: &Expr) -> bool {
    use ty_python_semantic::types::KnownClass;
    let Some(ty) = operand.inferred_type(model) else {
        return false;
    };
    let db = model.db();
    let env = model.program_environment();
    let text = |ty: Type<'_>| {
        matches!(ty, Type::NominalInstance(_) | Type::LiteralValue(_))
            && [KnownClass::Str, KnownClass::Bytes]
                .into_iter()
                .any(|class| ty.is_assignable_to(db, &env, class.to_instance(db, &env)))
    };
    match ty {
        Type::Union(types) => types.elements(db).iter().copied().any(text),
        _ => text(ty),
    }
}

#[derive(Clone)]
pub(super) struct ExecutionRequirement {
    source: SourceCallable,
    pub(super) report: Diagnostic,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SourceCallable {
    Builtin(ExecutionIntrinsic),
    SelectedBuiltin,
}

impl SourceCallable {
    fn may_be(self, name: &str) -> bool {
        match self {
            Self::Builtin(builtin) => builtin.name() == name,
            Self::SelectedBuiltin => matches!(name, "exec" | "eval"),
        }
    }

    fn missing_body(self) -> String {
        match self {
            Self::Builtin(builtin) => format!(
                "this call may select builtins.{} without a linked body for its source. Define the handlers as Python functions and select one by name, or declare the finite modules with --include-module.",
                builtin.name()
            ),
            Self::SelectedBuiltin => "this call selects a builtin by name and can execute source without a linked Python body. Call a fixed function or select from an explicit table such as {'len': len}.".into(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ArgumentFact {
    Source(SourceCallable),
    PlainString,
}

pub(super) type FunctionImports = HashMap<TextRange, (usize, Option<ExecutionRequirement>)>;
pub(super) type CapturedImports = HashMap<usize, FunctionImports>;

struct Escape<'a, 'db> {
    model: &'a SemanticModel<'db>,
    parsed: &'a ruff_db::parsed::ParsedModuleRef,
    entered: std::collections::HashSet<(TextRange, Vec<(TextRange, ArgumentFact)>)>,
    callbacks: Vec<(TextRange, ArgumentFact)>,
    guarded_names: Vec<TextRange>,
    session: &'db Session,
    module: usize,
    path: &'a Path,
    first: Option<ExecutionRequirement>,
    captured: FunctionImports,
    unreachable: Vec<TextRange>,
}

impl<'ast> Escape<'ast, '_> {
    // Follow simple aliases to the value that was actually assigned. A return or
    // parameter annotation alone cannot establish the native entry of a code object.
    fn assigned_value(&self, mut value: &'ast Expr) -> Option<&'ast Expr> {
        let mut seen = std::collections::HashSet::new();
        while let Some(def) = self.session.sole_binding(self.module, value)
            && let DefinitionKind::Assignment(assignment) = def.kind(self.model.db())
        {
            if !seen.insert(def) || assignment.unpack().is_some() {
                return None;
            }
            value = assignment.value(self.parsed);
        }
        Some(value)
    }

    fn declared_function(
        &self,
        value: &Expr,
    ) -> Option<(ruff_db::files::File, &FunctionDefinitionKind)> {
        let db = self.model.db();
        let ty @ Type::FunctionLiteral(_) = value.inferred_type(self.model)? else {
            return None;
        };
        let TypeDefinition::Function(def) = ty.definition(db, &self.model.program_environment())?
        else {
            return None;
        };
        let DefinitionKind::Function(function) = def.kind(db) else {
            return None;
        };
        let file = def.file(db);
        (self.session.files.contains(&file)
            && file
                .path(db)
                .as_system_path()
                .is_some_and(|path| path.extension() != Some("pyi")))
        .then_some((file, function))
    }

    fn local_function(&self, value: &Expr) -> Option<&'ast StmtFunctionDef> {
        let (file, function) = self.declared_function(value)?;
        if file != self.model.file() {
            return None;
        }
        let node = function.node(self.parsed);
        node.decorator_list.is_empty().then_some(node)
    }

    fn required(
        &self,
        source_callable: SourceCallable,
        range: TextRange,
        message: String,
    ) -> ExecutionRequirement {
        let source =
            ruff_db::source::source_text(&self.session.db, self.session.files[self.module]);
        ExecutionRequirement {
            source: source_callable,
            report: Diagnostic::at(source.as_str(), &range, message).with_path(self.path),
        }
    }

    fn forget_builtin(&mut self, name: &str) {
        self.captured.retain(|_, (_, requirement)| {
            requirement
                .as_ref()
                .is_none_or(|requirement| !requirement.source.may_be(name))
        });
    }

    fn has_live_import(&self, expression: &Expr) -> bool {
        !self.captured.is_empty()
            && !self
                .unreachable
                .iter()
                .any(|range| range.contains_range(expression.range()))
    }

    fn captured_target(&self, call: &ExprCall) -> Option<(usize, Option<ExecutionRequirement>)> {
        let name = call.func.as_name_expr()?;
        let db = self.model.db();
        let binding = self
            .session
            .reaching_binding(self.module, &call.func)
            .or_else(|| {
                // Free names in a helper use its module's global binding.
                let index = ty_python_core::semantic_index(db, self.model.program_file());
                let scope = ty_python_core::FileScopeId::global();
                let symbol = index.place_table(scope).symbol_id(name.id.as_str())?;
                let mut definitions = index
                    .use_def_map(scope)
                    .end_of_scope_symbol_bindings(symbol);
                let binding = definitions.next()?.binding.definition()?;
                definitions.next().is_none().then_some(binding)
            })?;
        if binding.file(db) != self.model.file() {
            return None;
        }
        let DefinitionKind::ImportFrom(import) = binding.kind(db) else {
            return None;
        };
        let alias = import.alias(self.parsed);
        let (owner, requirement) = self.captured.get(&alias.range())?;
        let (file, declaration) = self.declared_function(&call.func)?;
        let syntax = self.session.parsed(file);
        let function = declaration.node(&syntax);
        if self.session.files[*owner] != file || function.name.as_str() != alias.name.as_str() {
            return None;
        }
        crate::frontend::fixed_call_arguments(call, &function.parameters)?;
        call.arguments
            .iter_source_order()
            .all(|arg| self.argument_can_return(arg.value()))
            .then(|| (*owner, requirement.clone()))
    }

    fn argument_can_return(&self, expression: &Expr) -> bool {
        if expression.inferred_type(self.model) == Some(Type::Never) {
            return false;
        }
        let Some(function) = expression
            .as_call_expr()
            .and_then(|call| self.local_function(&call.func))
        else {
            return true;
        };
        let index = ty_python_core::semantic_index(self.model.db(), self.model.program_file());
        let Some(scope) =
            index.try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(function))
        else {
            return false;
        };
        function.is_async || scope.is_generator_function(index)
            || index.use_def_map(scope).end_of_scope_reachability() != ty_python_core::reachability_constraints::ScopedReachabilityConstraintId::ALWAYS_FALSE
            || function.body.iter().any(|s| s.is_return_stmt() && !self.unreachable.iter().any(|r| r.contains_range(s.range())))
    }

    fn source_module_is(&self, value: &Expr, name: &str) -> bool {
        let db = self.model.db();
        matches!(value.inferred_type(self.model), Some(Type::ModuleLiteral(m)) if m.module(db).name(db).as_str() == name && m.module(db).file(db).is_some_and(|f| f.path(db).is_vendored_path()))
    }

    /// Identify a function obtained from the genuine builtin namespace. Merely
    /// obtaining or inspecting it does not execute a source operand.
    fn selected_builtin(&self, value: &'ast Expr) -> Option<SourceCallable> {
        if !matches!(
            value,
            Expr::Call(_) | Expr::Subscript(_) | Expr::Name(_) | Expr::Attribute(_)
        ) {
            return None;
        }
        let value = self.assigned_value(value)?;
        if let Some((_, ArgumentFact::Source(target))) = self.parameter_fact(value) {
            return Some(target);
        }
        let key = match value {
            Expr::Call(call)
                if call.arguments.keywords.is_empty()
                    && (2..=3).contains(&call.arguments.args.len())
                    && super::resolves_to_stdlib_function(
                        self.model, &call.func, "builtins", "getattr",
                    )
                    && self.source_module_is(&call.arguments.args[0], "builtins") =>
            {
                &call.arguments.args[1]
            }
            Expr::Subscript(subscript) => {
                let namespace = self.assigned_value(&subscript.value)?;
                match namespace {
                    Expr::Attribute(attr)
                        if attr.attr.as_str() == "__dict__"
                            && self.source_module_is(&attr.value, "builtins") => {}
                    Expr::Call(call)
                        if call.arguments.keywords.is_empty()
                            && call.arguments.args.len() == 1
                            && super::resolves_to_stdlib_function(
                                self.model, &call.func, "builtins", "vars",
                            )
                            && self.source_module_is(&call.arguments.args[0], "builtins") => {}
                    _ => return None,
                }
                &subscript.slice
            }
            _ => return None,
        };
        if !self.guarded_names.is_empty()
            && self
                .parameter_fact(key)
                .is_some_and(|(parameter, _)| self.guarded_names.contains(&parameter))
        {
            return None;
        }
        let key_type = key.inferred_type(self.model)?;
        let names = self.session.parsed(self.session.execution_names);
        let [Stmt::Expr(stmt)] = names.syntax().body.as_slice() else {
            unreachable!()
        };
        let Expr::Tuple(tuple) = &*stmt.value else {
            unreachable!()
        };
        let model = SemanticModel::new(
            self.model.db(),
            self.model.db().program_file(self.session.execution_names),
        );
        let mut possible = [ExecutionIntrinsic::Exec, ExecutionIntrinsic::Eval]
            .into_iter()
            .zip(&tuple.elts)
            .filter(|(_, name)| {
                name.inferred_type(&model).is_some_and(|literal| {
                    literal.is_assignable_to(
                        self.model.db(),
                        &self.model.program_environment(),
                        key_type,
                    )
                })
            })
            .map(|(intrinsic, _)| intrinsic);
        let first = possible.next()?;
        Some(if possible.next().is_some() {
            SourceCallable::SelectedBuiltin
        } else {
            SourceCallable::Builtin(first)
        })
    }

    fn parameter_fact(&self, value: &Expr) -> Option<(TextRange, ArgumentFact)> {
        if self.callbacks.is_empty() || !value.is_name_expr() {
            return None;
        }
        let binding = self.session.reaching_binding(self.module, value)?;
        let kind = binding.kind(self.model.db());
        if !matches!(
            kind,
            DefinitionKind::Parameter(_) | DefinitionKind::LambdaParameter(_)
        ) {
            return None;
        }
        let parameter = kind.target_range(self.parsed);
        self.callbacks
            .iter()
            .copied()
            .find(|(site, _)| *site == parameter)
    }

    fn argument_fact(&self, value: &'ast Expr) -> Option<ArgumentFact> {
        if let Some(target) = self.selected_builtin(value) {
            return Some(ArgumentFact::Source(target));
        }
        let value = self.assigned_value(value)?;
        if let Some((_, fact)) = self.parameter_fact(value) {
            return Some(fact);
        }
        (value.is_string_literal_expr()
            || value.as_call_expr().is_some_and(|call| {
                super::resolves_to_stdlib_function(self.model, &call.func, "builtins", "input")
            }))
        .then_some(ArgumentFact::PlainString)
    }

    /// An unannotated parameter retains ty's unknown type. An actual input string
    /// must pass this immutable literal membership guard before the lookup below it.
    fn safe_membership(&self, test: &Expr, operator: ruff_python_ast::CmpOp) -> Option<TextRange> {
        let compare = test.as_compare_expr()?;
        if compare.ops.as_ref() != [operator] {
            return None;
        }
        let [operand, choices] = compare.operands.as_ref() else {
            return None;
        };
        let (name, ArgumentFact::PlainString) = self.parameter_fact(operand)? else {
            return None;
        };
        let elements: &[Expr] = match choices {
            Expr::Tuple(t) => &t.elts,
            Expr::List(l) => &l.elts,
            Expr::Set(s) => &s.elts,
            _ => return None,
        };
        elements
            .iter()
            .all(|v| {
                v.as_string_literal_expr()
                    .is_some_and(|s| !matches!(s.value.to_str(), "eval" | "exec"))
            })
            .then_some(name)
    }

    fn enter_call(
        &mut self,
        call: &'ast ExprCall,
        parameters: Option<&'ast ruff_python_ast::Parameters>,
        site: TextRange,
        visit: impl FnOnce(&mut Self),
    ) {
        let mut callbacks = self.callbacks.clone();
        if let Some(parameters) = parameters {
            callbacks.retain(|(p, _)| !parameters.range().contains_range(*p));
            if let Some(bound) = crate::frontend::fixed_call_arguments(call, parameters) {
                callbacks.extend(bound.into_iter().filter_map(|(p, arg)| {
                    self.argument_fact(arg)
                        .map(|target| (p.name().range(), target))
                }));
            }
        }
        if self.entered.insert((site, callbacks.clone())) {
            let outer = std::mem::replace(&mut self.callbacks, callbacks);
            visit(self);
            self.callbacks = outer;
        }
    }

    fn imported_callback(
        &self,
        module: usize,
        call: &'ast ExprCall,
    ) -> Option<ExecutionRequirement> {
        let actual: Vec<_> = call
            .arguments
            .iter_source_order()
            .filter_map(|arg| {
                self.argument_fact(arg.value())
                    .map(|source| (arg.value().range(), source))
            })
            .collect();
        if actual.is_empty() {
            return None;
        }
        let (owner, declaration) = self.declared_function(&call.func)?;
        let parsed = self.session.parsed(owner);
        let function = declaration.node(&parsed);
        let bound = crate::frontend::fixed_call_arguments(call, &function.parameters)?;
        let model = SemanticModel::new(self.model.db(), self.model.db().program_file(owner));
        let path = owner.path(self.model.db()).as_system_path()?.as_std_path();
        let mut nested = self
            .session
            .execution_visitor((module, path), &parsed, &model);
        nested.callbacks = bound
            .into_iter()
            .filter_map(|(p, arg)| {
                actual
                    .iter()
                    .find(|(r, _)| *r == arg.range())
                    .map(|(_, target)| (p.name().range(), *target))
            })
            .collect();
        nested.visit_body(&function.body);
        nested.first
    }

    fn exposes_captured_namespace(&self, value: &Expr) -> bool {
        let db = self.model.db();
        if let Expr::Attribute(attr) = value {
            let ty = attr.value.inferred_type(self.model);
            let known_frame = || {
                ty.and_then(|t| t.definition(db, &self.model.program_environment()))
                .is_some_and(|definition| matches!(definition, TypeDefinition::StaticClass(class) if class.name(db).as_deref()==Some("FrameType") && class.file(db).path(db).is_vendored_path()))
            };
            match attr.attr.as_str() {
                "__globals__" if matches!(ty, Some(Type::FunctionLiteral(_))) => return true,
                "f_globals" | "f_locals" if known_frame() => return true,
                "modules" if self.source_module_is(&attr.value, "sys") => return true,
                "__dict__" if self.source_module_is(&attr.value, "builtins") => return true,
                _ => {}
            }
        }
        matches!(value.inferred_type(self.model), Some(Type::ModuleLiteral(m))
            if m.module(db).file(db) == Some(self.model.file()) || m.module(db).name(db).as_str() == "__main__")
    }

    fn update_captured_calls(&mut self, call: &ExprCall) {
        let Some(ty @ (Type::FunctionLiteral(_) | Type::ClassLiteral(_))) =
            call.func.inferred_type(self.model)
        else {
            self.captured.clear();
            return;
        };
        if ["globals", "locals", "vars"]
            .into_iter()
            .any(|name| is_stdlib_function_type(self.model, ty, "builtins", name))
        {
            self.captured.clear();
            return;
        }
        if ["setattr", "delattr"]
            .into_iter()
            .any(|name| is_stdlib_function_type(self.model, ty, "builtins", name))
            && call
                .arguments
                .args
                .first()
                .is_some_and(|target| self.source_module_is(target, "builtins"))
        {
            if let Some(name) = call
                .arguments
                .args
                .get(1)
                .and_then(Expr::as_string_literal_expr)
            {
                self.forget_builtin(name.value.to_str());
            } else {
                self.captured.clear();
            }
            return;
        }
        if let Some((file, declaration)) = self.declared_function(&call.func)
            && file != self.model.file()
        {
            let parsed = self.session.parsed(file);
            let function = declaration.node(&parsed);
            if !function.is_async
                && !imre_ast::code_flags::is_generator(&function.body)
                && !super::initializer_calls::passive_function(function)
            {
                self.captured.clear();
            }
        }
    }

    fn compiled_function(&self, value: &'ast Expr) -> bool {
        let Some(value) = self.assigned_value(value) else {
            return false;
        };
        if value.is_lambda_expr() {
            return true;
        }
        if matches!(value, Expr::Name(_) | Expr::Attribute(_)) {
            return self
                .declared_function(value)
                .is_some_and(|(file, function)| {
                    function
                        .node(&self.session.parsed(file))
                        .decorator_list
                        .is_empty()
                });
        }
        // A closure factory can return a real nested function before its result
        // has a useful inferred type. Require the returned definition itself.
        let Expr::Call(call) = value else {
            return false;
        };
        let Some(factory) = self.local_function(&call.func) else {
            return false;
        };
        let Some((Stmt::Return(ret), definitions)) = factory.body.split_last() else {
            return false;
        };
        !factory.is_async
            && definitions.iter().all(Stmt::is_function_def_stmt)
            && ret
                .value
                .as_deref()
                .and_then(|v| self.local_function(v))
                .is_some_and(|returned| {
                    definitions
                        .iter()
                        .any(|stmt| stmt.range() == returned.range())
                })
    }

    // Member assignments have reaching definitions in the same semantic index
    // as names. A replaced loader method or module origin is not canonical.
    fn member_was_assigned(&self, value: &Expr) -> bool {
        let db = self.model.db();
        let index = ty_python_core::semantic_index(db, self.model.program_file());
        index
            .try_expression_scope_id(value)
            .zip(index.try_expression_use_id(value.into()))
            .is_some_and(|(scope, use_id)| {
                index
                    .use_def_map(scope)
                    .bindings_at_use(use_id)
                    .any(|binding| binding.binding.definition().is_some())
            })
    }

    fn registered_loader_code(&self, call: &'ast ExprCall) -> bool {
        let Expr::Attribute(method) = &*call.func else {
            return false;
        };
        let Some(Expr::Call(loader)) = self.assigned_value(&method.value) else {
            return false;
        };
        let db = self.model.db();
        let env = self.model.program_environment();
        let Some(ty @ Type::ClassLiteral(_)) = loader.func.inferred_type(self.model) else {
            return false;
        };
        let Some(TypeDefinition::StaticClass(class)) = ty.definition(db, &env) else {
            return false;
        };
        if method.attr.as_str() != "get_code"
            || self.member_was_assigned(&call.func)
            || class.file(db) != self.session.external_loader_declarations
            || class.name(db).as_deref() != Some("SourceFileLoader")
        {
            return false;
        }
        let Some(Expr::StringLiteral(name)) = call.arguments.find_positional(0) else {
            return false;
        };
        let Some(Expr::StringLiteral(loaded)) = loader.arguments.find_positional(0) else {
            return false;
        };
        let Some(origin) = loader.arguments.find_positional(1) else {
            return false;
        };
        let Expr::Attribute(path) = origin else {
            return false;
        };
        name.value.to_str() == loaded.value.to_str()
            && path.attr.as_str() == "__file__"
            && !self.member_was_assigned(origin)
            && matches!(path.value.inferred_type(self.model), Some(Type::ModuleLiteral(m))
                if m.module(db).name(db).as_str() == name.value.to_str()
                    && m.module(db).file(db).is_some_and(|file|
                        self.session.files.contains(&file)
                        && file.path(db).as_system_path().is_some_and(|p| p.extension() == Some("py"))))
    }

    fn has_executable_entry(&self, operand: &'ast Expr) -> bool {
        match self.assigned_value(operand) {
            Some(value @ Expr::Attribute(code)) if code.attr.as_str() == "__code__" => {
                !self.member_was_assigned(value) && self.compiled_function(&code.value)
            }
            Some(Expr::Call(call)) => self.registered_loader_code(call),
            _ => false,
        }
    }
}

impl<'a> Visitor<'a> for Escape<'a, '_> {
    fn visit_body(&mut self, statements: &'a [Stmt]) {
        let prior = self.guarded_names.len();
        for statement in statements {
            self.visit_stmt(statement);
            if let Stmt::If(guard) = statement
                && guard.elif_else_clauses.is_empty()
                && guard
                    .body
                    .last()
                    .is_some_and(|s| s.is_return_stmt() || s.is_raise_stmt())
                && let Some(name) = self.safe_membership(&guard.test, ruff_python_ast::CmpOp::NotIn)
            {
                self.guarded_names.push(name);
            }
        }
        self.guarded_names.truncate(prior);
    }

    fn visit_stmt(&mut self, statement: &'a Stmt) {
        if let Stmt::If(guard) = statement
            && let Some(name) = self.safe_membership(&guard.test, ruff_python_ast::CmpOp::In)
        {
            self.visit_expr(&guard.test);
            self.guarded_names.push(name);
            self.visit_body(&guard.body);
            self.guarded_names.pop();
            for clause in &guard.elif_else_clauses {
                if let Some(test) = &clause.test {
                    self.visit_expr(test);
                }
                self.visit_body(&clause.body);
            }
            return;
        }

        if let Stmt::Assign(assign) = statement
            && assign.targets.iter().all(Expr::is_name_expr)
            && ExecutionIntrinsic::possible(self.model, &assign.value).is_some()
        {
            // The semantic index retains this name binding. Check the read
            // where its value is used, including a later unmodelled consumer.
            walk_expr(self, &assign.value);
            return;
        }
        if let Stmt::FunctionDef(function) = statement {
            for decorator in &function.decorator_list {
                self.visit_expr(&decorator.expression);
            }
            for parameter in function.parameters.iter_non_variadic_params() {
                if let Some(default) = &parameter.default {
                    self.visit_expr(default);
                }
            }
        } else {
            walk_stmt(self, statement);
        }
    }

    fn visit_expr(&mut self, expression: &'a Expr) {
        if self.first.is_some() {
            return;
        }
        let Expr::Call(call) = expression else {
            if self.has_live_import(expression) {
                if self.exposes_captured_namespace(expression) {
                    self.captured.clear();
                } else if let Expr::Attribute(attr) = expression
                    && attr.ctx != ruff_python_ast::ExprContext::Load
                    && self.source_module_is(&attr.value, "builtins")
                {
                    self.forget_builtin(attr.attr.as_str());
                }
            }
            if let Expr::Attribute(attribute) = expression
                && text_operand(self.model, expression)
                && ExecutionIntrinsic::possible(self.model, &attribute.value).is_some()
            {
                // Reading string metadata of a builtin does not invoke it.
                walk_expr(self, &attribute.value);
                return;
            }
            if let Some(intrinsic) = ExecutionIntrinsic::possible(self.model, expression) {
                self.first = Some(self.required(
                    SourceCallable::Builtin(intrinsic),
                    expression.range(),
                    intrinsic.escape_message(),
                ));
                return;
            }
            if let Expr::Lambda(lambda) = expression {
                if let Some(parameters) = &lambda.parameters {
                    for parameter in parameters.iter_non_variadic_params() {
                        if let Some(default) = &parameter.default {
                            self.visit_expr(default);
                        }
                    }
                }
            } else {
                walk_expr(self, expression);
            }
            return;
        };
        // Python selects the callable before its argument expressions can rebind that name.
        let live_import = self.has_live_import(expression);
        let capture = live_import.then(|| self.captured_target(call)).flatten();
        let intrinsic = ExecutionIntrinsic::possible(self.model, &call.func);
        let source_callable = intrinsic.map(SourceCallable::Builtin).or_else(|| {
            self.selected_builtin(&call.func).filter(|_| {
                !ty_python_semantic::types::ide_support::unreachable_ranges(
                    self.model.db(),
                    self.model.program_file(),
                )
                .iter()
                .any(|site| site.range.contains_range(call.range()))
            })
        });
        if intrinsic.is_some() {
            // The builtin is the selected consumer here. Its operands still run
            // through ordinary argument and namespace dispatch.
            walk_expr(self, &call.func);
        } else {
            self.visit_expr(&call.func);
        }
        for argument in call.arguments.iter_source_order() {
            let operand = argument.value();
            self.visit_expr(operand);
        }
        if self.first.is_none()
            && let Some(source_callable) = source_callable
            && let Some(operand) = call.arguments.args.first()
            && !self.has_executable_entry(operand)
        {
            self.first = Some(self.required(
                source_callable,
                operand.range(),
                source_callable.missing_body(),
            ));
            return;
        }
        if self.first.is_none()
            && let Some((owner, requirement)) = capture
        {
            self.first = requirement.or_else(|| self.imported_callback(owner, call));
            if self.first.is_some() {
                return;
            }
        }
        if live_import {
            self.update_captured_calls(call);
        }
        if self.first.is_none()
            && let Some(Expr::Lambda(lambda)) = self.assigned_value(&call.func)
            && !imre_ast::code_flags::yields_in_expression(&lambda.body)
        {
            self.enter_call(
                call,
                lambda.parameters.as_deref(),
                lambda.range(),
                |visitor| visitor.visit_expr(&lambda.body),
            );
        }
        // Synchronous generator frames resume the same linked function body.
        if self.first.is_none()
            && let Some(node) = self.local_function(&call.func)
            && !node.is_async
        {
            self.enter_call(call, Some(&node.parameters), node.range(), |visitor| {
                visitor.visit_body(&node.body)
            });
        }
    }
}

impl Session {
    fn execution_visitor<'ast, 'db>(
        &'db self,
        (module, path): (usize, &'ast Path),
        parsed: &'ast ruff_db::parsed::ParsedModuleRef,
        model: &'ast SemanticModel<'db>,
    ) -> Escape<'ast, 'db> {
        Escape {
            model,
            parsed,
            entered: Default::default(),
            callbacks: Vec::new(),
            guarded_names: Vec::new(),
            session: self,
            module,
            path,
            first: None,
            captured: Default::default(),
            unreachable: Vec::new(),
        }
    }

    /// An initializer reading source from the genuine input builtin has no finite body
    /// to register. Resolve both callables before any later initializer contract is checked.
    pub(super) fn external_source_execution<'ast, 'db>(
        &'db self,
        source: (usize, &'ast Path),
        parsed: &'ast ruff_db::parsed::ParsedModuleRef,
        model: &'ast SemanticModel<'db>,
        value: &'ast Expr,
    ) -> Option<Diagnostic> {
        let call = value.as_call_expr()?;
        ExecutionIntrinsic::resolve(model, &call.func)?;
        let operand = call.arguments.args.first()?;
        let input = operand.as_call_expr()?;
        if !input.arguments.keywords.is_empty()
            || input.arguments.args.len() > 1
            || !input
                .arguments
                .args
                .iter()
                .all(Expr::is_string_literal_expr)
            || !call.arguments.keywords.is_empty()
            || call.arguments.args.len() > 3
        {
            return None;
        }
        for namespace in &call.arguments.args[1..] {
            let scope = namespace.as_call_expr()?;
            let function = scope.func.inferred_type(model)?;
            if !scope.arguments.is_empty()
                || !["globals", "locals"]
                    .into_iter()
                    .any(|name| is_stdlib_function_type(model, function, "builtins", name))
            {
                return None;
            }
        }
        if !is_stdlib_function_type(model, input.func.inferred_type(model)?, "builtins", "input") {
            return None;
        }
        let mut check = self.execution_visitor(source, parsed, model);
        check.visit_expr(value);
        check.first.map(|requirement| requirement.report)
    }

    pub(super) fn generator_execution_error<'ast, 'db>(
        &'db self,
        source: (usize, &'ast Path),
        parsed: &'ast ruff_db::parsed::ParsedModuleRef,
        model: &'ast SemanticModel<'db>,
        function: &'ast StmtFunctionDef,
    ) -> Option<ExecutionRequirement> {
        let mut visitor = self.execution_visitor(source, parsed, model);
        visitor.visit_body(&function.body);
        visitor.first
    }

    pub(crate) fn check_execution_escapes<'a>(
        &self,
        sources: impl Iterator<Item = (usize, &'a std::path::Path)>,
    ) -> Result<(), String> {
        self.check_execution_with_imports(sources, CapturedImports::default())
    }

    pub(super) fn check_execution_with_imports<'a>(
        &self,
        sources: impl Iterator<Item = (usize, &'a Path)>,
        mut captures: CapturedImports,
    ) -> Result<(), String> {
        for (index, path) in sources {
            let file = self.files[index];
            if path.extension().is_some_and(|extension| extension == "pyi") {
                continue;
            }
            let source = self.db.program_file(file);
            let parsed = self.parsed(file);
            let model = SemanticModel::new(&self.db, source);
            let mut visitor = self.execution_visitor((index, path), &parsed, &model);
            if let Some(imports) = captures.remove(&index)
                && !imports.is_empty()
            {
                visitor.captured = imports;
                visitor.unreachable =
                    ty_python_semantic::types::ide_support::unreachable_ranges(&self.db, source)
                        .iter()
                        .map(|unreachable| unreachable.range)
                        .collect();
            }
            visitor.visit_body(&parsed.syntax().body);
            if let Some(error) = visitor.first {
                return Err(error.report.to_string());
            }
        }
        Ok(())
    }
}
