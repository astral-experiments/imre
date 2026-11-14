//! Class bodies can store their implicit metadata in a cell from an enclosing function.

use std::collections::{BTreeSet, HashSet};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

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
    for name in GENERATED {
        if (uses.direct.contains(name) || uses.nested.contains(name))
            && !scope.bindings.locals.contains(name)
            && !scope.bindings.globals.contains(name)
        {
            names.nonlocals.insert(name.to_owned());
        }
    }
    // Class bindings shadow direct reads; functions defined in the body skip its namespace.
    uses.direct.retain(|name| {
        !scope.bindings.locals.contains(name) && !scope.bindings.globals.contains(name)
    });
    uses.direct.append(&mut uses.nested);
    for name in scope.cells.supplied_only() {
        uses.direct.remove(name);
    }
    uses.direct.extend(names.nonlocals.iter().cloned());
    names.reads = uses.direct;
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
) -> super::annotations::ParameterReads {
    let mut uses = Uses::new(string_annotations);
    for (_, expression) in entries {
        uses.visit_annotation(expression);
    }
    super::annotations::ParameterReads {
        direct: uses.direct,
        nested: uses.nested,
    }
}

/// The body of a lifted display excludes its first iterable, which belongs to its caller.
pub(in crate::frontend) fn display_bindings(
    plan: &imre_ast::comprehension::Plan<'_>,
) -> (HashSet<String>, BTreeSet<String>) {
    let mut uses = Uses::default();
    let targets: HashSet<String> = plan.scope_bindings(|expr| uses.visit_expr(expr));
    uses.direct.append(&mut uses.nested);
    uses.direct.retain(|name| !targets.contains(name));
    (targets, uses.direct)
}

/// Resolve free names through nested expression scopes.
pub(in crate::frontend) fn expression_reads(expression: &ast::Expr) -> BTreeSet<String> {
    let mut uses = Uses::default();
    uses.visit_expr(expression);
    uses.direct.append(&mut uses.nested);
    uses.direct
}

/// Reads that reach the containing function and assignment expressions that bind there.
pub(super) fn comprehension_scope(
    expression: &ast::Expr,
    string_annotations: bool,
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    // Eager comprehensions use the enclosing frame's fast slots. An escaping
    // genexpr or function nested inside the display still captures its reads.
    let mut uses = Uses {
        inline_comprehensions: true,
        ..Uses::new(string_annotations)
    };
    uses.visit_expr(expression);
    uses.direct.extend(uses.nested.iter().cloned());
    (uses.direct, uses.walrus, uses.nested)
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
            inline_comprehensions: true,
            ..Uses::new(string_annotations)
        };
        let mut locals: indexmap::IndexSet<String> = plan.scope_bindings(|expr| {
            visit(expr);
            uses.visit_expr(expr);
        });
        locals.insert(".0".into());
        uses.direct.extend(uses.nested.iter().cloned());
        let names = ClosureNames {
            locals,
            reads: uses.direct,
            cell_reads: uses.nested,
            nonlocals: uses.walrus,
            ..ClosureNames::default()
        };
        let mut scope = Self::new(names, None, false);
        scope.captures_class = scope.names.reads.contains("__class__");
        scope
    }
}

#[derive(Default)]
struct Uses {
    direct: BTreeSet<String>,
    nested: BTreeSet<String>,
    walrus: BTreeSet<String>,
    implicit_class: bool,
    inline_comprehensions: bool,
    string_annotations: bool,
}

impl Uses {
    fn new(string_annotations: bool) -> Self {
        Self {
            string_annotations,
            ..Self::default()
        }
    }

    fn child_scope(&self) -> Self {
        Self {
            implicit_class: self.implicit_class,
            inline_comprehensions: self.inline_comprehensions,
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
            child.direct.remove(name);
            child.nested.remove(name);
        }
        self.direct.append(&mut child.direct);
        self.nested.append(&mut child.nested);
    }

    /// A function hides its own bindings from its enclosing scope.
    fn child(
        &mut self,
        mut child: Self,
        params: Option<&ast::Parameters>,
        is_bound: impl Fn(&str) -> bool,
    ) {
        child.direct.append(&mut child.nested);
        for name in child.direct {
            if !is_bound(&name)
                && !params
                    .is_some_and(|p| p.iter().any(|arg| arg.as_parameter().name.as_str() == name))
            {
                self.nested.insert(name);
            }
        }
    }

    fn comprehension(&mut self, plan: imre_ast::comprehension::Plan<'_>) {
        self.visit_expr(plan.outer);
        let mut child = self.child_scope();
        let targets: HashSet<String> = plan.scope_bindings(|expr| child.visit_expr(expr));
        // Assignment expressions escape comprehension scopes, but stop at a lambda or function.
        if self.inline_comprehensions {
            if !matches!(plan.output, imre_ast::comprehension::Output::Generator(_)) {
                self.walrus.append(&mut child.walrus);
                self.direct.extend(
                    child
                        .direct
                        .into_iter()
                        .filter(|name| !targets.contains(name.as_str())),
                );
                self.nested.extend(
                    child
                        .nested
                        .into_iter()
                        .filter(|name| !targets.contains(name.as_str())),
                );
                return;
            }
            self.nested.extend(child.walrus.iter().cloned());
        }
        self.walrus.append(&mut child.walrus);
        self.child(child, None, |name| targets.contains(name));
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
                let mut child = self.uses.child_scope();
                if !method.metadata.reads_complete() {
                    // A nested class may pass on reads its enclosing function can't resolve.
                    BodyUses {
                        uses: &mut child,
                        scopes: self.scopes,
                    }
                    .visit_body(&f.body);
                    child.direct.append(&mut child.nested);
                    for local in method.metadata.local_names() {
                        if let Some(name) = local.as_str() {
                            child.direct.remove(name);
                        }
                    }
                    child.direct.retain(|name| !method.globals.contains(name));
                }
                let reads = method
                    .metadata
                    .reads()
                    .chain(child.direct.iter().map(String::as_str));
                self.uses.declaration(f.type_params.as_deref(), |scope| {
                    if !scope.string_annotations {
                        for (_, expr) in crate::frontend::parameter_annotations(
                            &f.parameters,
                            f.returns.as_deref(),
                        ) {
                            scope.visit_expr(expr);
                        }
                    }
                    scope.nested.extend(reads.map(str::to_owned));
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
            ast::Expr::Name(n) if n.ctx == ast::ExprContext::Load => {
                let name = n.id.as_str();
                self.direct.insert(name.to_owned());
                if self.implicit_class && name == "super" {
                    self.direct.insert("__class__".to_owned());
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
                let locals = std::mem::take(&mut child.walrus);
                self.child(child, f.parameters.as_deref(), |name| locals.contains(name));
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
