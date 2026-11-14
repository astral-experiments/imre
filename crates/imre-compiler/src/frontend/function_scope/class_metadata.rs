//! Class bodies can store their implicit metadata in a cell from an enclosing function.

use std::collections::{BTreeSet, HashSet};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

use super::annotations::{ParameterReads, Symbol};
use super::{ClassBindings, ClosureNames, FunctionScope, ScopeFacts, classes};

const GENERATED: [&str; 3] = ["__module__", "__qualname__", "__doc__"];

/// Collect class frame reads and the implicit metadata stores resolved as nonlocals.
pub(super) fn capture(
    class: &ast::StmtClassDef,
    string_annotations: bool,
    scopes: &mut ScopeFacts<'_>,
) {
    let mut uses = Uses::new(string_annotations);
    BodyUses {
        uses: &mut uses,
        scopes,
    }
    .visit_body(&class.body);
    let scope = scopes
        .classes
        .get_mut(&classes::site(class))
        .expect("class bindings on entry");
    let names = &mut scope.namespace.names;
    let mut reads = uses.reads.enclosing(Some(&scope.bindings));
    for name in GENERATED {
        if reads.contains(name)
            && !scope.bindings.locals.contains(name)
            && !scope.bindings.globals.contains(name)
        {
            names.nonlocals.insert(name.to_owned());
        }
    }
    for name in scope.cells.supplied_only() {
        reads.remove(name);
    }
    reads.extend(names.nonlocals.iter().cloned());
    names.reads = reads;
    if class.type_params.is_some() {
        // The class body publishes the tuple from its generic header as __type_params__.
        names.reads.insert(".type_params".to_owned());
    }
    names.finish_scope();
}

/// Reads in a nested lambda or comprehension bypass the enclosing class's namespace.
pub(super) fn annotation_reads(
    entries: &[(&str, &ast::Expr)],
    class: Option<&ClassBindings>,
    string_annotations: bool,
) -> BTreeSet<String> {
    parameter_reads(entries, string_annotations).resolve(class)
}

pub(in crate::frontend) fn parameter_reads(
    entries: &[(&str, &ast::Expr)],
    string_annotations: bool,
) -> ParameterReads {
    let mut uses = Uses::new(string_annotations);
    for (_, expression) in entries {
        uses.visit_annotation(expression);
    }
    uses.reads
}

/// The body of a lifted display excludes its first iterable, which belongs to its caller.
pub(in crate::frontend) fn display_bindings(
    plan: &imre_ast::comprehension::Plan<'_>,
) -> (HashSet<String>, BTreeSet<String>) {
    let mut uses = Uses::default();
    let targets: HashSet<String> = plan.scope_bindings(|expr| uses.visit_expr(expr));
    let mut reads = uses.reads.enclosing(None);
    reads.retain(|name| !targets.contains(name));
    (targets, reads)
}

/// Resolve free names through nested expression scopes.
pub(in crate::frontend) fn expression_reads(expression: &ast::Expr) -> BTreeSet<String> {
    let mut uses = Uses::default();
    uses.visit_expr(expression);
    uses.reads.enclosing(None)
}

/// Reads that reach the containing function and assignment expressions that bind there.
pub(super) fn comprehension_scope(
    expression: &ast::Expr,
    string_annotations: bool,
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    // Eager comprehensions use the enclosing frame's fast slots. An escaping
    // genexpr or function nested inside the display still captures its reads.
    let mut uses = Uses {
        scope: ExpressionScope::Function,
        ..Uses::new(string_annotations)
    };
    uses.visit_expr(expression);
    let captured = uses.reads.nested.clone();
    (uses.reads.enclosing(None), uses.walrus, captured)
}

