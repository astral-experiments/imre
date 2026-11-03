//! Expression restrictions in annotation scopes used by type aliases and generic classes.
use std::collections::HashSet;

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

type Error<'a> = (&'a Expr, &'static str);

/// Validate alias declarations before execution, including those in nested or untaken suites.
pub fn type_alias_error(body: &[Stmt]) -> Option<(TextRange, String)> {
    #[derive(Default)]
    struct Module(Option<(TextRange, String)>);
    impl<'a> Visitor<'a> for Module {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.0.is_some() {
                return;
            }
            if let Stmt::TypeAlias(alias) = stmt {
                if let Some(params) = &alias.type_params {
                    let mut names = HashSet::new();
                    for param in params.iter() {
                        let name = param.name();
                        if !names.insert(name.as_str()) {
                            self.0 =
                                Some((param.range(), format!("duplicate type parameter '{name}'")));
                            return;
                        }
                    }
                }
                let mut rhs = Rhs::default();
                rhs.visit_expr(&alias.value);
                self.0 = rhs
                    .error
                    .map(|(expr, reason)| (expr.range(), reason.to_owned()));
            } else {
                visitor::walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, _: &'a Expr) {}
    }

    let mut module = Module::default();
    module.visit_body(body);
    module.0
}

/// Validate the base and keyword expressions executed in a generic class's parameter scope.
pub fn generic_class_arguments_error(arguments: &ast::Arguments) -> Option<Error<'_>> {
    let mut rhs = Rhs {
        generic_class: true,
        ..Default::default()
    };
    rhs.visit_arguments(arguments);
    rhs.error
}

#[derive(Default)]
struct Rhs<'a> {
    generic_class: bool,
    error: Option<Error<'a>>,
    comprehension: Option<&'a Expr>,
    async_allowed: bool,
    in_iterable: bool,
}

impl<'a> Visitor<'a> for Rhs<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.error.is_some() {
            return;
        }
        let error = match expr {
            Expr::Named(_) if self.comprehension.is_none() => (
                expr,
                if self.generic_class {
                    "named expression cannot be used within the definition of a generic"
                } else {
                    "named expression cannot be used within a type alias"
                },
            ),
            Expr::Named(_) if self.in_iterable => (
                expr,
                "assignment expression cannot be used in a comprehension iterable expression",
            ),
            Expr::Named(n) => (
                &*n.target,
                if self.generic_class {
                    "assignment expression within a comprehension cannot be used within the definition of a generic"
                } else {
                    "assignment expression within a comprehension cannot be used in a type alias"
                },
            ),
            Expr::Yield(_) | Expr::YieldFrom(_) => (
                expr,
                match self.comprehension {
                    Some(Expr::ListComp(_)) => "'yield' inside list comprehension",
                    Some(Expr::SetComp(_)) => "'yield' inside set comprehension",
                    Some(Expr::DictComp(_)) => "'yield' inside dict comprehension",
                    Some(_) => "'yield' inside generator expression",
                    None if self.generic_class => {
                        "yield expression cannot be used within the definition of a generic"
                    }
                    None => "yield expression cannot be used within a type alias",
                },
            ),
            Expr::Await(_) if !self.async_allowed => match self.comprehension {
                Some(comp) => (
                    comp,
                    "asynchronous comprehension outside of an asynchronous function",
                ),
                None if self.generic_class => (
                    expr,
                    "await expression cannot be used within the definition of a generic",
                ),
                None => (expr, "await expression cannot be used within a type alias"),
            },
            Expr::Lambda(lambda) => {
                // Defaults run in the enclosing scope. The lambda body has its own scope.
                if let Some(params) = &lambda.parameters {
                    self.visit_parameters(params);
                }
                return;
            }
            Expr::Generator(g) => return self.comprehension(expr, &g.generators, &[&g.elt]),
            Expr::ListComp(c) => return self.comprehension(expr, &c.generators, &[&c.elt]),
            Expr::SetComp(c) => return self.comprehension(expr, &c.generators, &[&c.elt]),
            Expr::DictComp(c) => match c.key.as_deref() {
                Some(key) => return self.comprehension(expr, &c.generators, &[key, &c.value]),
                None => return self.comprehension(expr, &c.generators, &[&c.value]),
            },
            _ => {
                return stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    visitor::walk_expr(self, expr)
                });
            }
        };
        self.error = Some(error);
    }
}

impl<'a> Rhs<'a> {
    fn comprehension(
        &mut self,
        expr: &'a Expr,
        clauses: &'a [ast::Comprehension],
        result: &[&'a Expr],
    ) {
        // The first iterable still runs in the enclosing scope.
        self.visit_expr(&clauses[0].iter);
        let mut child = Self {
            generic_class: self.generic_class,
            error: self.error,
            comprehension: Some(expr),
            async_allowed: self.async_allowed || matches!(expr, Expr::Generator(_)),
            in_iterable: self.in_iterable,
        };
        for (i, clause) in clauses.iter().enumerate() {
            if i != 0 {
                child.in_iterable = true;
                child.visit_expr(&clause.iter);
                child.in_iterable = self.in_iterable;
            }
            if clause.is_async && !child.async_allowed {
                child.error.get_or_insert((
                    expr,
                    "asynchronous comprehension outside of an asynchronous function",
                ));
            }
            for condition in &clause.ifs {
                child.visit_expr(condition);
            }
        }
        for value in result {
            child.visit_expr(value);
        }
        self.error = child.error;
    }
}
