//! Resolve first-class execution operations using the declarations of their operands.

use super::{Session, is_stdlib_function_type};
use crate::dynamic::literal::{LiteralSite, ParsedLiteral};
use crate::frontend::phase::Diagnostic;
use ruff_db::files::File;
use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprCall, Stmt, StmtFunctionDef};
use ruff_text_size::{Ranged, TextRange, TextSize};
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
            ty => Self::from_type(model, ty),
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
            Self::Builtin(builtin) => {
                let literal_hint = match builtin {
                    ExecutionIntrinsic::Eval => " For Python literal data, use ast.literal_eval.",
                    ExecutionIntrinsic::Exec => "",
                };
                format!(
                    "this call may select builtins.{} without a linked body for its source. Define the handlers as Python functions and select one by name, or declare the finite modules with --include-module.{literal_hint}",
                    builtin.name()
                )
            }
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
pub(super) type CapturedImports = HashMap<usize, InitializerImports>;

#[derive(Default)]
pub(super) struct InitializerImports {
    pub(super) functions: FunctionImports,
    /// The active import whose selected initializer cannot complete.
    pub(super) stop: Option<TextSize>,
}

struct CapturedImport<'a> {
    owner: File,
    name: &'a str,
    requirement: Option<ExecutionRequirement>,
}

struct Escape<'a, 'db> {
    model: &'a SemanticModel<'db>,
    parsed: &'a ruff_db::parsed::ParsedModuleRef,
    entered: std::collections::HashSet<(TextRange, Vec<(TextRange, ArgumentFact)>)>,
    callbacks: Vec<(TextRange, ArgumentFact)>,
    guarded_names: Vec<TextRange>,
    session: &'db Session,
    open_providers: &'a [File],
    path: &'a Path,
    first: Option<ExecutionRequirement>,
    captured: FunctionImports,
    unreachable: Vec<TextRange>,
}

type SourceFunction<'db> = (File, &'db FunctionDefinitionKind);

impl<'ast, 'db> Escape<'ast, 'db> {
    // Follow simple aliases to the value that was actually assigned. A return or
    // parameter annotation alone cannot establish the native entry of a code object.
    fn assigned_value(&self, value: &'ast Expr) -> Option<&'ast Expr> {
        self.session
            .assigned_value(self.model.file(), self.parsed, value, |_| true)
    }

    fn declared_function(&self, value: &Expr) -> Option<SourceFunction<'db>> {
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
        (file == self.model.file()
            || self.session.files.contains(&file)
                && file
                    .path(db)
                    .as_system_path()
                    .is_some_and(|path| path.extension() != Some("pyi")))
        .then_some((file, function))
    }

