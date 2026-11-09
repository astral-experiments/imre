//! Class bodies can store their implicit metadata in a cell from an enclosing function.

use std::collections::BTreeSet;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

use super::{ClassBindings, ClosureNames};

const GENERATED: [&str; 3] = ["__module__", "__qualname__", "__doc__"];

/// Collect class frame reads and the implicit metadata stores resolved as nonlocals.
pub(super) fn capture(
    bindings: &mut ClosureNames,
    body: &[ast::Stmt],
    string_annotations: bool,
) -> BTreeSet<String> {
    let mut uses = Uses::new(string_annotations);
    uses.visit_body(body);
    for name in GENERATED {
        if (uses.direct.contains(name) || uses.nested.contains(name))
            && !bindings.locals.contains(name)
            && !bindings.globals.contains(name)
        {
            bindings.nonlocals.insert(name.to_owned());
        }
    }
    // Class bindings shadow direct reads; functions defined in the body skip its namespace.
    uses.direct
        .retain(|name| !bindings.locals.contains(name) && !bindings.globals.contains(name));
    uses.direct.append(&mut uses.nested);
    uses.direct.extend(bindings.nonlocals.iter().cloned());
    uses.direct
}

/// Methods look through the class namespace to read their enclosing cells.
pub(super) fn method_class_reads(
    body: &[ast::Stmt],
    string_annotations: bool,
) -> Option<BTreeSet<String>> {
    if !body
        .iter()
        .all(|stmt| matches!(stmt, ast::Stmt::Pass(_) | ast::Stmt::FunctionDef(_)))
    {
        return None;
    }
    let mut uses = Uses::new(string_annotations);
    uses.visit_body(body);
    uses.direct.append(&mut uses.nested);
    Some(uses.direct)
}

/// Reads in a nested lambda or comprehension bypass the enclosing class's namespace.
pub(super) fn annotation_reads(
    entries: &[(&str, &ast::Expr)],
    class: Option<&ClassBindings>,
    string_annotations: bool,
) -> BTreeSet<String> {
    let mut uses = Uses::new(string_annotations);
    for (_, expression) in entries {
        uses.visit_annotation(expression);
    }
    if let Some(class) = class {
        if uses.direct.contains("super") || uses.nested.contains("super") {
            uses.nested.insert("__class__".to_owned());
        }
        uses.direct
            .retain(|name| !class.locals.contains(name) && !class.globals.contains(name));
    }
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

/// Resolve the body in its own scope. The leftmost iterable has already been
/// evaluated in the enclosing scope and cannot contribute to this code's cells.
pub(super) fn generator_scope(
    plan: &imre_ast::comprehension::Plan<'_>,
    locals: indexmap::IndexSet<String>,
    string_annotations: bool,
) -> ClosureNames {
    let mut uses = Uses {
        implicit_class: true,
        ..Uses::new(string_annotations)
    };
    plan.visit_body(|expr| uses.visit_expr(expr));
    uses.direct.append(&mut uses.nested);
    let mut scope = ClosureNames {
        locals,
        reads: uses.direct,
        nonlocals: uses.walrus,
        ..ClosureNames::default()
    };
    scope.finish_scope();
    scope
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

    /// A function hides its own bindings; a class namespace does not enclose the next method.
    fn child(
        &mut self,
        mut child: Self,
        bounds: ClosureNames,
        params: Option<&ast::Parameters>,
        class: bool,
    ) {
        if class {
            self.nested.append(&mut child.nested);
        } else {
            child.direct.append(&mut child.nested);
        }
        for name in child.direct {
            if !bounds.globals.contains(&name)
                && !bounds.locals.contains(&name)
                && !params
                    .is_some_and(|p| p.iter().any(|arg| arg.as_parameter().name.as_str() == name))
            {
                self.nested.insert(name);
            }
        }
        self.nested.extend(bounds.nonlocals);
    }

    fn suite(&mut self, body: &[ast::Stmt], params: Option<&ast::Parameters>, class: bool) {
        let mut bounds = ClosureNames::default();
        bounds.visit_body(body);
        let mut child = self.child_scope();
        child.visit_body(body);
        self.child(child, bounds, params, class);
    }

    fn comprehension(&mut self, plan: imre_ast::comprehension::Plan<'_>) {
        self.visit_expr(plan.outer);
        let mut child = self.child_scope();
        let bounds = ClosureNames {
            locals: plan.collect_target_names(),
            ..ClosureNames::default()
        };
        plan.visit_body(|expr| child.visit_expr(expr));
        // Assignment expressions escape comprehension scopes, but stop at a lambda or function.
        if self.inline_comprehensions {
            if !matches!(plan.output, imre_ast::comprehension::Output::Generator(_)) {
                self.walrus.append(&mut child.walrus);
                self.direct.extend(
                    child
                        .direct
                        .into_iter()
                        .filter(|name| !bounds.locals.contains(name.as_str())),
                );
                self.nested.extend(
                    child
                        .nested
                        .into_iter()
                        .filter(|name| !bounds.locals.contains(name.as_str())),
                );
                return;
            }
            self.nested.extend(child.walrus.iter().cloned());
        }
        self.walrus.append(&mut child.walrus);
        self.child(child, bounds, None, false);
    }
}

impl<'a> Visitor<'a> for Uses {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(f) => {
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(params) = &f.type_params {
                    self.visit_type_params(params);
                }
                self.visit_parameters(&f.parameters);
                if let Some(result) = &f.returns {
                    self.visit_annotation(result);
                }
                self.suite(&f.body, Some(&f.parameters), false);
            }
            ast::Stmt::ClassDef(c) => {
                for decorator in &c.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(params) = &c.type_params {
                    self.visit_type_params(params);
                }
                if let Some(args) = &c.arguments {
                    self.visit_arguments(args);
                }
                self.suite(&c.body, None, true);
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

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
                let mut bounds = ClosureNames::default();
                bounds.visit_expr(&f.body);
                let mut child = self.child_scope();
                child.visit_expr(&f.body);
                self.child(child, bounds, f.parameters.as_deref(), false);
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
