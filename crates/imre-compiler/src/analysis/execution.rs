//! Resolve first-class execution operations using the declarations of their operands.

use super::{Session, is_stdlib_function_type};
use crate::dynamic::literal::{LiteralCandidate, LiteralSite, ParsedLiteral};
use crate::frontend::phase::Diagnostic;
use ruff_db::files::File;
use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprCall, Stmt, StmtFunctionDef};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::{collections::HashMap, path::Path};
use ty_python_core::definition::{DefinitionKind, FunctionDefinitionKind, TargetKind};
use ty_python_core::unpack::UnpackPosition;
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{Db, HasType, SemanticModel};

/// Follow the sole assignments that reach this use of the operand.
pub(super) fn assigned_value<'a>(
    db: &dyn Db,
    file: File,
    parsed: &'a ParsedModuleRef,
    mut value: &'a Expr,
    follow: impl Fn(&Expr) -> bool,
) -> Option<&'a Expr> {
    let mut seen = std::collections::HashSet::new();
    while follow(value)
        && let Some(def) = super::shipped_exports::binding_at_use(db, file, value)
        && let DefinitionKind::Assignment(assignment) = def.kind(db)
    {
        if !seen.insert(def) || assignment.unpack().is_some() {
            return None;
        }
        value = assignment.value(parsed);
    }
    Some(value)
}

/// Follow only assignments whose values occur before the code operand they supply.
fn prior_code_value<'a>(
    db: &dyn Db,
    file: File,
    parsed: &'a ParsedModuleRef,
    mut value: &'a Expr,
) -> Option<&'a Expr> {
    let mut seen = std::collections::HashSet::new();
    while value.is_name_expr() {
        let definition = super::shipped_exports::binding_at_use(db, file, value)?;
        let DefinitionKind::Assignment(assignment) = definition.kind(db) else {
            return None;
        };
        let assigned = assignment.value(parsed);
        if assignment.unpack().is_some()
            || assigned.start() >= value.start()
            || !seen.insert(definition)
        {
            return None;
        }
        value = assigned;
    }
    Some(value)
}

/// Admit only the positional source and valid, non-duplicated eval namespaces.
pub(crate) fn valid_eval_arguments(call: &ExprCall) -> bool {
    let args = &call.arguments.args;
    if args.is_empty() || args.len() > 3 || args.iter().any(Expr::is_starred_expr) {
        return false;
    }
    let mut globals = args.len() >= 2;
    let mut locals = args.len() >= 3;
    for keyword in &call.arguments.keywords {
        let present = match keyword.arg.as_ref().map(|name| name.as_str()) {
            Some("globals") => &mut globals,
            Some("locals") => &mut locals,
            _ => return false,
        };
        if std::mem::replace(present, true) {
            return false;
        }
    }
    true
}

/// Extract the bounded integer choices from a tuple of `(count, expected)` rows.
pub(crate) fn finite_first_integer_choices(iterable: &Expr) -> Option<Vec<usize>> {
    let rows = iterable.as_tuple_expr()?;
    if rows.elts.is_empty() || rows.elts.len() > 32 {
        return None;
    }
    rows.elts
        .iter()
        .map(|row| {
            let literal = row
                .as_tuple_expr()?
                .elts
                .first()?
                .as_number_literal_expr()?;
            let ruff_python_ast::Number::Int(value) = &literal.value else {
                return None;
            };
            usize::try_from(value.as_i64()?).ok()
        })
        .collect()
}

/// Recognize `prefix + repeated * count + suffix` without evaluating Python code.
pub(crate) fn finite_eval_count_operand(expression: &Expr) -> Option<&Expr> {
    let outer = expression.as_bin_op_expr()?;
    let left = outer.left.as_bin_op_expr()?;
    let repeated = left.right.as_bin_op_expr()?;
    (outer.op == ruff_python_ast::Operator::Add
        && left.op == ruff_python_ast::Operator::Add
        && repeated.op == ruff_python_ast::Operator::Mult
        && left.left.is_string_literal_expr()
        && repeated.left.is_string_literal_expr()
        && repeated.right.is_name_expr()
        && outer.right.is_string_literal_expr())
    .then_some(&repeated.right)
}

/// Build each finite expression source while enforcing the ordinary source-size bound.
pub(crate) fn finite_eval_sources(expression: &Expr, counts: &[usize]) -> Option<Vec<String>> {
    finite_eval_count_operand(expression)?;
    let outer = expression.as_bin_op_expr()?;
    let left = outer.left.as_bin_op_expr()?;
    let repeated = left.right.as_bin_op_expr()?;
    let prefix = left.left.as_string_literal_expr()?.value.to_str();
    let item = repeated.left.as_string_literal_expr()?.value.to_str();
    let suffix = outer.right.as_string_literal_expr()?.value.to_str();
    counts
        .iter()
        .map(|&count| {
            let size = prefix
                .len()
                .checked_add(item.len().checked_mul(count)?)?
                .checked_add(suffix.len())?;
            if size > 1 << 20 {
                return None;
            }
            let mut source = String::new();
            source.try_reserve_exact(size).ok()?;
            source.push_str(prefix);
            if !item.is_empty() {
                for _ in 0..count {
                    source.push_str(item);
                }
            }
            source.push_str(suffix);
            Some(source)
        })
        .collect()
}

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
        super::select_stdlib_function(model, ty, "builtins", |function| {
            [Self::Exec, Self::Eval]
                .into_iter()
                .find(|kind| kind.name() == function.name.as_str())
        })
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
    linked_exec: Option<&'a std::collections::HashSet<TextRange>>,
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
        assigned_value(
            self.model.db(),
            self.model.file(),
            self.parsed,
            value,
            |_| true,
        )
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
            .or_else(|| super::sole_global_name(self.model, name))?;
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
        self.visit_operand(expression, true);
    }
}

