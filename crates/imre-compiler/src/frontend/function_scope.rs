//! Lexical bindings and code-object metadata for prepared Python functions.

mod annotations;
pub(crate) use annotations::ClassAnnotations;
pub(super) use annotations::ParameterReads;
mod callable;
pub(crate) use callable::{CallableMetadata, NamedFunction};
mod class_metadata;
pub(super) use class_metadata::{display_bindings, expression_reads, parameter_reads};
pub(crate) mod classes;
mod type_parameters;

pub(crate) use type_parameters::{
    TypeParameterScope, boxed_function_type_params, check_nonlocals, type_parameter_scope,
};

use imre_ast::comprehension::Plan;
use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_text_size::{Ranged, TextRange, TextSize};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

#[derive(Default)]
struct ClosureNames {
    locals: indexmap::IndexSet<String>,
    reads: BTreeSet<String>,
    globals: BTreeSet<String>,
    nonlocals: BTreeSet<String>,
    include_nested_reads: bool,
    cell_reads: BTreeSet<String>,
    string_annotations: bool,
    incomplete: bool,
    locals_incomplete: bool,
}

pub(crate) struct ClassBindings {
    pub(crate) locals: indexmap::IndexSet<String>,
    pub(crate) globals: BTreeSet<String>,
}

impl ClosureNames {
    fn class_declarations(body: &[ast::Stmt]) -> Self {
        let mut names = Self::default();
        names.visit_body(body);
        for name in names.globals.iter().chain(&names.nonlocals) {
            // Class namespace lookups only need membership, so removal can reorder names.
            names.locals.swap_remove(name);
        }
        names
    }

    fn finish_scope(&mut self) {
        for name in self.globals.iter().chain(&self.nonlocals) {
            self.locals.shift_remove(name);
        }
        self.reads
            .retain(|name| !self.locals.contains(name) && !self.globals.contains(name));
        self.reads.extend(self.nonlocals.iter().cloned());
    }

    /// Propagate a completed child's free names, including its lazy annotation scope.
    fn read_child_function(&mut self, f: &ast::StmtFunctionDef, child: ClosureNames) {
        self.incomplete |= child.incomplete;
        let mut reads = child.reads;
        let mut annotations = if self.string_annotations {
            Vec::new()
        } else {
            crate::frontend::parameter_annotations(&f.parameters, f.returns.as_deref())
        };
        if let Some(params) = &f.type_params {
            annotations.extend(
                params
                    .iter()
                    .flat_map(imre_ast::TypeParameterField::expressions)
                    .map(|(_, expr)| ("", expr)),
            );
        }
        reads.extend(class_metadata::annotation_reads(&annotations, None, false));
        if let Some(params) = &f.type_params {
            for param in params.iter() {
                reads.remove(param.name().as_str());
            }
        }
        self.cell_reads.extend(reads.iter().cloned());
        self.reads.extend(reads);
    }

    fn child_reads(
        &mut self,
        parameters: Option<&ast::Parameters>,
        visit: impl FnOnce(&mut Self),
    ) -> std::collections::BTreeSet<String> {
        let mut child = Self {
            include_nested_reads: true,
            string_annotations: self.string_annotations,
            ..Self::default()
        };
        visit(&mut child);
        self.incomplete |= child.incomplete;
        if let Some(parameters) = parameters {
            child
                .locals
                .extend(parameters.iter().map(|p| p.name().to_string()));
        }
        child.finish_scope();
        child.reads
    }
}