impl FunctionScope {
    /// Collect the suspended body’s binding and declaration sites together. Its first
    /// iterable has already been evaluated in the enclosing scope.
    pub(super) fn generator<'a>(
        plan: &imre_ast::comprehension::Plan<'a>,
        string_annotations: bool,
        mut visit: impl FnMut(&'a ast::Expr),
    ) -> Self {
        let mut uses = Uses {
            implicit_class: true,
            scope: ExpressionScope::Function,
            ..Uses::new(string_annotations)
        };
        let mut locals: indexmap::IndexSet<String> = plan.scope_bindings(|expr| {
            visit(expr);
            uses.visit_expr(expr);
        });
        locals.insert(".0".into());
        let captured = uses.reads.nested.clone();
        let names = ClosureNames {
            locals,
            reads: uses.reads.enclosing(None),
            cell_reads: captured,
            nonlocals: uses.walrus,
            ..ClosureNames::default()
        };
        let mut scope = Self::new(names, None, false);
        scope.captures_class = scope.names.reads.contains("__class__");
        scope
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ExpressionScope {
    // Code which requires reads from a separately compiled display.
    #[default]
    Separate,
    Function,
    // The annotation itself ignores names. Its children still resolve symbols.
    Unevaluated,
    AnnotationChild,
}

#[derive(Default)]
pub(in crate::frontend) struct Uses {
    pub(super) reads: ParameterReads,
    walrus: BTreeSet<String>,
    implicit_class: bool,
    scope: ExpressionScope,
    string_annotations: bool,
}

impl Uses {
    fn new(string_annotations: bool) -> Self {
        Self {
            string_annotations,
            ..Self::default()
        }
    }

    pub(super) fn unevaluated() -> Box<Self> {
        Box::new(Self {
            scope: ExpressionScope::Unevaluated,
            implicit_class: true,
            ..Self::default()
        })
    }

    pub(super) fn finish_annotation(mut self, parent: &mut ClosureNames) {
        parent.locals.extend(self.walrus.iter().cloned());
        self.reads.remove_locals();
        self.reads.nested.append(&mut self.walrus);
        let reads = self.reads.enclosing(None);
        parent.cell_reads.extend(reads.iter().cloned());
        parent.reads.extend(reads);
    }

    fn child_scope(&self) -> Self {
        Self {
            implicit_class: self.implicit_class,
            scope: match self.scope {
                ExpressionScope::Unevaluated => ExpressionScope::AnnotationChild,
                scope => scope,
            },
            ..Self::new(self.string_annotations)
        }
    }

    /// A generic declaration supplies its own parameters to its header and body.
    fn declaration(&mut self, params: Option<&ast::TypeParams>, read: impl FnOnce(&mut Self)) {
        let Some(params) = params else {
            return read(self);
        };
        let mut child = self.child_scope();
        child.visit_type_params(params);
        read(&mut child);
        // The generic header binds its parameter tuple as well as each parameter.
        for name in params
            .iter()
            .map(|p| p.name().as_str())
            .chain([".type_params"])
        {
            child.reads.symbols.remove(name);
            child.reads.nested.remove(name);
        }
        self.reads.symbols.extend(
            child
                .reads
                .symbols
                .into_iter()
                .filter(|(_, symbol)| *symbol == Symbol::Read),
        );
        self.reads.nested.append(&mut child.reads.nested);
    }

    fn child(&mut self, mut child: ParameterReads, locals: BTreeSet<String>, inline: bool) {
        child
            .symbols
            .extend(locals.into_iter().map(|name| (name, Symbol::Local)));
        child.remove_locals();
        for name in &child.nested {
            child.symbols.entry(name.clone()).or_insert(Symbol::Read);
        }
        for (name, symbol) in child.symbols {
            match (inline, self.scope) {
                (true, ExpressionScope::Unevaluated | ExpressionScope::AnnotationChild) => {
                    // CPython copies only the first symbol supplied by an inlined display.
                    self.reads.symbols.entry(name).or_insert(symbol);
                }
                (true, ExpressionScope::Function) if symbol == Symbol::Read => {
                    self.reads.symbols.insert(name, symbol);
                }
                _ if symbol == Symbol::Read => {
                    self.reads.nested.insert(name);
                }
                _ => {}
            }
        }
        self.reads.nested.append(&mut child.nested);
    }

    fn comprehension(&mut self, plan: imre_ast::comprehension::Plan<'_>) {
        self.visit_expr(plan.outer);
        let mut child = self.child_scope();
        let targets = plan.scope_bindings(|expr| child.visit_expr(expr));
        let inline = !matches!(plan.output, imre_ast::comprehension::Output::Generator(_));
        if !inline && self.scope == ExpressionScope::Function {
            self.reads.nested.extend(child.walrus.iter().cloned());
        }
        self.walrus.append(&mut child.walrus);
        self.child(child.reads, targets, inline);
    }
}

struct BodyUses<'s, 'p> {
    uses: &'s mut Uses,
    scopes: &'s ScopeFacts<'p>,
}

impl<'a> Visitor<'a> for BodyUses<'_, '_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        self.uses.visit_expr(expr);
    }