impl<'a> Escape<'a, '_> {
    fn visit_operand(&mut self, expression: &'a Expr, check_escape: bool) {
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
            if check_escape
                && let Some(intrinsic) = ExecutionIntrinsic::possible(self.model, expression)
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
            // The target was just checked. Visit its reads and children in the live scope.
            self.visit_operand(&call.func, false);
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
            && !(source_callable == SourceCallable::Builtin(ExecutionIntrinsic::Eval)
                && call
                    .func
                    .as_name_expr()
                    .is_some_and(|name| name.id == "eval")
                && (operand.is_string_literal_expr()
                    || self
                        .session
                        .selected_finite_eval_sources(
                            self.model.file(),
                            self.parsed,
                            self.model,
                            call,
                        )
                        .is_some()
                    || self
                        .session
                        .selected_compiled_eval_source(
                            self.model.file(),
                            self.parsed,
                            self.model,
                            call,
                            ruff_db::source::source_text(self.model.db(), self.model.file())
                                .as_str(),
                        )
                        .is_some()))
            && !(source_callable == SourceCallable::Builtin(ExecutionIntrinsic::Exec)
                && (crate::frontend::exec_source_operand(call)
                    .is_some_and(Expr::is_string_literal_expr)
                    || self
                        .session
                        .selected_exec_literal(self.model.file(), self.parsed, self.model, call)
                        .is_some()
                    || self
                        .session
                        .selected_compiled_exec_source(
                            self.model.file(),
                            self.parsed,
                            self.model,
                            call,
                            ruff_db::source::source_text(self.model.db(), self.model.file())
                                .as_str(),
                        )
                        .is_some()
                    || self
                        .session
                        .selected_finite_compiled_exec_sources(
                            self.model.file(),
                            self.parsed,
                            self.model,
                            call,
                        )
                        .is_some()
                    || self
                        .session
                        .selected_compiled_single_source(
                            self.model.file(),
                            self.parsed,
                            self.model,
                            call,
                            ruff_db::source::source_text(self.model.db(), self.model.file())
                                .as_str(),
                        )
                        .is_some()
                    || self.session.finite_repeated_keyword_exec(
                        self.model.file(),
                        self.parsed,
                        self.model,
                        call,
                        ruff_db::source::source_text(self.model.db(), self.model.file()).as_str(),
                    )
                    || self
                        .session
                        .finite_concat_exec_sources(
                            self.model.file(),
                            self.parsed,
                            self.model,
                            call,
                            ruff_db::source::source_text(self.model.db(), self.model.file())
                                .as_str(),
                        )
                        .is_some()
                    || self
                        .session
                        .finite_exec_literals(self.model.file(), self.parsed, self.model, call)
                        .is_some()
                    || self
                        .linked_exec
                        .is_some_and(|sites| sites.contains(&call.range()))))
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

/// Resolve a source operand in the active module namespace.
pub(super) fn selected_module_exec_literal<'a>(
    model: &SemanticModel<'_>,
    file: File,
    parsed: &'a ParsedModuleRef,
    call: &'a ExprCall,
) -> Option<&'a ruff_python_ast::ExprStringLiteral> {
    let operand = crate::frontend::exec_source_operand(call)?;
    if ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec) {
        return None;
    }
    assigned_value(model.db(), file, parsed, operand, Expr::is_name_expr)?.as_string_literal_expr()
}

/// Fold a bounded, side-effect-free source expression without executing Python.
///
/// This recognizes shapes such as `'x = ' + '1' * 3`, while deliberately
/// refusing arbitrary values, deep trees, and results larger than one MiB.
pub(crate) fn bounded_source_string(expression: &Expr, outer: &str) -> Option<String> {
    const MAX_SOURCE: usize = 1 << 20;
    fn fold(expression: &Expr, outer: &str, depth: usize) -> Option<String> {
        if depth == 0 {
            return None;
        }
        match expression {
            Expr::StringLiteral(literal) => {
                let text = crate::frontend::exec_literal_text(literal, outer).ok()?;
                (text.len() <= MAX_SOURCE).then(|| text.to_owned())
            }
            Expr::BinOp(binary) if binary.op == ruff_python_ast::Operator::Add => {
                let mut left = fold(&binary.left, outer, depth - 1)?;
                let right = fold(&binary.right, outer, depth - 1)?;
                if left.len().checked_add(right.len())? > MAX_SOURCE {
                    return None;
                }
                left.try_reserve_exact(right.len()).ok()?;
                left.push_str(&right);
                Some(left)
            }
            Expr::BinOp(binary) if binary.op == ruff_python_ast::Operator::Mult => {
                let text = fold(&binary.left, outer, depth - 1)?;
                let ruff_python_ast::Number::Int(count) =
                    &binary.right.as_number_literal_expr()?.value
                else {
                    return None;
                };
                let count = usize::try_from(count.as_i64()?).ok()?;
                if text.is_empty() || count == 0 {
                    return Some(String::new());
                }
                let size = text.len().checked_mul(count)?;
                if size > MAX_SOURCE {
                    return None;
                }
                let mut output = String::new();
                output.try_reserve_exact(size).ok()?;
                for _ in 0..count {
                    output.push_str(&text);
                }
                Some(output)
            }
            _ => None,
        }
    }
    fold(expression, outer, 32)
}