impl<'a> Visitor<'a> for ClosureNames {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                self.locals.insert(f.name.to_string());
                // The declaration collector has already propagated this child's reads.
                if !self.include_nested_reads {
                    self.incomplete = true;
                }
                for expr in imre_ast::function_definition_expressions(f) {
                    self.visit_expr(expr);
                }
            }
            // Local variable annotations never evaluate in a function's scope.
            ast::Stmt::AnnAssign(a) if self.include_nested_reads => {
                // A bare parenthesized name with no value has no binding effect.
                if imre_ast::annotation_visits_target(a) {
                    self.visit_expr(&a.target);
                }
                if let Some(value) = &a.value {
                    self.visit_expr(value);
                }
            }
            ast::Stmt::ClassDef(c) => {
                self.locals.insert(c.name.to_string());
                self.incomplete = true;
                if self.include_nested_reads
                    && let Some(params) = &c.type_params
                {
                    let mut reads = self.child_reads(None, |child| {
                        child.visit_type_params(params);
                        if let Some(args) = &c.arguments {
                            child.visit_arguments(args);
                        }
                    });
                    for param in params.iter() {
                        reads.remove(param.name().as_str());
                    }
                    self.cell_reads.extend(reads.iter().cloned());
                    self.reads.extend(reads);
                }
                for expr in imre_ast::class_definition_expressions(c) {
                    self.visit_expr(expr);
                }
            }
            ast::Stmt::Global(g) => {
                self.globals.extend(g.names.iter().map(ToString::to_string));
            }
            ast::Stmt::Nonlocal(n) => {
                self.nonlocals
                    .extend(n.names.iter().map(ToString::to_string));
            }
            ast::Stmt::Import(i) => {
                for name in &i.names {
                    self.locals.insert(name.asname.as_ref().map_or_else(
                        || name.name.as_str().split('.').next().unwrap().to_string(),
                        ToString::to_string,
                    ));
                }
            }
            ast::Stmt::ImportFrom(i) => {
                for name in &i.names {
                    if name.name.as_str() == "*" {
                        self.incomplete = true;
                        self.locals_incomplete = true;
                    } else {
                        self.locals
                            .insert(name.asname.as_ref().unwrap_or(&name.name).to_string());
                    }
                }
            }
            ast::Stmt::TypeAlias(alias) => {
                self.visit_expr(&alias.name);
                if self.include_nested_reads {
                    // The delayed callable closes over its own parameters before this frame.
                    let mut reads = self.child_reads(None, |child| {
                        child.visit_expr(&alias.value);
                        if let Some(params) = &alias.type_params {
                            child.visit_type_params(params);
                        }
                    });
                    if let Some(params) = &alias.type_params {
                        for param in params.iter() {
                            reads.remove(param.name().as_str());
                        }
                    }
                    self.cell_reads.extend(reads.iter().cloned());
                    self.reads.extend(reads);
                } else {
                    self.incomplete = true;
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        visitor::walk_pattern(self, pattern);
        if let Some(name) = imre_ast::executable::capture_name(pattern) {
            self.locals.insert(name.to_string());
        }
    }

    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(handler_data) = handler;
        if let Some(name) = &handler_data.name {
            self.locals.insert(name.to_string());
        }
        visitor::walk_except_handler(self, handler);
    }
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        match expr {
            ast::Expr::Name(n) => match n.ctx {
                ast::ExprContext::Load => {
                    self.reads.insert(n.id.to_string());
                }
                ast::ExprContext::Store | ast::ExprContext::Del => {
                    self.locals.insert(n.id.to_string());
                }
                _ => {}
            },
            ast::Expr::Lambda(lambda) => {
                if self.include_nested_reads {
                    let reads = self.child_reads(lambda.parameters.as_deref(), |child| {
                        child.visit_expr(&lambda.body)
                    });
                    self.cell_reads.extend(reads.iter().cloned());
                    self.reads.extend(reads);
                } else {
                    self.incomplete = true;
                }
                if let Some(parameters) = &lambda.parameters {
                    for default in imre_ast::parameter_defaults(parameters) {
                        self.visit_expr(default);
                    }
                }
            }
            ast::Expr::ListComp(_)
            | ast::Expr::SetComp(_)
            | ast::Expr::DictComp(_)
            | ast::Expr::Generator(_) => {
                let (reads, bindings, captured) =
                    class_metadata::comprehension_scope(expr, self.string_annotations);
                self.reads.extend(reads);
                self.cell_reads.extend(captured);
                self.locals.extend(bindings);
                if !self.include_nested_reads {
                    self.incomplete = true;
                    self.locals_incomplete = true;
                }
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}

/// Resolved lexical layout shared by function, lambda and namespace entries.
pub(crate) struct FunctionScope {
    names: ClosureNames,
    pub(crate) captures_class: bool,
}

impl FunctionScope {
    pub(super) fn parameter(reads: ParameterReads, class: Option<&ClassBindings>) -> Self {
        let mut names = ClosureNames {
            reads: reads.resolve(class),
            ..ClosureNames::default()
        };
        if class.is_some() {
            names.reads.insert("__classdict__".into());
        }
        names.locals.insert(".format".into());
        Self::new(names, None, false)
    }

    fn new(
        mut names: ClosureNames,
        parameters: Option<&ast::Parameters>,
        captures_class: bool,
    ) -> Self {
        // The frame ABI places keyword-only arguments before varargs.
        if let Some(p) = parameters {
            names.locals.extend(
                p.posonlyargs
                    .iter()
                    .chain(&p.args)
                    .chain(&p.kwonlyargs)
                    .map(|arg| arg.parameter.name.to_string()),
            );
            names.locals.extend(
                p.vararg
                    .iter()
                    .chain(p.kwarg.iter())
                    .map(|arg| arg.name.to_string()),
            );
        }
        names.finish_scope();
        names.cell_reads.retain(|name| names.locals.contains(name));
        Self {
            names,
            captures_class,
        }
    }

    fn function(
        def: &ast::StmtFunctionDef,
        mut names: ClosureNames,
        may_capture_class: bool,
    ) -> Self {
        names.visit_body(&def.body);
        let parameters = Some(def.parameters.as_ref());
        let captures_class = may_capture_class
            && imre_ast::class_cell::function_needed(
                &def.body,
                parameters,
                names.string_annotations,
            );
        Self::new(names, parameters, captures_class)
    }

    pub(crate) fn lambda(expr: &ast::ExprLambda, string_annotations: bool) -> Self {
        let mut names = ClosureNames {
            include_nested_reads: true,
            string_annotations,
            ..Default::default()
        };
        names.visit_expr(&expr.body);
        let parameters = expr.parameters.as_deref();
        Self::new(
            names,
            parameters,
            imre_ast::class_cell::expression_needed(&expr.body, parameters, string_annotations),
        )
    }

    pub(crate) fn write_signature(&self, signature: &mut serde_json::Value) {
        let names = &self.names;
        signature["scope_cellvars"] = if names.incomplete {
            serde_json::Value::Null
        } else {
            json!(names.cell_reads)
        };
        if !names.locals.is_empty() {
            signature["scope_locals"] = json!(names.locals.iter().collect::<Vec<_>>());
        }
        if !names.reads.is_empty() {
            signature["scope_reads"] = json!(names.reads);
        }
        if !names.nonlocals.is_empty() {
            signature["scope_nonlocals"] = json!(names.nonlocals);
        }
        signature["scope_complete"] = json!(!names.incomplete);
        signature["scope_locals_complete"] = json!(!names.locals_incomplete);
    }
}

/// Function-local declarations in lexical source order. These borrows end before emission.
pub(crate) type FunctionChildren<'a> = Vec<&'a ast::Stmt>;

#[derive(Default)]
pub(crate) struct ScopeFacts<'p> {
    pub(crate) module_annotations: bool,
    pub(crate) functions: HashMap<TextSize, NamedFunction<'p>>,
    pub(crate) classes: HashMap<TextSize, classes::ClassScope>,
    pub(crate) module_globals: BTreeSet<String>,
}

