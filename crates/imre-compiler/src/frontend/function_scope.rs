//! Lexical bindings and code-object metadata for prepared Python functions.

mod annotations;
pub(crate) use annotations::ClassAnnotations;
mod callable;
pub(crate) use callable::{CallableMetadata, OrdinaryFunction, parameter_layout};
mod class_metadata;
pub(crate) mod classes;
mod type_parameters;

pub(crate) use type_parameters::{
    TypeParameterScope, boxed_function_type_params, check_nonlocals, type_parameter_scope,
};

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

#[derive(Clone)]
pub(crate) struct ClassBindings {
    pub(crate) locals: BTreeSet<String>,
    pub(crate) globals: BTreeSet<String>,
}

fn class_locals(body: &[ast::Stmt]) -> ClassBindings {
    let mut names = ClosureNames::default();
    names.visit_body(body);
    ClassBindings::from_names(&names)
}

impl ClosureNames {
    fn finish_scope(&mut self) {
        for name in self.globals.iter().chain(&self.nonlocals) {
            self.locals.shift_remove(name);
        }
        self.reads
            .retain(|name| !self.locals.contains(name) && !self.globals.contains(name));
        self.reads.extend(self.nonlocals.iter().cloned());
    }

    /// A nested function's external reads propagate through its enclosing function.
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
                if self.include_nested_reads {
                    // The child body and its lazy annotations see type parameters. Only
                    // the body sees the child's parameters and local bindings.
                    let mut reads =
                        self.child_reads(Some(&f.parameters), |child| child.visit_body(&f.body));
                    let mut annotations = if self.string_annotations {
                        Vec::new()
                    } else {
                        crate::frontend::parameter_annotations(&f.parameters, f.returns.as_deref())
                    };
                    if let Some(params) = &f.type_params {
                        for param in params.iter() {
                            if let ast::TypeParam::TypeVar(v) = param
                                && let Some(bound) = v.bound.as_deref()
                            {
                                annotations.push(("", bound));
                            }
                            if let Some(default) = param.default() {
                                annotations.push(("", default));
                            }
                        }
                    }
                    reads.extend(class_metadata::annotation_reads(&annotations, None, false));
                    if let Some(params) = &f.type_params {
                        for param in params.iter() {
                            reads.remove(param.name().as_str());
                        }
                    }
                    self.cell_reads.extend(reads.iter().cloned());
                    self.reads.extend(reads);
                } else {
                    self.incomplete = true;
                }
                // Decorator expressions run here and can bind names in this frame.
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                }
                for param in f.parameters.iter_non_variadic_params() {
                    if let Some(default) = &param.default {
                        // Defaults run here. Their expression walk records enclosing bindings.
                        self.visit_expr(default);
                    }
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
                // Only class header expressions can introduce names in this enclosing function.
                for decorator in &c.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(args) = &c.arguments {
                    self.visit_arguments(args);
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
                    let mut reads = self.child_reads(None, |child| child.visit_expr(&alias.value));
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
            ast::Stmt::Match(_) => {
                self.incomplete = true;
                self.locals_incomplete = true;
                visitor::walk_stmt(self, stmt);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        let capture = match pattern {
            ast::Pattern::MatchAs(p) => p.name.as_ref(),
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = capture {
            self.locals.insert(name.to_string());
        }
        visitor::walk_pattern(self, pattern);
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
                    for parameter in parameters.iter_non_variadic_params() {
                        if let Some(default) = &parameter.default {
                            self.visit_expr(default);
                        }
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

    pub(crate) fn function(def: &ast::StmtFunctionDef, string_annotations: bool) -> Self {
        let mut names = ClosureNames {
            include_nested_reads: true,
            string_annotations,
            ..Default::default()
        };
        names.visit_body(&def.body);
        let parameters = Some(def.parameters.as_ref());
        Self::new(
            names,
            parameters,
            imre_ast::class_cell::function_needed(&def.body, parameters, string_annotations),
        )
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

    pub(crate) fn globals(&self) -> &BTreeSet<String> {
        &self.names.globals
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

pub(crate) struct NamedFunctionScope {
    pub(crate) ordinary: Option<OrdinaryFunction>,
    pub(crate) lexical: FunctionScope,
    pub(crate) suspension_flags: u32,
}

#[derive(Default)]
pub(crate) struct ScopeFacts {
    pub(crate) module_annotations: bool,
    pub(crate) functions: HashMap<TextSize, NamedFunctionScope>,
    pub(crate) lambdas: HashMap<TextSize, FunctionScope>,
    pub(crate) aliases: HashMap<TextSize, FunctionScope>,
    pub(crate) generators: HashMap<TextRange, FunctionScope>,
    pub(crate) comprehension_targets: HashMap<TextRange, String>,
    pub(crate) classes: HashMap<TextSize, classes::ClassScope>,
    pub(crate) annotations: annotations::AnnotationScopes,
    pub(crate) module_globals: BTreeSet<String>,
}

impl ScopeFacts {
    pub(crate) fn function(&self, def: &ast::StmtFunctionDef) -> &NamedFunctionScope {
        &self.functions[&def
            .decorator_list
            .first()
            .map_or(def.start(), Ranged::start)]
    }

    pub(crate) fn prepare<'a>(
        body: &'a [ast::Stmt],
        names: &imre_ast::Names,
        alias_site: impl FnMut(&'a ast::StmtTypeAlias),
        expression_site: impl FnMut(&ast::Expr),
    ) -> Self {
        let mut scopes = Self::default();
        Collector {
            scopes: &mut scopes,
            names,
            scope: Scope::Module,
            alias_site,
            expression_site,
        }
        .visit_body(body);
        scopes
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Module,
    Class(TextSize),
    Function,
}

struct Collector<'a, A, E> {
    scopes: &'a mut ScopeFacts,
    names: &'a imre_ast::Names,
    scope: Scope,
    alias_site: A,
    expression_site: E,
}

impl<'a, A: FnMut(&'a ast::StmtTypeAlias), E: FnMut(&ast::Expr)> Visitor<'a>
    for Collector<'_, A, E>
{
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        let strings = self.names.string_annotations;
        let parent = self.scope;
        match stmt {
            ast::Stmt::FunctionDef(def) => {
                let start = def
                    .decorator_list
                    .first()
                    .map_or(def.start(), Ranged::start);
                use imre_ast::code_flags::{ASYNC_GENERATOR, COROUTINE, GENERATOR, is_generator};
                let suspension_flags = match (def.is_async, is_generator(&def.body)) {
                    (false, false) => 0,
                    (false, true) => GENERATOR,
                    (true, false) => COROUTINE,
                    (true, true) => ASYNC_GENERATOR,
                };
                let lexical = FunctionScope::function(def, strings);
                let ordinary =
                    OrdinaryFunction::prepare(def, self.names, &lexical, suspension_flags);
                self.scopes.functions.insert(
                    start,
                    NamedFunctionScope {
                        lexical,
                        ordinary,
                        suspension_flags,
                    },
                );
                self.scopes.annotations.function(def, strings);
                self.scope = Scope::Function;
            }
            ast::Stmt::ClassDef(class) => {
                let start = class
                    .decorator_list
                    .first()
                    .map_or(class.start(), Ranged::start);
                self.scopes
                    .classes
                    .insert(start, classes::ClassScope::prepare(class, self.names));
                self.scope = Scope::Class(start);
            }
            ast::Stmt::TypeAlias(alias) => {
                (self.alias_site)(alias);
                let mut names = ClosureNames {
                    string_annotations: strings,
                    ..Default::default()
                };
                if let Scope::Class(site) = self.scope {
                    let class = &self.scopes.classes[&site].bindings;
                    names.reads =
                        class_metadata::annotation_reads(&[("", &alias.value)], Some(class), false);
                } else {
                    names.visit_expr(&alias.value);
                }
                names.locals.insert(".format".into());
                self.scopes
                    .aliases
                    .insert(alias.start(), FunctionScope::new(names, None, false));
            }
            ast::Stmt::Global(global) if self.scope == Scope::Module => {
                self.scopes
                    .module_globals
                    .extend(global.names.iter().map(ToString::to_string));
            }
            ast::Stmt::AnnAssign(assign) => match self.scope {
                Scope::Module => self.scopes.module_annotations = true,
                Scope::Class(site) if !strings => {
                    self.scopes.annotations.assignment(assign);
                    self.scopes
                        .classes
                        .get_mut(&site)
                        .unwrap()
                        .annotations
                        .observe(assign);
                }
                _ => {}
            },
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
        if let (ast::Stmt::ClassDef(class), Scope::Class(start)) = (stmt, self.scope) {
            let scope = self.scopes.classes.get_mut(&start).unwrap();
            scope
                .annotations
                .finish(class, &self.scopes.annotations, &scope.bindings);
        }
        self.scope = parent;
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        (self.expression_site)(expr);
        let strings = self.names.string_annotations;
        if let ast::Expr::Lambda(lambda) = expr {
            self.scopes
                .lambdas
                .insert(lambda.start(), FunctionScope::lambda(lambda, strings));
        } else if let Some(plan) = imre_ast::comprehension::Plan::from_expr(expr) {
            let targets: indexmap::IndexSet<String> = plan.collect_target_names();
            self.scopes.comprehension_targets.insert(
                expr.range(),
                targets
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            if matches!(expr, ast::Expr::Generator(_)) {
                let mut names = class_metadata::generator_scope(&plan, targets, strings);
                let captures_class = names.reads.contains("__class__");
                names.locals.insert(".0".into());
                self.scopes.generators.insert(
                    expr.range(),
                    FunctionScope::new(names, None, captures_class),
                );
            }
        }
        visitor::walk_expr(self, expr);
    }
}