/// Give resolution and lowering the same checked virtual file for this decoded operand.
pub(super) fn checked_literal_source<'a>(
    db: &dyn super::LiteralSourceDb,
    parent: File,
    site: TextSize,
    original: &'a str,
) -> Result<(File, std::borrow::Cow<'a, str>, ParsedModuleRef), String> {
    let text = crate::normalize_source(original, crate::SourceKind::Command)?;
    let input = imre_ast::indentation::prepare(&text);
    super::check_source_complexity(&input.text).map_err(|error| format!("<string>: {error}"))?;
    // Virtual files do not introduce importable modules into the application's search
    // path. Only this parent occurrence at this exact source revision can reuse its File.
    let mut identity = blake3::Hasher::new();
    let parent_path = parent.path(db).as_str().as_bytes();
    identity.update(&(parent_path.len() as u64).to_le_bytes());
    identity.update(parent_path);
    identity.update(&u32::from(site).to_le_bytes());
    identity.update(text.as_bytes());
    let key = format!("imre/exec/{}.py", identity.finalize().to_hex());
    let file = db.virtual_source(&key, &input.text);
    let (parsed, _) = Session::checked_parse(
        ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db),
        "<string>",
        &text,
        &input,
        crate::SourceKind::Command,
    )?;
    Ok((file, text, parsed))
}