impl<'p> ScopeFacts<'p> {
    pub(crate) fn function(&self, def: &ast::StmtFunctionDef) -> &NamedFunction<'p> {
        &self.functions[&def
            .decorator_list
            .first()
            .map_or(def.start(), Ranged::start)]
    }

    pub(crate) fn type_alias_scope(
        &self,
        value: &ast::Expr,
        enclosing_class: Option<TextSize>,
        string_annotations: bool,
    ) -> FunctionScope {
        let mut names = ClosureNames {
            string_annotations,
            ..Default::default()
        };
        if let Some(site) = enclosing_class {
            let class = &self.classes[&site].bindings;
            names.reads = class_metadata::annotation_reads(&[("", value)], Some(class), false);
        } else {
            names.visit_expr(value);
        }
        names.locals.insert(".format".into());
        FunctionScope::new(names, None, false)
    }

    pub(crate) fn prepare<'a>(
        body: &'a [ast::Stmt],
        names: &imre_ast::Names,
        alias_site: impl FnMut(&'a ast::StmtTypeAlias, Option<TextSize>),
        expression: impl FnMut(LexicalSite<'a, '_>),
    ) -> (Self, FunctionChildren<'a>) {
        let mut scopes = Self::default();
        let mut children = Vec::new();
        Collector {
            children: &mut children,
            function_names: ClosureNames::default(),
            scopes: &mut scopes,
            names,
            scope: Scope::Module,
            suite_scope: None,
            yields: false,
            body_yields: false,
            alias_site,
            expression,
        }
        .visit_body(body);
        (scopes, children)
    }
}

