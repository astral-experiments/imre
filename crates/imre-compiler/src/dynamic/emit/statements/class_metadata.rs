//! Class bodies can store their implicit metadata in a cell from an enclosing function.

use std::collections::BTreeSet;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

use super::ClosureNames;

const GENERATED: [&str; 3] = ["__module__", "__qualname__", "__doc__"];

pub(super) fn capture(bindings: &mut ClosureNames, body: &[ast::Stmt], string_annotations: bool) {
    let mut uses = Uses::new(string_annotations);
    uses.visit_body(body);
    for name in uses.direct.into_iter().chain(uses.nested) {
        if !bindings.locals.contains(name) && !bindings.globals.contains(name) {
            bindings.nonlocals.insert(name.to_owned());
        }
    }
}

#[derive(Default)]
struct Uses {
    direct: BTreeSet<&'static str>,
    nested: BTreeSet<&'static str>,
    string_annotations: bool,
}

impl Uses {
    fn new(string_annotations: bool) -> Self {
        Self {
            string_annotations,
            ..Self::default()
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
        for name in GENERATED {
            let external = child.direct.contains(name)
                && !bounds.globals.contains(name)
                && !bounds.locals.contains(name)
                && !params
                    .is_some_and(|p| p.iter().any(|arg| arg.as_parameter().name.as_str() == name));
            if external || bounds.nonlocals.contains(name) {
                self.nested.insert(name);
            }
        }
    }

    fn suite(&mut self, body: &[ast::Stmt], params: Option<&ast::Parameters>, class: bool) {
        let mut bounds = ClosureNames::default();
        bounds.visit_body(body);
        let mut child = Self::new(self.string_annotations);
        child.visit_body(body);
        self.child(child, bounds, params, class);
    }

    fn comprehension<'a>(
        &mut self,
        generators: &[ast::Comprehension],
        items: impl IntoIterator<Item = &'a ast::Expr>,
    ) {
        let mut child = Self::new(self.string_annotations);
        let mut bounds = ClosureNames::default();
        for (index, generator) in generators.iter().enumerate() {
            bounds.visit_expr(&generator.target);
            if index == 0 {
                self.visit_expr(&generator.iter);
            } else {
                child.visit_expr(&generator.iter);
            }
            for condition in &generator.ifs {
                child.visit_expr(condition);
            }
        }
        for item in items {
            child.visit_expr(item);
        }
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
                if let Some(name) = GENERATED
                    .iter()
                    .copied()
                    .find(|name| n.id.as_str() == *name)
                {
                    self.direct.insert(name);
                }
            }
            ast::Expr::Lambda(f) => {
                if let Some(params) = &f.parameters {
                    self.visit_parameters(params);
                }
                let mut bounds = ClosureNames::default();
                bounds.visit_expr(&f.body);
                let mut child = Self::new(self.string_annotations);
                child.visit_expr(&f.body);
                self.child(child, bounds, f.parameters.as_deref(), false);
            }
            ast::Expr::ListComp(e) => self.comprehension(&e.generators, [e.elt.as_ref()]),
            ast::Expr::SetComp(e) => self.comprehension(&e.generators, [e.elt.as_ref()]),
            ast::Expr::DictComp(e) => self.comprehension(
                &e.generators,
                e.key.as_deref().into_iter().chain([e.value.as_ref()]),
            ),
            ast::Expr::Generator(e) => self.comprehension(&e.generators, [e.elt.as_ref()]),
            _ => visitor::walk_expr(self, expr),
        }
    }

    fn visit_annotation(&mut self, expr: &'a ast::Expr) {
        if !self.string_annotations {
            self.visit_expr(expr);
        }
    }
}