/// Match the normalized exec input checked by runtime builtin `compile` before parsing.
fn compiled_exec_exceeds_complexity(source: &str) -> bool {
    let Ok(text) = crate::normalize_source(source, crate::SourceKind::Command) else {
        return false;
    };
    let input = imre_ast::indentation::prepare(&text);
    imre_ast::source_complexity::check_runtime_compile(&input.text).is_err()
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

    fn selected_exec_literal<'a>(
        &self,
        file: File,
        parsed: &'a ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &'a ExprCall,
    ) -> Option<&'a ruff_python_ast::ExprStringLiteral> {
        if self.reads_custom_builtins(file, parsed, &call.func) {
            return None;
        }
        selected_module_exec_literal(model, file, parsed, call)
    }

    /// Fold fixed bytes or bytearray input to `compile` without executing Python.
    ///
    /// ```python
    /// padding = 113 * 8 - len(b"#\nVALUE = 7\n")
    /// source = bytearray(b"#" + b"x" * padding + b"\nVALUE = 7\n")
    /// exec(compile(source, "settings.py", "exec"), {})
    /// ```
    ///
    /// Only sole reaching assignments, literal arithmetic, and the builtin
    /// `len` and `bytearray` are recognized. The result remains bounded by
    /// the same one-MiB limit as string source.
    fn bounded_compiled_bytes<'a>(
        &self,
        file: File,
        parsed: &'a ParsedModuleRef,
        model: &SemanticModel<'_>,
        expression: &'a Expr,
    ) -> Option<String> {
        const MAX_SOURCE: usize = 1 << 20;

        fn bytes<'a>(
            session: &Session,
            file: File,
            parsed: &'a ParsedModuleRef,
            model: &SemanticModel<'_>,
            expression: &'a Expr,
            depth: usize,
        ) -> Option<Vec<u8>> {
            if depth == 0 {
                return None;
            }
            let expression =
                assigned_value(&session.db, file, parsed, expression, Expr::is_name_expr)?;
            match expression {
                Expr::BytesLiteral(literal) => {
                    if literal.value.len() > MAX_SOURCE {
                        return None;
                    }
                    let mut result = Vec::new();
                    result.try_reserve_exact(literal.value.len()).ok()?;
                    result.extend(literal.value.bytes());
                    Some(result)
                }
                Expr::BinOp(binary) if binary.op == ruff_python_ast::Operator::Add => {
                    let mut left = bytes(session, file, parsed, model, &binary.left, depth - 1)?;
                    let right = bytes(session, file, parsed, model, &binary.right, depth - 1)?;
                    if left.len().checked_add(right.len())? > MAX_SOURCE {
                        return None;
                    }
                    left.try_reserve_exact(right.len()).ok()?;
                    left.extend_from_slice(&right);
                    Some(left)
                }
                Expr::BinOp(binary) if binary.op == ruff_python_ast::Operator::Mult => {
                    let value = bytes(session, file, parsed, model, &binary.left, depth - 1)?;
                    let count = usize::try_from(integer(
                        session,
                        file,
                        parsed,
                        model,
                        &binary.right,
                        depth - 1,
                    )?)
                    .ok()?;
                    if value.is_empty() || count == 0 {
                        return Some(Vec::new());
                    }
                    let size = value.len().checked_mul(count)?;
                    if size > MAX_SOURCE {
                        return None;
                    }
                    let mut result = Vec::new();
                    result.try_reserve_exact(size).ok()?;
                    for _ in 0..count {
                        result.extend_from_slice(&value);
                    }
                    Some(result)
                }
                _ => None,
            }
        }

        fn integer<'a>(
            session: &Session,
            file: File,
            parsed: &'a ParsedModuleRef,
            model: &SemanticModel<'_>,
            expression: &'a Expr,
            depth: usize,
        ) -> Option<i64> {
            if depth == 0 {
                return None;
            }
            let expression =
                assigned_value(&session.db, file, parsed, expression, Expr::is_name_expr)?;
            match expression {
                Expr::NumberLiteral(literal) => match &literal.value {
                    ruff_python_ast::Number::Int(value) => value.as_i64(),
                    _ => None,
                },
                Expr::BinOp(binary) => {
                    let left = integer(session, file, parsed, model, &binary.left, depth - 1)?;
                    let right = integer(session, file, parsed, model, &binary.right, depth - 1)?;
                    match binary.op {
                        ruff_python_ast::Operator::Add => left.checked_add(right),
                        ruff_python_ast::Operator::Sub => left.checked_sub(right),
                        ruff_python_ast::Operator::Mult => left.checked_mul(right),
                        _ => None,
                    }
                }
                Expr::Call(call)
                    if !session.reads_custom_builtins(file, parsed, &call.func)
                        && super::resolves_to_stdlib_function(
                            model, &call.func, "builtins", "len",
                        )
                        && call.arguments.keywords.is_empty() =>
                {
                    let [value] = call.arguments.args.as_ref() else {
                        return None;
                    };
                    i64::try_from(bytes(session, file, parsed, model, value, depth - 1)?.len()).ok()
                }
                _ => None,
            }
        }

        let selected = assigned_value(&self.db, file, parsed, expression, Expr::is_name_expr)?;
        let source = if let Expr::Call(call) = selected {
            let env = model.program_environment();
            let ty @ Type::ClassLiteral(_) = call.func.inferred_type(model)? else {
                return None;
            };
            let canonical =
                ty_python_semantic::types::KnownClass::Bytearray.to_instance(&self.db, &env);
            if self.reads_custom_builtins(file, parsed, &call.func)
                || Some(ty.definition(&self.db, &env)?) != canonical.definition(&self.db, &env)
                || !call.arguments.keywords.is_empty()
            {
                return None;
            }
            let [source] = call.arguments.args.as_ref() else {
                return None;
            };
            source
        } else {
            selected
        };
        let source = bytes(self, file, parsed, model, source, 32)?;
        imre_ast::source_encoding::decode_compile_source(source, |_, error| error.to_string()).ok()
    }

    /// Recognize a fixed builtin `compile` result consumed by builtin `exec`.
    ///
    /// ```python
    /// code = compile('answer = ' + '3' * 2, '<generated>', 'exec')
    /// exec(code, namespace)
    /// ```
    ///
    /// Only a sole reaching assignment and bounded literal source are linked;
    /// the runtime checks the selected callable and code object's source again.
    fn selected_compiled_exec_source(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
        outer: &str,
    ) -> Option<String> {
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
        {
            return None;
        }
        let operand = crate::frontend::exec_source_operand(call)?;
        let compiled =
            assigned_value(&self.db, file, parsed, operand, Expr::is_name_expr)?.as_call_expr()?;
        if self.reads_custom_builtins(file, parsed, &compiled.func)
            || !super::resolves_to_stdlib_function(model, &compiled.func, "builtins", "compile")
            || !compiled.arguments.keywords.is_empty()
        {
            return None;
        }
        let [source, filename, mode] = compiled.arguments.args.as_ref() else {
            return None;
        };
        if filename
            .as_string_literal_expr()?
            .value
            .to_str()
            .contains('\0')
            || mode.as_string_literal_expr()?.value.to_str() != "exec"
        {
            return None;
        }
        let selected = assigned_value(&self.db, file, parsed, source, Expr::is_name_expr)?;
        bounded_source_string(selected, outer)
            .or_else(|| self.bounded_compiled_bytes(file, parsed, model, selected))
    }

    /// Link `exec(compile(source, fixed_filename, "exec"))` when one finite
    /// loop binding supplies only bounded bytes or bytearray sources. Every
    /// choice is decoded using Python's BOM/cookie rules before it is linked;
    /// the runtime guards the immutable decoded compile-time snapshot again.
    fn selected_finite_compiled_exec_sources(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
    ) -> Option<Vec<String>> {
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
        {
            return None;
        }
        let compiled = crate::frontend::exec_source_operand(call)?.as_call_expr()?;
        if !compiled
            .func
            .as_name_expr()
            .is_some_and(|name| name.id == "compile")
            || self.reads_custom_builtins(file, parsed, &compiled.func)
            || !super::resolves_to_stdlib_function(model, &compiled.func, "builtins", "compile")
            || !compiled.arguments.keywords.is_empty()
        {
            return None;
        }
        let [source, filename, mode] = compiled.arguments.args.as_ref() else {
            return None;
        };
        if filename
            .as_string_literal_expr()?
            .value
            .to_str()
            .contains('\0')
            || mode.as_string_literal_expr()?.value.to_str() != "exec"
        {
            return None;
        }
        let name = source.as_name_expr()?;
        let definition = super::shipped_exports::binding_at_use(&self.db, file, source)?;
        let DefinitionKind::For(binding) = definition.kind(&self.db) else {
            return None;
        };
        if binding.is_async()
            || !matches!(binding.target_kind(), TargetKind::Single)
            || binding.target(parsed).as_name_expr()?.id != name.id
        {
            return None;
        }
        let choices = match binding.iterable(parsed) {
            Expr::Tuple(values) => &values.elts,
            Expr::List(values) => &values.elts,
            _ => return None,
        };
        if choices.is_empty() || choices.len() > 32 {
            return None;
        }
        choices
            .iter()
            .map(|choice| self.bounded_compiled_bytes(file, parsed, model, choice))
            .collect()
    }

    /// Link a fixed `single`-mode annotated assignment consumed by builtin `exec`.
    /// Expression statements remain excluded because interactive mode must display them.
    fn selected_compiled_single_source(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
        outer: &str,
    ) -> Option<String> {
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
        {
            return None;
        }
        let compiled = crate::frontend::exec_source_operand(call)?.as_call_expr()?;
        if !compiled
            .func
            .as_name_expr()
            .is_some_and(|name| name.id == "compile")
            || self.reads_custom_builtins(file, parsed, &compiled.func)
            || !super::resolves_to_stdlib_function(model, &compiled.func, "builtins", "compile")
            || !compiled.arguments.keywords.is_empty()
        {
            return None;
        }
        let [source, filename, mode] = compiled.arguments.args.as_ref() else {
            return None;
        };
        if filename
            .as_string_literal_expr()?
            .value
            .to_str()
            .contains('\0')
            || mode.as_string_literal_expr()?.value.to_str() != "single"
        {
            return None;
        }
        let source = bounded_source_string(source, outer)?;
        let checked = ruff_python_parser::parse_module(&source).ok()?;
        if !matches!(checked.syntax().body.as_slice(), [Stmt::AnnAssign(_)])
            || imre_ast::check_single_input(&checked.syntax().body, checked.tokens()).is_err()
        {
            return None;
        }
        Some(source)
    }

    /// Recognize a fixed builtin `compile(..., "eval")` result consumed by builtin `eval`.
    /// A sole reaching code assignment and bounded source are required; runtime
    /// dispatch verifies the live eval callable, parsed code source, and flags.
    fn selected_compiled_eval_source(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
        outer: &str,
    ) -> Option<String> {
        if !call
            .func
            .as_name_expr()
            .is_some_and(|name| name.id == "eval")
            || !valid_eval_arguments(call)
            || self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Eval)
        {
            return None;
        }
        let operand = call.arguments.args.first()?;
        let compiled = prior_code_value(&self.db, file, parsed, operand)?.as_call_expr()?;
        self.selected_eval_compile_source(file, parsed, model, compiled, outer)
    }

    /// Resolve only builtin `compile` calls with a bounded expression source.
    fn selected_eval_compile_source(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        compiled: &ExprCall,
        outer: &str,
    ) -> Option<String> {
        if !compiled
            .func
            .as_name_expr()
            .is_some_and(|name| name.id == "compile")
            || self.reads_custom_builtins(file, parsed, &compiled.func)
            || !super::resolves_to_stdlib_function(model, &compiled.func, "builtins", "compile")
            || !compiled.arguments.keywords.is_empty()
        {
            return None;
        }
        let [source, filename, mode] = compiled.arguments.args.as_ref() else {
            return None;
        };
        if filename
            .as_string_literal_expr()?
            .value
            .to_str()
            .contains('\0')
            || mode.as_string_literal_expr()?.value.to_str() != "eval"
        {
            return None;
        }
        bounded_source_string(source, outer)
    }

    /// Route a bounded eval-mode compile through registered lowering.
    pub(crate) fn fixed_eval_compile_call(&self, module: usize, call: &ExprCall) -> bool {
        if !call
            .func
            .as_name_expr()
            .is_some_and(|name| name.id == "compile")
        {
            return false;
        }
        let file = self.files[module];
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        self.selected_eval_compile_source(
            file,
            &self.parsed(file),
            &model,
            call,
            ruff_db::source::source_text(&self.db, file).as_str(),
        )
        .is_some()
    }

    /// Permit a finite exec whose every source fails before Python can execute a body.
    ///
    /// ```python
    /// for source in ("f(a=1, a=2)", "f(**{}, a=1, a=2)"):
    ///     exec("mark()\n" + source, namespace)
    /// ```
    ///
    /// The same shared syntax check runs at runtime; a valid alternative keeps
    /// the ordinary missing-body refusal.
    fn finite_repeated_keyword_exec(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
        outer: &str,
    ) -> bool {
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
        {
            return false;
        }
        let Some(binary) =
            crate::frontend::exec_source_operand(call).and_then(Expr::as_bin_op_expr)
        else {
            return false;
        };
        if binary.op != ruff_python_ast::Operator::Add {
            return false;
        }
        let (Some(prefix), Some(name)) = (
            binary.left.as_string_literal_expr(),
            binary.right.as_name_expr(),
        ) else {
            return false;
        };
        let Some(definition) =
            super::shipped_exports::binding_at_use(&self.db, file, &binary.right)
        else {
            return false;
        };
        let DefinitionKind::For(binding) = definition.kind(&self.db) else {
            return false;
        };
        if binding.is_async()
            || binding
                .target(parsed)
                .as_name_expr()
                .is_none_or(|target| target.id != name.id)
        {
            return false;
        }
        let Some(values) = binding.iterable(parsed).as_tuple_expr() else {
            return false;
        };
        if values.elts.is_empty() || values.elts.len() > 32 {
            return false;
        }
        let Ok(prefix) = crate::frontend::exec_literal_text(prefix, outer) else {
            return false;
        };
        values.elts.iter().all(|value| {
            let Some(literal) = value.as_string_literal_expr() else {
                return false;
            };
            let Ok(value) = crate::frontend::exec_literal_text(literal, outer) else {
                return false;
            };
            let Some(size) = prefix
                .len()
                .checked_add(value.len())
                .filter(|size| *size <= 1 << 20)
            else {
                return false;
            };
            let mut source = String::new();
            if source.try_reserve_exact(size).is_err() {
                return false;
            }
            source.push_str(prefix);
            source.push_str(value);
            ruff_python_parser::parse_module(&source)
                .ok()
                .is_some_and(|parsed| imre_ast::repeated_keyword(parsed.syntax().into()).is_some())
        })
    }

    /// Link a fixed string pattern containing one finite loop-bound string. Both
    /// concatenation and a plain f-string field are checked against the live source.
    fn finite_concat_exec_sources(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
        outer: &str,
    ) -> Option<Vec<String>> {
        use ruff_python_ast as ast;
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
            || self.finite_repeated_keyword_exec(file, parsed, model, call, outer)
        {
            return None;
        }
        struct Pattern<'a> {
            name: Option<&'a Expr>,
            prefix: String,
            suffix: String,
        }
        impl<'a> Pattern<'a> {
            fn fixed(&mut self, text: &str) -> Option<()> {
                let accumulated = self
                    .prefix
                    .len()
                    .checked_add(self.suffix.len())?
                    .checked_add(text.len())?;
                if accumulated > 1 << 20 {
                    return None;
                }
                let target = if self.name.is_some() {
                    &mut self.suffix
                } else {
                    &mut self.prefix
                };
                target.try_reserve(text.len()).ok()?;
                target.push_str(text);
                Some(())
            }
            fn field(&mut self, expression: &'a Expr) -> Option<()> {
                if !expression.is_name_expr() || self.name.replace(expression).is_some() {
                    return None;
                }
                Some(())
            }
            fn concat(&mut self, expression: &'a Expr, outer: &str, depth: usize) -> Option<()> {
                if depth == 0 {
                    return None;
                }
                match expression {
                    Expr::BinOp(binary) if binary.op == ast::Operator::Add => {
                        self.concat(&binary.left, outer, depth - 1)?;
                        self.concat(&binary.right, outer, depth - 1)
                    }
                    Expr::StringLiteral(literal) => {
                        self.fixed(crate::frontend::exec_literal_text(literal, outer).ok()?)
                    }
                    Expr::Name(_) => self.field(expression),
                    _ => None,
                }
            }
            fn fstring(&mut self, expression: &'a ast::ExprFString, outer: &str) -> Option<()> {
                let mut parts = expression.value.iter();
                let ast::FStringPartRef::FString(part) = parts.next()? else {
                    return None;
                };
                if parts.next().is_some() {
                    return None;
                }
                for element in &part.elements {
                    match element {
                        ast::InterpolatedStringElement::Literal(literal) => {
                            let spelling = outer
                                .get(usize::from(literal.start())..usize::from(literal.end()))?;
                            // Accept only literal spelling with no escape or brace transformation.
                            if spelling != literal.value.as_ref() || !spelling.is_ascii() {
                                return None;
                            }
                            self.fixed(spelling)?;
                        }
                        ast::InterpolatedStringElement::Interpolation(field) => {
                            if field.conversion != ast::ConversionFlag::None
                                || field.format_spec.is_some()
                                || field.debug_text.is_some()
                            {
                                return None;
                            }
                            self.field(&field.expression)?;
                        }
                    }
                }
                Some(())
            }
        }
        let operand = crate::frontend::exec_source_operand(call)?;
        let mut pattern = Pattern {
            name: None,
            prefix: String::new(),
            suffix: String::new(),
        };
        match operand {
            Expr::BinOp(_) => pattern.concat(operand, outer, 8)?,
            Expr::FString(value) => pattern.fstring(value, outer)?,
            _ => return None,
        }
        let name_operand = pattern.name?;
        let name = name_operand.as_name_expr()?;
        let definition = super::shipped_exports::binding_at_use(&self.db, file, name_operand)?;
        let DefinitionKind::For(binding) = definition.kind(&self.db) else {
            return None;
        };
        if binding.is_async() || binding.target(parsed).as_name_expr()?.id != name.id {
            return None;
        }
        let first = match binding.target_kind() {
            TargetKind::Single => false,
            TargetKind::Sequence(UnpackPosition::First, _) => true,
            _ => return None,
        };
        let values = match binding.iterable(parsed) {
            Expr::Tuple(values) => &values.elts,
            Expr::List(values) => &values.elts,
            _ => return None,
        };
        if values.is_empty() || values.len() > 32 {
            return None;
        }
        values
            .iter()
            .map(|value| {
                let value = if first {
                    value.as_tuple_expr()?.elts.first()?
                } else {
                    value
                };
                let choice =
                    crate::frontend::exec_literal_text(value.as_string_literal_expr()?, outer)
                        .ok()?;
                let size = pattern
                    .prefix
                    .len()
                    .checked_add(choice.len())?
                    .checked_add(pattern.suffix.len())?;
                if size > 1 << 20 {
                    return None;
                }
                let mut source = String::new();
                source.try_reserve_exact(size).ok()?;
                source.push_str(&pattern.prefix);
                source.push_str(choice);
                source.push_str(&pattern.suffix);
                let checked = ruff_python_parser::parse_module(&source).ok()?;
                if imre_ast::repeated_keyword(checked.syntax().into()).is_some() {
                    return None;
                }
                Some(source)
            })
            .collect()
    }

    /// Select a small literal source set from the sole loop binding reaching `exec`.
    ///
    /// ```python
    /// for source, expected in (("f(*42)", "not iterable"), ("f(*missing)", "missing")):
    ///     exec(source, namespace)
    /// ```
    ///
    /// The site table links each alternative independently; the runtime still
    /// checks the live callable and exact source before choosing a body.
    fn finite_exec_literals<'a>(
        &self,
        file: File,
        parsed: &'a ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
    ) -> Option<Vec<&'a ruff_python_ast::ExprStringLiteral>> {
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
        {
            return None;
        }
        let operand = crate::frontend::exec_source_operand(call)?;
        let name = operand.as_name_expr()?;
        let definition = super::shipped_exports::binding_at_use(&self.db, file, operand)?;
        let DefinitionKind::For(binding) = definition.kind(&self.db) else {
            return None;
        };
        if binding.is_async() || binding.target(parsed).as_name_expr()?.id != name.id {
            return None;
        }
        let values = binding.iterable(parsed).as_tuple_expr()?;
        if values.elts.is_empty() || values.elts.len() > 32 {
            return None;
        }
        let first = match binding.target_kind() {
            TargetKind::Single => false,
            TargetKind::Sequence(UnpackPosition::First, _) => true,
            _ => return None,
        };
        values
            .elts
            .iter()
            .map(|value| {
                let value = if first {
                    value.as_tuple_expr()?.elts.first()?
                } else {
                    value
                };
                let literal = value.as_string_literal_expr()?;
                (literal.value.to_str().len() <= 1 << 20).then_some(literal)
            })
            .collect()
    }

    /// Select expression alternatives when one finite loop binding supplies the repeat count.
    /// The emitter links the same bounded strings, and the runtime verifies the live source.
    fn selected_finite_eval_sources(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
    ) -> Option<Vec<String>> {
        let operand = call.arguments.args.first()?;
        let count = finite_eval_count_operand(operand)?;
        if !valid_eval_arguments(call)
            || self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Eval)
        {
            return None;
        }
        let name = count.as_name_expr()?;
        let definition = super::shipped_exports::binding_at_use(&self.db, file, count)?;
        let DefinitionKind::For(binding) = definition.kind(&self.db) else {
            return None;
        };
        if binding.is_async()
            || binding.target(parsed).as_name_expr()?.id != name.id
            || !matches!(
                binding.target_kind(),
                TargetKind::Sequence(UnpackPosition::First, _)
            )
        {
            return None;
        }
        finite_eval_sources(
            operand,
            &finite_first_integer_choices(binding.iterable(parsed))?,
        )
    }

    /// Route a finite computed eval through registered lowering, which owns its alternatives.
    pub(crate) fn fixed_eval_call(&self, module: usize, call: &ExprCall) -> bool {
        let file = self.files[module];
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        self.selected_finite_eval_sources(file, &self.parsed(file), &model, call)
            .is_some()
            || self
                .selected_compiled_eval_source(
                    file,
                    &self.parsed(file),
                    &model,
                    call,
                    ruff_db::source::source_text(&self.db, file).as_str(),
                )
                .is_some()
    }

    /// The launcher can link the initial text of a fixed sys.argv operand. The
    /// runtime still evaluates the operand and checks its exact text at the call.
    fn selected_launcher_exec_source<'a>(
        &self,
        file: File,
        parsed: &ParsedModuleRef,
        model: &SemanticModel<'_>,
        call: &ExprCall,
        arguments: &'a [Option<String>],
    ) -> Option<&'a str> {
        if self.reads_custom_builtins(file, parsed, &call.func)
            || ExecutionIntrinsic::resolve(model, &call.func) != Some(ExecutionIntrinsic::Exec)
        {
            return None;
        }
        let operand = crate::frontend::exec_source_operand(call)?.as_subscript_expr()?;
        let argv = operand.value.as_attribute_expr()?;
        if argv.attr.as_str() != "argv"
            || !matches!(argv.value.inferred_type(model), Some(Type::ModuleLiteral(module))
                if module.module(&self.db).name(&self.db).as_str() == "sys")
        {
            return None;
        }
        let ruff_python_ast::Number::Int(index) = &operand.slice.as_number_literal_expr()?.value
        else {
            return None;
        };
        let index = usize::try_from(index.as_i64()?).ok()?.checked_sub(1)?;
        arguments.get(index)?.as_deref()
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
            || self
                .selected_compiled_exec_source(
                    file,
                    &self.parsed(file),
                    &model,
                    call,
                    ruff_db::source::source_text(&self.db, file).as_str(),
                )
                .is_some()
            || self
                .selected_finite_compiled_exec_sources(file, &self.parsed(file), &model, call)
                .is_some()
            || self
                .selected_compiled_single_source(
                    file,
                    &self.parsed(file),
                    &model,
                    call,
                    ruff_db::source::source_text(&self.db, file).as_str(),
                )
                .is_some()
            || self.finite_repeated_keyword_exec(
                file,
                &self.parsed(file),
                &model,
                call,
                ruff_db::source::source_text(&self.db, file).as_str(),
            )
            || self
                .finite_concat_exec_sources(
                    file,
                    &self.parsed(file),
                    &model,
                    call,
                    ruff_db::source::source_text(&self.db, file).as_str(),
                )
                .is_some()
            || self
                .finite_exec_literals(file, &self.parsed(file), &model, call)
                .is_some()
    }

    /// Resolve fixed exec operands and file-write sources after the parent's imports are registered.
    /// Each returned parse supplies the next level's imports and its eventual checked module.
    pub(crate) fn select_literal_level(
        &self,
        parent: File,
        parsed: &ParsedModuleRef,
        source: &str,
        inherited_flags: u32,
        candidates: &[LiteralCandidate],
        (launcher_arguments, site_lines): (&[Option<String>], &[String]),
    ) -> Vec<(LiteralSite, Result<ParsedLiteral, String>)> {
        use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
        if candidates.is_empty() {
            return Vec::new();
        }
        let model = SemanticModel::new(&self.db, self.db.program_file(parent));
        let mut sites = Vec::new();
        for candidate in candidates {
            let range = candidate.call;
            let AnyNodeRef::ExprCall(call) = covering_node(parsed.syntax().into(), range).node()
            else {
                continue;
            };
            if call.range() != range {
                continue;
            }
            let Some(argument) = (if candidate.file.is_some() {
                call.arguments.args.first()
            } else {
                crate::frontend::exec_source_operand(call)
            }) else {
                continue;
            };
            let operand = argument.start();
            let mut add = |original: Result<&str, String>, optional, mode| {
                let module = original.and_then(|original| {
                    // A text write emits UTF-8 bytes, but a loader decodes those bytes
                    // according to any PEP 263 cookie before compiling the module.
                    let decoded;
                    let input = if candidate.file.is_some() {
                        decoded = imre_ast::source_encoding::decode_compile_source(
                            original.as_bytes(),
                            |_, error| error.to_string(),
                        )
                        .map_err(|error| format!("{error:?}"))?;
                        decoded.as_str()
                    } else {
                        original
                    };
                    let (file, text, parsed) =
                        checked_literal_source(&self.db, parent, operand, input)?;
                    Ok(ParsedLiteral::new(
                        file,
                        text,
                        original,
                        parsed,
                        if candidate.file.is_some() {
                            0
                        } else {
                            inherited_flags
                        },
                        mode,
                    ))
                });
                sites.push((
                    LiteralSite {
                        call: range,
                        operand,
                        optional,
                        file: candidate.file.clone(),
                    },
                    module,
                ));
            };
            if candidate.file.is_some() {
                if let Some(literal) = argument.as_string_literal_expr() {
                    add(
                        crate::frontend::exec_literal_text(literal, source).map_err(str::to_owned),
                        true,
                        crate::llvm::registered::sites::CompiledSourceMode::Exec,
                    );
                }
            } else if let Some(literal) = self.selected_exec_literal(parent, parsed, &model, call) {
                add(
                    crate::frontend::exec_literal_text(literal, source).map_err(str::to_owned),
                    false,
                    crate::llvm::registered::sites::CompiledSourceMode::Exec,
                );
            } else if let Some(original) =
                self.selected_compiled_exec_source(parent, parsed, &model, call, source)
            {
                // The runtime compile call is guaranteed to raise RecursionError on this
                // exact normalized source, so its consumer cannot execute a body.
                if !compiled_exec_exceeds_complexity(&original) {
                    add(
                        Ok(&original),
                        false,
                        crate::llvm::registered::sites::CompiledSourceMode::Exec,
                    );
                }
            } else if let Some(originals) =
                self.selected_finite_compiled_exec_sources(parent, parsed, &model, call)
            {
                for original in originals {
                    add(
                        Ok(&original),
                        false,
                        crate::llvm::registered::sites::CompiledSourceMode::Exec,
                    );
                }
            } else if let Some(original) =
                self.selected_compiled_single_source(parent, parsed, &model, call, source)
            {
                add(
                    Ok(&original),
                    false,
                    crate::llvm::registered::sites::CompiledSourceMode::Single,
                );
            } else if let Some(literals) = self.finite_exec_literals(parent, parsed, &model, call) {
                for literal in literals {
                    add(
                        crate::frontend::exec_literal_text(literal, source).map_err(str::to_owned),
                        false,
                        crate::llvm::registered::sites::CompiledSourceMode::Exec,
                    );
                }
            } else if let Some(originals) =
                self.finite_concat_exec_sources(parent, parsed, &model, call, source)
            {
                for original in originals {
                    add(
                        Ok(&original),
                        false,
                        crate::llvm::registered::sites::CompiledSourceMode::Exec,
                    );
                }
            } else if let Some(original) =
                self.selected_launcher_exec_source(parent, parsed, &model, call, launcher_arguments)
            {
                add(
                    Ok(original),
                    false,
                    crate::llvm::registered::sites::CompiledSourceMode::Exec,
                );
            } else if !site_lines.is_empty()
                && argument
                    .as_name_expr()
                    .is_some_and(|name| name.id == "line")
                && !self.reads_custom_builtins(parent, parsed, &call.func)
                && ExecutionIntrinsic::resolve(&model, &call.func) == Some(ExecutionIntrinsic::Exec)
            {
                for line in site_lines {
                    add(
                        Ok(line),
                        true,
                        crate::llvm::registered::sites::CompiledSourceMode::Exec,
                    );
                }
            }
        }
        sites
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
            linked_exec: None,
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

    /// A declaration-only receiver has no fixed call targets. Check its methods before
    /// requesting object registration on the strength of this class alone.
    pub(super) fn class_methods_have_linked_execution(
        &self,
        definition: ty_python_core::definition::Definition<'_>,
        path: &Path,
    ) -> bool {
        let file = definition.file(&self.db);
        let parsed = self.parsed(file);
        let Some(class) = definition.kind(&self.db).as_class() else {
            return false;
        };
        let model = SemanticModel::new(&self.db, self.db.program_file(file));
        class
            .node(&parsed)
            .body
            .iter()
            .filter_map(Stmt::as_function_def_stmt)
            .all(|method| {
                let mut visitor = self.execution_visitor(path, &parsed, &model);
                visitor.visit_body(&method.body);
                visitor.first.is_none()
            })
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
            sources.map(|(id, path)| (id, path, false, None)),
            CapturedImports::default(),
        )
    }

    pub(super) fn check_execution_with_imports<'a>(
        &self,
        sources: impl Iterator<
            Item = (
                usize,
                &'a Path,
                bool,
                Option<&'a std::collections::HashSet<TextRange>>,
            ),
        > + Clone,
        mut captures: CapturedImports,
    ) -> Result<(), String> {
        let open_providers: Vec<_> = sources
            .clone()
            .filter_map(|(id, _, open, _)| open.then_some(self.files[id]))
            .collect();
        for (index, path, _, linked_exec) in sources {
            let file = self.files[index];
            if path.extension().is_some_and(|extension| extension == "pyi") {
                continue;
            }
            let source = self.db.program_file(file);
            let parsed = self.parsed(file);
            let model = SemanticModel::new(&self.db, source);
            let mut visitor = self.execution_visitor(path, &parsed, &model);
            visitor.open_providers = &open_providers;
            visitor.linked_exec = linked_exec;
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