    fn local_function(
        &self,
        (file, function): SourceFunction<'_>,
    ) -> Option<&'ast StmtFunctionDef> {
        (file == self.model.file() && !function.has_decorators())
            .then(|| function.node(self.parsed))
    }

    fn required(
        &self,
        source_callable: SourceCallable,
        range: TextRange,
        message: String,
    ) -> ExecutionRequirement {
        let source = ruff_db::source::source_text(&self.session.db, self.model.file());
        ExecutionRequirement {
            source: source_callable,
            report: Diagnostic::at(source.as_str(), &range, message).with_path(self.path),
        }
    }

    fn reachable(&self, site: TextRange) -> bool {
        !ty_python_semantic::types::ide_support::unreachable_ranges(
            self.model.db(),
            self.model.program_file(),
        )
        .iter()
        .any(|unreachable| unreachable.range.contains_range(site))
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

    fn captured_target(&self, callee: &Expr) -> Option<CapturedImport<'ast>> {
        let name = callee.as_name_expr()?;
        let db = self.model.db();
        let binding = self
            .session
            .file_binding(self.model.file(), callee)
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
        Some(CapturedImport {
            owner: self.session.files[*owner],
            name: alias.name.as_str(),
            requirement: requirement.clone(),
        })
    }

    fn argument_can_return(&self, expression: &Expr) -> bool {
        if expression.inferred_type(self.model) == Some(Type::Never) {
            return false;
        }
        let Some(function) = expression
            .as_call_expr()
            .and_then(|call| self.declared_function(&call.func))
            .and_then(|target| self.local_function(target))
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

    /// A resolved operand can carry a caller's argument or select a function from builtins.
    fn bound_argument(&self, value: &'ast Expr) -> Option<ArgumentFact> {
        if let Some((_, fact)) = self.parameter_fact(value) {
            return Some(fact);
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
        Some(ArgumentFact::Source(if possible.next().is_some() {
            SourceCallable::SelectedBuiltin
        } else {
            SourceCallable::Builtin(first)
        }))
    }

    fn parameter_fact(&self, value: &Expr) -> Option<(TextRange, ArgumentFact)> {
        if self.callbacks.is_empty() {
            return None;
        }
        let parameter = self
            .session
            .parameter_read(self.model.file(), self.parsed, value)?;
        self.callbacks
            .iter()
            .copied()
            .find(|(site, _)| *site == parameter)
    }

    fn argument_fact(&self, value: &'ast Expr) -> Option<ArgumentFact> {
        let value = self.assigned_value(value)?;
        if let Some(fact) = self.bound_argument(value) {
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

    fn imported_execution(
        &self,
        (owner, declaration): SourceFunction<'_>,
        capture: Option<CapturedImport<'_>>,
        call: &'ast ExprCall,
    ) -> Option<ExecutionRequirement> {
        let follows_open_source =
            owner != self.model.file() && self.open_providers.contains(&owner);
        let capture = capture.filter(|capture| capture.owner == owner);
        if !follows_open_source && capture.is_none() {
            return None;
        }
        let parsed = self.session.parsed(owner);
        let function = declaration.node(&parsed);
        let capture = capture.filter(|capture| capture.name == function.name.as_str());
        let captured = capture.is_some();
        let bound = crate::frontend::fixed_call_arguments(call, &function.parameters)?;
        if !bound
            .iter()
            .all(|(_, argument)| self.argument_can_return(argument))
        {
            return None;
        }
        if let Some(requirement) = capture.and_then(|capture| capture.requirement) {
            return Some(requirement);
        }
        let callbacks: Vec<_> = bound
            .into_iter()
            .filter_map(|(parameter, argument)| {
                self.argument_fact(argument)
                    .map(|fact| (parameter.name().range(), fact))
            })
            .collect();
        let callback = captured
            && callbacks
                .iter()
                .any(|(_, fact)| matches!(fact, ArgumentFact::Source(_)));
        if !follows_open_source && !callback {
            return None;
        }
        if !callback
            && (!function.decorator_list.is_empty()
                || function.is_async
                || self.session.indexed_generator(owner, function)?
                || !self.reachable(call.range()))
        {
            return None;
        }
        let model = SemanticModel::new(self.model.db(), self.model.db().program_file(owner));
        let path = owner.path(self.model.db()).as_system_path()?.as_std_path();
        let mut nested = self.session.execution_visitor(path, &parsed, &model);
        nested.callbacks = callbacks;
        nested.visit_body(&function.body);
        nested.first.filter(|error| {
            callback || error.source == SourceCallable::Builtin(ExecutionIntrinsic::Exec)
        })
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

    fn update_captured_calls(&mut self, call: &ExprCall, target: Option<SourceFunction<'_>>) {
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
        if let Some((file, declaration)) = target
            && file != self.model.file()
        {
            let parsed = self.session.parsed(file);
            let function = declaration.node(&parsed);
            if !function.is_async
                && self.session.indexed_generator(file, function) != Some(true)
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
                .is_some_and(|(_, function)| !function.has_decorators());
        }
        // A closure factory can return a real nested function before its result
        // has a useful inferred type. Require the returned definition itself.
        let Expr::Call(call) = value else {
            return false;
        };
        let Some(factory) = self
            .declared_function(&call.func)
            .and_then(|target| self.local_function(target))
        else {
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
                .and_then(|v| self.declared_function(v))
                .and_then(|target| self.local_function(target))
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
            if let Some(intrinsic) = ExecutionIntrinsic::possible(self.model, expression)
                && self.reachable(expression.range())
            {
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
        let capture = live_import
            .then(|| self.captured_target(&call.func))
            .flatten();
        let intrinsic = ExecutionIntrinsic::possible(self.model, &call.func);
        let callee = self.assigned_value(&call.func);
        let source_callable = intrinsic
            .map(SourceCallable::Builtin)
            .or_else(|| match self.bound_argument(callee?)? {
                ArgumentFact::Source(target) => Some(target),
                ArgumentFact::PlainString => None,
            })
            .filter(|_| self.reachable(call.range()))
            .filter(|_| {
                !self
                    .session
                    .reads_custom_builtins(self.model.file(), self.parsed, &call.func)
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
        if self.first.is_some() {
            return;
        }
        if let Some(source_callable) = source_callable
            && let Some(operand) = call.arguments.args.first()
            && !(source_callable == SourceCallable::Builtin(ExecutionIntrinsic::Exec)
                && (crate::frontend::exec_source_operand(call)
                    .is_some_and(Expr::is_string_literal_expr)
                    || self
                        .session
                        .selected_exec_literal(self.model.file(), self.parsed, self.model, call)
                        .is_some()))
            && !self.has_executable_entry(operand)
        {
            self.first = Some(self.required(
                source_callable,
                operand.range(),
                source_callable.missing_body(),
            ));
            return;
        }
        let target = self.declared_function(&call.func);
        if let Some(target) = target
            && (capture.is_some() || !self.open_providers.is_empty())
            && let Some(requirement) = self.imported_execution(target, capture, call)
        {
            self.first = Some(requirement);
            return;
        }
        if live_import {
            self.update_captured_calls(call, target);
        }
        if let Some(Expr::Lambda(lambda)) = callee
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
            && let Some(node) = target.and_then(|target| self.local_function(target))
            && !node.is_async
        {
            self.enter_call(call, Some(&node.parameters), node.range(), |visitor| {
                visitor.visit_body(&node.body)
            });
        }
    }
}

/// A directly declared function captures the module's initial builtins when
/// only inert declarations precede its creation. Its own name is bound afterward.
fn captures_initial_builtins(parsed: &ParsedModuleRef, function: &StmtFunctionDef) -> bool {
    let body = parsed.suite();
    let Ok(at) = body.binary_search_by_key(&function.start(), Ranged::start) else {
        return false;
    };
    body[at].range() == function.range()
        && super::inert_declaration(&body[at])
        && body[..at].iter().all(|statement| {
            super::inert_declaration(statement)
                && statement
                    .as_function_def_stmt()
                    .is_none_or(|def| def.name.as_str() != "__builtins__")
        })
}

impl Session {
    /// A function captures its defining namespace's __builtins__ at creation.
    /// A lexical builtin type alone cannot establish lookup through a replacement.
    fn has_custom_builtins(&self, file: File) -> bool {
        let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
        index
            .place_table(ty_python_core::FileScopeId::global())
            .symbol_by_name("__builtins__")
            .is_some_and(|symbol| symbol.is_bound())
    }

    /// A bare fallback in a function uses the captured builtins. Genuinely bound
    /// names, qualified attributes, and the active module frame do their own lookup.
    fn reads_custom_builtins(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        expression: &Expr,
    ) -> bool {
        use ty_python_core::scope::NodeWithScopeKind;
        let Some(name) = expression.as_name_expr() else {
            return false;
        };
        if !self.has_custom_builtins(file) {
            return false;
        }
        let index = ty_python_core::semantic_index(&self.db, self.db.program_file(file));
        let global = ty_python_core::FileScopeId::global();
        if index
            .place_table(global)
            .symbol_by_name(name.id.as_str())
            .is_some_and(|symbol| symbol.is_bound())
            || self.file_binding(file, expression).is_some()
        {
            return false;
        }
        let Some(scope) = index.try_expression_scope_id(expression) else {
            return false;
        };
        index
            .ancestor_scopes(scope)
            .find_map(|(_, scope)| match scope.node() {
                NodeWithScopeKind::Function(def) => Some(
                    file.path(&self.db).as_system_path().is_none()
                        || !captures_initial_builtins(parsed, def.node(parsed)),
                ),
                NodeWithScopeKind::Lambda(_) => Some(true),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// The physical formal supplying this name at its use, while its binding is unchanged.
    pub(super) fn parameter_read(
        &self,
        owner: File,
        parsed: &ParsedModuleRef,
        value: &Expr,
    ) -> Option<TextRange> {
        value.as_name_expr()?;
        let kind = self.file_binding(owner, value)?.kind(&self.db);
        matches!(
            kind,
            DefinitionKind::Parameter(_) | DefinitionKind::LambdaParameter(_)
        )
        .then(|| kind.target_range(parsed))
    }

    pub(super) fn assigned_value<'a>(
        &self,
        file: File,
        parsed: &'a ParsedModuleRef,
        mut value: &'a Expr,
        follow: impl Fn(&Expr) -> bool,
    ) -> Option<&'a Expr> {
        let mut seen = std::collections::HashSet::new();
        while follow(value)
            && let Some(def) = self.file_binding(file, value)
            && let DefinitionKind::Assignment(assignment) = def.kind(&self.db)
        {
            if !seen.insert(def) || assignment.unpack().is_some() {
                return None;
            }
            value = assignment.value(parsed);
        }
        Some(value)
    }

    fn selected_exec_literal<'a>(
        &self,
        file: File,
        parsed: &'a ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &'a ExprCall,
    ) -> Option<&'a ruff_python_ast::ExprStringLiteral> {
        let operand = crate::frontend::exec_source_operand(call)?;
        if ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
            || self.reads_custom_builtins(file, parsed, &call.func)
        {
            return None;
        }
        self.assigned_value(file, parsed, operand, Expr::is_name_expr)?
            .as_string_literal_expr()
    }

    pub(crate) fn fixed_exec_call(&self, module: usize, call: &ExprCall) -> bool {
        let Some(operand) = crate::frontend::exec_source_operand(call) else {
            return false;
        };
        if operand.is_string_literal_expr() {
            return self.is_stdlib_function(module, &call.func, "builtins", "exec");
        }
        let file = self.files[module];
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        self.selected_exec_literal(file, &self.parsed(file), &model, call)
            .is_some()
    }

    /// Resolve fixed operands after the selected parent's immediate imports are registered.
    /// Each returned parse supplies the next level's imports and its eventual checked module.
    pub(crate) fn select_literal_level(
        &self,
        parent: File,
        parsed: &ParsedModuleRef,
        source: &str,
        inherited_flags: u32,
        candidates: &[TextRange],
    ) -> Vec<(LiteralSite, Result<ParsedLiteral, String>)> {
        use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
        if candidates.is_empty() {
            return Vec::new();
        }
        let model = SemanticModel::new(&self.db, self.db.program_file(parent));
        candidates
            .iter()
            .filter_map(|&range| {
                let AnyNodeRef::ExprCall(call) =
                    covering_node(parsed.syntax().into(), range).node()
                else {
                    return None;
                };
                if call.range() != range {
                    return None;
                }
                let literal = self.selected_exec_literal(parent, parsed, &model, call)?;
                let operand = call.arguments.args.first()?.start();
                let module = crate::frontend::exec_literal_text(literal, source)
                    .map_err(str::to_owned)
                    .and_then(|text| self.parse_literal(parent, operand, text, inherited_flags));
                Some((
                    LiteralSite {
                        call: range,
                        operand,
                    },
                    module,
                ))
            })
            .collect()
    }

    fn parse_literal(
        &self,
        parent: File,
        site: TextSize,
        original: &str,
        inherited_flags: u32,
    ) -> Result<ParsedLiteral, String> {
        let text = crate::normalize_source(original, crate::SourceKind::Command)?;
        let input = imre_ast::indentation::prepare(&text);
        super::check_source_complexity(&input.text)
            .map_err(|error| format!("<string>: {error}"))?;
        // Virtual files do not introduce importable modules into the application's search
        // path. Only this parent occurrence at this exact source revision can reuse its File.
        let mut identity = blake3::Hasher::new();
        let parent_path = parent.path(&self.db).as_str().as_bytes();
        identity.update(&(parent_path.len() as u64).to_le_bytes());
        identity.update(parent_path);
        identity.update(&u32::from(site).to_le_bytes());
        identity.update(text.as_bytes());
        let key = format!("imre/exec/{}.py", identity.finalize().to_hex());
        let file = self.virtual_source(&key, &input.text);
        let (parsed, _) =
            self.checked_parse(file, "<string>", &text, &input, crate::SourceKind::Command)?;
        Ok(ParsedLiteral::new(
            file,
            text,
            original,
            parsed,
            inherited_flags,
        ))
    }

    /// Check the resolved nested operations in their originating project database.
    pub(crate) fn check_literal(&self, source: &ParsedLiteral) -> Result<(), String> {
        let path = Path::new("<string>");
        let file = source.file;
        let parsed = &source.parsed;
        let text = source.text.as_str();
        let body = &parsed.syntax().body;
        if let Some(issue) = crate::dynamic::ModuleSyntaxError::in_body(text, body) {
            return Err(Diagnostic::at(text, &issue.range, issue.message)
                .with_path(path)
                .to_string());
        }
        if let Some(error) = self.semantic_syntax_error(file) {
            return Err(
                Diagnostic::at(text, &error.range, format!("SyntaxError: {error}"))
                    .with_path(path)
                    .to_string(),
            );
        }
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        let mut visitor = self.execution_visitor(path, parsed, &model);
        visitor.visit_body(body);
        visitor
            .first
            .map_or(Ok(()), |error| Err(error.report.to_string()))
    }

    fn execution_visitor<'ast, 'db>(
        &'db self,
        path: &'ast Path,
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
            open_providers: &[],
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
        let operand = call.arguments.args.first()?.as_call_expr()?;
        // compile() creates passive code for input text. A fixed filename and
        // mode preserve the same missing entry when exec/eval consumes it.
        let input = if is_stdlib_function_type(
            model,
            operand.func.inferred_type(model)?,
            "builtins",
            "compile",
        ) {
            if self.has_custom_builtins(self.files[source.0]) {
                return None;
            }
            let [source, filename, mode] = operand.arguments.args.as_ref() else {
                return None;
            };
            if !operand.arguments.keywords.is_empty()
                || filename
                    .as_string_literal_expr()?
                    .value
                    .to_str()
                    .contains('\0')
                || mode.as_string_literal_expr()?.value.to_str() != "exec"
            {
                return None;
            }
            source.as_call_expr()?
        } else {
            operand
        };
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
        let mut check = self.execution_visitor(source.1, parsed, model);
        check.visit_expr(value);
        check.first.map(|requirement| requirement.report)
    }

    pub(super) fn generator_execution_error<'ast, 'db>(
        &'db self,
        (_, path): (usize, &'ast Path),
        parsed: &'ast ruff_db::parsed::ParsedModuleRef,
        model: &'ast SemanticModel<'db>,
        function: &'ast StmtFunctionDef,
    ) -> Option<ExecutionRequirement> {
        let mut visitor = self.execution_visitor(path, parsed, model);
        visitor.visit_body(&function.body);
        visitor.first
    }

    pub(crate) fn check_execution_escapes<'a>(
        &self,
        sources: impl Iterator<Item = (usize, &'a std::path::Path)> + Clone,
    ) -> Result<(), String> {
        self.check_execution_with_imports(
            sources.map(|(id, path)| (id, path, false)),
            CapturedImports::default(),
        )
    }

    pub(super) fn check_execution_with_imports<'a>(
        &self,
        sources: impl Iterator<Item = (usize, &'a Path, bool)> + Clone,
        mut captures: CapturedImports,
    ) -> Result<(), String> {
        let open_providers: Vec<_> = sources
            .clone()
            .filter_map(|(id, _, open)| open.then_some(self.files[id]))
            .collect();
        for (index, path, _) in sources {
            let file = self.files[index];
            if path.extension().is_some_and(|extension| extension == "pyi") {
                continue;
            }
            let source = self.db.program_file(file);
            let parsed = self.parsed(file);
            let model = SemanticModel::new(&self.db, source);
            let mut visitor = self.execution_visitor(path, &parsed, &model);
            visitor.open_providers = &open_providers;
            let mut body = parsed.suite().as_slice();
            if let Some(imports) = captures.remove(&index) {
                if let Some(stop) = imports.stop {
                    body = &body[..body.partition_point(|stmt| stmt.start() < stop)];
                }
                visitor.captured = imports.functions;
            }
            if !visitor.captured.is_empty() {
                visitor.unreachable =
                    ty_python_semantic::types::ide_support::unreachable_ranges(&self.db, source)
                        .iter()
                        .map(|unreachable| unreachable.range)
                        .collect();
            }
            visitor.visit_body(body);
            if let Some(error) = visitor.first {
                return Err(error.report.to_string());
            }
        }
        Ok(())
    }
}
