//! Resolve first-class execution operations using the declarations of their operands.

use super::{Session, is_stdlib_function_type};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{Expr, ExprCall, Stmt, StmtFunctionDef};
use ruff_text_size::{Ranged, TextRange};
use ty_python_core::definition::{DefinitionKind, FunctionDefinitionKind};
use ty_python_semantic::types::{Type, TypeDefinition};
use ty_python_semantic::{Db, HasType, SemanticModel};

#[derive(Clone, Copy)]
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

struct Escape<'a, 'db> {
    model: &'a SemanticModel<'db>,
    parsed: &'a ruff_db::parsed::ParsedModuleRef,
    entered: std::collections::HashSet<TextRange>,
    session: &'db Session,
    module: usize,
    first: Option<(TextRange, String)>,
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
    fn visit_stmt(&mut self, statement: &'a Stmt) {
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
            if let Expr::Attribute(attribute) = expression
                && text_operand(self.model, expression)
                && ExecutionIntrinsic::possible(self.model, &attribute.value).is_some()
            {
                // Reading string metadata of a builtin does not invoke it.
                walk_expr(self, &attribute.value);
                return;
            }
            if let Some(intrinsic) = ExecutionIntrinsic::possible(self.model, expression) {
                self.first = Some((expression.range(), intrinsic.escape_message()));
                return;
            }
            if !matches!(expression, Expr::Lambda(_)) {
                walk_expr(self, expression);
            }
            return;
        };
        let intrinsic = ExecutionIntrinsic::possible(self.model, &call.func);
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
            && let Some(intrinsic) = intrinsic
            && let Some(operand) = call.arguments.args.first()
            && !self.has_executable_entry(operand)
        {
            self.first = Some((
                operand.range(),
                format!(
                    "this call may select builtins.{} without a linked body for its source. Define the handlers as Python functions and select one by name, or declare the finite modules with --include-module.",
                    intrinsic.name()
                ),
            ));
            return;
        }
        // Enter only a reached ordinary definition in this checked source.
        // Calls producing generators and coroutines return a suspended object.
        if self.first.is_none()
            && let Some(node) = self.local_function(&call.func)
            && !node.is_async
        {
            let index = ty_python_core::semantic_index(self.model.db(), self.model.program_file());
            if index
                .try_node_scope(ty_python_core::scope::NodeWithScopeRef::Function(node))
                .is_some_and(|scope| !scope.is_generator_function(index))
                && self.entered.insert(node.range())
            {
                self.visit_body(&node.body);
            }
        }
    }
}

impl Session {
    pub(crate) fn check_execution_escapes<'a>(
        &self,
        sources: impl Iterator<Item = (usize, &'a std::path::Path)>,
    ) -> Result<(), String> {
        for (index, path) in sources {
            let file = self.files[index];
            if path.extension().is_some_and(|extension| extension == "pyi") {
                continue;
            }
            let source = self.db.program_file(file);
            let parsed = self.parsed(file);
            let model = SemanticModel::new(&self.db, source);
            let mut visitor = Escape {
                model: &model,
                parsed: &parsed,
                entered: Default::default(),
                session: self,
                module: index,
                first: None,
            };
            visitor.visit_body(&parsed.syntax().body);
            if let Some((range, reason)) = visitor.first {
                let text = ruff_db::source::source_text(&self.db, file);
                return Err(
                    crate::frontend::phase::Diagnostic::at(text.as_str(), &range, reason)
                        .with_path(path)
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}