/// A lexical operand is ready after the scopes of its children have closed.
pub(crate) enum LexicalSite<'a, 's> {
    Lambda(&'a ast::ExprLambda, super::RegisteredScope, bool),
    Template(&'a ast::Expr, super::RegisteredScope),
    Generator(TextRange, &'s Plan<'a>, bool, &'s FunctionScope),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Module,
    Class(TextSize),
    Function { is_async: bool },
    Lambda,
}

impl Scope {
    fn registered(self) -> super::RegisteredScope {
        match self {
            Self::Module => super::RegisteredScope::Module,
            Self::Class(_) => super::RegisteredScope::Class,
            Self::Function { is_async } => super::RegisteredScope::Function { is_async },
            Self::Lambda => super::RegisteredScope::Lambda,
        }
    }
}

struct Collector<'s, 'p, 'a, A, G> {
    children: &'s mut FunctionChildren<'a>,
    // Nested declarations finish before the enclosing function binds its own body.
    function_names: ClosureNames,
    scopes: &'s mut ScopeFacts<'p>,
    names: &'s imre_ast::Names,
    scope: Scope,
    suite_scope: Option<Scope>,
    yields: bool,
    body_yields: bool,
    alias_site: A,
    expression: G,
}

impl<'a, A, G> Visitor<'a> for Collector<'_, '_, 'a, A, G>
where
    A: FnMut(&'a ast::StmtTypeAlias, Option<TextSize>),
    G: FnMut(LexicalSite<'a, '_>),
{
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        let strings = self.names.string_annotations;
        let parent_yields = self.yields;
        if matches!(self.scope, Scope::Function { .. })
            && matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
        {
            self.children.push(stmt);
        }
        match stmt {
            ast::Stmt::FunctionDef(def) => {
                let parent_names = std::mem::replace(
                    &mut self.function_names,
                    ClosureNames {
                        include_nested_reads: true,
                        string_annotations: strings,
                        ..Default::default()
                    },
                );
                self.suite_scope = Some(Scope::Function {
                    is_async: def.is_async,
                });
                visitor::walk_stmt(self, stmt);
                if self.yields && !parent_yields {
                    // Only decorators and default values execute in the parent.
                    self.yields = imre_ast::function_definition_expressions(def)
                        .any(imre_ast::code_flags::yields_in_expression);
                }
                let start = def
                    .decorator_list
                    .first()
                    .map_or(def.start(), Ranged::start);
                use imre_ast::code_flags::{ASYNC_GENERATOR, COROUTINE, GENERATOR};
                let suspension_flags = match (def.is_async, self.body_yields) {
                    (false, false) => 0,
                    (false, true) => GENERATOR,
                    (true, false) => COROUTINE,
                    (true, true) => ASYNC_GENERATOR,
                };
                // A module supplies globals. A generic function has its own type-parameter scope.
                let may_capture_class = def.type_params.is_some()
                    || match self.scope {
                        Scope::Module => false,
                        Scope::Class(site) => self.scopes.classes[&site].needs_class_cell,
                        Scope::Function { .. } | Scope::Lambda => true,
                    };
                let names = std::mem::replace(&mut self.function_names, parent_names);
                let mut lexical = FunctionScope::function(def, names, may_capture_class);
                let header =
                    NamedFunction::prepare(def, self.names, &mut lexical, suspension_flags);
                if matches!(self.scope, Scope::Function { .. }) {
                    self.function_names.read_child_function(def, lexical.names);
                }
                self.scopes.functions.insert(start, header);
                return;
            }
            ast::Stmt::ClassDef(class) => {
                let start = classes::site(class);
                let enclosing_class = match self.scope {
                    Scope::Class(site) => Some(site),
                    _ => None,
                };
                self.scopes.classes.insert(
                    start,
                    classes::ClassScope::prepare(class, self.names, enclosing_class),
                );
                self.suite_scope = Some(Scope::Class(start));
            }
            ast::Stmt::TypeAlias(alias) => {
                let enclosing_class = match self.scope {
                    Scope::Class(site) => Some(site),
                    _ => None,
                };
                (self.alias_site)(alias, enclosing_class);
            }
            ast::Stmt::Global(global) if self.scope == Scope::Module => {
                self.scopes
                    .module_globals
                    .extend(global.names.iter().map(ToString::to_string));
            }
            ast::Stmt::AnnAssign(_) if self.scope == Scope::Module => {
                self.scopes.module_annotations = true;
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
        if let ast::Stmt::ClassDef(class) = stmt {
            // Methods and aliases need the bindings on entry. Nested class reads are ready on exit.
            class_metadata::capture(class, strings, self.scopes);
            if self.yields && !parent_yields {
                self.yields = imre_ast::class_definition_expressions(class)
                    .any(imre_ast::code_flags::yields_in_expression);
            }
        }
    }

    fn visit_body(&mut self, body: &'a [ast::Stmt]) {
        // Decorators, defaults, and bases are evaluated before entering the declaration's suite.
        let Some(scope) = self.suite_scope.take() else {
            visitor::walk_body(self, body);
            return;
        };
        let parent = std::mem::replace(&mut self.scope, scope);
        let parent_yields = std::mem::take(&mut self.yields);
        visitor::walk_body(self, body);
        self.body_yields = std::mem::replace(&mut self.yields, parent_yields);
        self.scope = parent;
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match expr {
            ast::Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                let parent = std::mem::replace(&mut self.scope, Scope::Lambda);
                let parent_yields = std::mem::take(&mut self.yields);
                self.visit_expr(&lambda.body);
                let yields = std::mem::replace(&mut self.yields, parent_yields);
                self.scope = parent;
                (self.expression)(LexicalSite::Lambda(lambda, parent.registered(), yields));
            }
            ast::Expr::Generator(generator) => {
                let plan = Plan::generator(generator);
                self.visit_expr(plan.outer);
                let is_async = plan.body_suspends();
                let parent = std::mem::replace(&mut self.scope, Scope::Function { is_async });
                let scope =
                    FunctionScope::generator(&plan, self.names.string_annotations, |child| {
                        self.visit_expr(child)
                    });
                self.scope = parent;
                (self.expression)(LexicalSite::Generator(
                    expr.range(),
                    &plan,
                    is_async,
                    &scope,
                ));
            }
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => {
                self.yields = true;
                visitor::walk_expr(self, expr);
            }
            ast::Expr::TString(_) => {
                visitor::walk_expr(self, expr);
                (self.expression)(LexicalSite::Template(expr, self.scope.registered()));
            }
            _ => visitor::walk_expr(self, expr),
        });
    }
}