    fn visit_annotation(&mut self, expr: &'a ast::Expr) {
        self.uses.visit_annotation(expr);
    }

    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                for expr in imre_ast::function_definition_expressions(f) {
                    self.visit_expr(expr);
                }
                let method = self.scopes.function(f);
                let mut extra = BTreeSet::new();
                if !method.metadata.reads_complete() {
                    // A nested class may pass on reads its enclosing function can't resolve.
                    let mut child = self.uses.child_scope();
                    BodyUses {
                        uses: &mut child,
                        scopes: self.scopes,
                    }
                    .visit_body(&f.body);
                    extra = child.reads.enclosing(None);
                    for local in method.metadata.local_names() {
                        if let Some(name) = local.as_str() {
                            extra.remove(name);
                        }
                    }
                    extra.retain(|name| !method.globals.contains(name));
                }
                let reads = method
                    .metadata
                    .reads()
                    .chain(extra.iter().map(String::as_str));
                self.uses.declaration(f.type_params.as_deref(), |scope| {
                    if !scope.string_annotations {
                        for (_, expr) in crate::frontend::parameter_annotations(
                            &f.parameters,
                            f.returns.as_deref(),
                        ) {
                            scope.visit_expr(expr);
                        }
                    }
                    scope.reads.nested.extend(reads.map(str::to_owned));
                });
            }
            ast::Stmt::ClassDef(c) => {
                for decorator in &c.decorator_list {
                    self.visit_decorator(decorator);
                }
                let child = &self.scopes.classes[&classes::site(c)];
                self.uses.declaration(c.type_params.as_deref(), |scope| {
                    if let Some(args) = &c.arguments {
                        scope.visit_arguments(args);
                    }
                    scope
                        .reads
                        .nested
                        .extend(child.namespace.names.reads.iter().cloned());
                });
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }
}

impl<'a> Visitor<'a> for Uses {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        match expr {
            ast::Expr::Name(n)
                if n.ctx == ast::ExprContext::Load
                    && self.scope != ExpressionScope::Unevaluated =>
            {
                let name = n.id.as_str();
                self.reads.symbols.insert(name.to_owned(), Symbol::Read);
                if self.implicit_class && name == "super" {
                    self.reads
                        .symbols
                        .insert("__class__".to_owned(), Symbol::Read);
                }
            }
            ast::Expr::Dict(dict) => {
                for expr in dict.iter_keys().flatten().chain(dict.iter_values()) {
                    self.visit_expr(expr);
                }
            }
            ast::Expr::Named(n) => {
                if let ast::Expr::Name(target) = &*n.target {
                    self.walrus.insert(target.id.to_string());
                }
                self.visit_expr(&n.value);
            }
            ast::Expr::Lambda(f) => {
                if let Some(params) = &f.parameters {
                    self.visit_parameters(params);
                }
                let mut child = self.child_scope();
                child.visit_expr(&f.body);
                let mut locals = std::mem::take(&mut child.walrus);
                if let Some(params) = &f.parameters {
                    locals.extend(params.iter().map(|arg| arg.name().to_string()));
                }
                self.child(child.reads, locals, false);
            }
            expr if let Some(plan) = imre_ast::comprehension::Plan::from_expr(expr) => {
                self.comprehension(plan);
            }
            _ => visitor::walk_expr(self, expr),
        }
    }

    fn visit_annotation(&mut self, expr: &'a ast::Expr) {
        if !self.string_annotations {
            self.visit_expr(expr);
        }
    }
}
