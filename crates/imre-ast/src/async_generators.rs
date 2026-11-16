//! Check suspension and comprehension bindings in their lexical scopes.
use ruff_python_ast::name::Name;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use std::collections::HashSet;

use crate::comprehension::{Output, Plan, visit_target_leaves};

pub struct Error {
    pub range: TextRange,
    pub message: String,
    /// Code-generation errors include the location tuple in SyntaxError.args.
    /// Symbol-table errors set the location attributes separately.
    pub location_in_args: bool,
}

pub fn check(module: ast::ModRef<'_>, has_named_bindings: bool) -> Option<Error> {
    let mut check = Check {
        has_named_bindings,
        ..Check::default()
    };
    match module {
        ast::ModRef::Module(module) => check.visit_body(&module.body),
        ast::ModRef::Expression(expr) => check.visit_expr(&expr.body),
    }
    let error = check.error?;
    if let ast::ModRef::Module(module) = module
        && let Some((range, _)) = crate::annotation_scope_error(&module.body)
        && (error.location_in_args || range.start() <= error.range.start())
    {
        return None;
    }
    if error.location_in_args
        && crate::repeated_keyword(module.into()).is_some_and(|kw| kw.start() < error.range.start())
    {
        return None;
    }
    Some(error)
}

#[derive(Default)]
struct AsyncBody {
    yields: bool,
    valued_return: Option<TextRange>,
}

#[derive(Default)]
struct Comprehension {
    targets: HashSet<Name>,
    writes: HashSet<Name>,
}

#[derive(Default)]
struct Check<'a> {
    has_named_bindings: bool,
    comprehensions: Vec<Comprehension>,
    in_target: bool,
    in_iterable: bool,
    private: &'a str,
    forbidden_binding: Option<&'static str>,
    annotation: bool,
    current: Option<AsyncBody>,
    function: bool,
    comprehension_yield_error: Option<&'static str>,
    outer_iterable: bool,
    error: Option<Error>,
}

impl<'a> Check<'a> {
    fn record(&mut self, range: TextRange, message: impl Into<String>, location_in_args: bool) {
        if self.error.as_ref().is_none_or(|previous| {
            previous.location_in_args
                && (!location_in_args || range.start() < previous.range.start())
        }) {
            self.error = Some(Error {
                range,
                message: message.into(),
                location_in_args,
            });
        }
    }

    fn inner_loop_conflict(&mut self, name: &ast::ExprName) {
        self.record(
            name.range(),
            format!(
                "comprehension inner loop cannot rebind assignment expression target '{}'",
                name.id
            ),
            false,
        );
    }

    fn scope(&mut self, body: &'a [Stmt], function: bool, is_async: bool) {
        let outer = std::mem::replace(&mut self.current, is_async.then(AsyncBody::default));
        let enclosing_binding = std::mem::replace(
            &mut self.forbidden_binding,
            (!function).then_some(
                "assignment expression within a comprehension cannot be used in a class body",
            ),
        );
        let enclosing_function = std::mem::replace(&mut self.function, function);
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.visit_body(body));
        self.function = enclosing_function;
        self.forbidden_binding = enclosing_binding;
        if let Some(AsyncBody {
            yields: true,
            valued_return: Some(range),
        }) = std::mem::replace(&mut self.current, outer)
        {
            self.record(range, "'return' with value in async generator", true);
        }
    }

    fn target(&mut self, target: &'a Expr) {
        self.in_target = self.has_named_bindings;
        visit_target_leaves(target, |_| Ok(()), |leaf| self.visit_expr(leaf))
            .expect("collecting target leaves without shape validation");
        self.in_target = false;
    }

    fn comprehension(&mut self, expr: &Expr, plan: Plan<'a>) {
        let previous = self.error.take();
        let outer = std::mem::replace(&mut self.outer_iterable, true);
        let enclosing_iterable = std::mem::replace(&mut self.in_iterable, true);
        self.visit_expr(plan.outer);
        self.in_iterable = enclosing_iterable;
        self.outer_iterable = false;
        let parent = self.comprehension_yield_error.replace(match plan.output {
            Output::Generator(_) => "'yield' inside generator expression",
            Output::List(_) => "'yield' inside list comprehension",
            Output::Set(_) => "'yield' inside set comprehension",
            Output::Dict(..) => "'yield' inside dict comprehension",
        });
        let enclosing_target = self.in_target;
        if self.has_named_bindings {
            self.comprehensions.push(Comprehension::default());
        }
        plan.visit_symbol_body(self, Self::target, |scope, iterable| {
            scope.in_iterable = true;
            scope.visit_expr(iterable);
            scope.in_iterable = enclosing_iterable;
        });
        self.comprehensions.pop();
        self.in_target = enclosing_target;
        self.comprehension_yield_error = parent;
        self.outer_iterable = outer;
        // A deferred frame may suspend anywhere. Nested eager displays propagate
        // their suspension to the outer display, whose scope determines legality.
        if !self.annotation
            && !matches!(plan.output, Output::Generator(_))
            && parent.is_none()
            && self.current.is_none()
            && plan.body_suspends()
            && self.error.as_ref().is_none_or(|e| e.location_in_args)
        {
            self.record(
                expr.range(),
                "asynchronous comprehension outside of an asynchronous function",
                false,
            );
        }
        if let Some(err) = std::mem::replace(&mut self.error, previous) {
            self.record(err.range, err.message, err.location_in_args);
        }
    }
}

impl<'a> Visitor<'a> for Check<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                // The symbol table processes defaults before decorator expressions.
                for expr in crate::parameter_defaults(&f.parameters)
                    .chain(crate::decorator_expressions(&f.decorator_list))
                {
                    self.visit_expr(expr);
                }
                if self.has_named_bindings {
                    let enclosing = self.forbidden_binding;
                    if f.type_params.is_some() {
                        self.forbidden_binding = Some(
                            "assignment expression within a comprehension cannot be used within the definition of a generic",
                        );
                    }
                    let p = &f.parameters;
                    for annotation in p
                        .posonlyargs
                        .iter()
                        .chain(&p.args)
                        .map(|p| &p.parameter)
                        .chain(p.vararg.as_deref())
                        .chain(p.kwarg.as_deref())
                        .chain(p.kwonlyargs.iter().map(|p| &p.parameter))
                        .filter_map(|p| p.annotation.as_deref())
                        .chain(f.returns.as_deref())
                    {
                        self.visit_annotation(annotation);
                    }
                    self.forbidden_binding = enclosing;
                }
                self.scope(&f.body, true, f.is_async);
            }
            Stmt::ClassDef(c) => {
                for expr in crate::class_definition_expressions(c) {
                    self.visit_expr(expr);
                }
                let owner = std::mem::replace(&mut self.private, c.name.as_str());
                self.scope(&c.body, false, false);
                self.private = owner;
            }
            Stmt::AnnAssign(assignment) if self.has_named_bindings => {
                if crate::annotation_visits_target(assignment) {
                    self.visit_expr(&assignment.target);
                }
                self.visit_annotation(&assignment.annotation);
                if let Some(value) = &assignment.value {
                    self.visit_expr(value);
                }
            }
            Stmt::TypeAlias(_) => {}
            _ => {
                if let Stmt::Return(r) = stmt
                    && r.value.is_some()
                    && let Some(body) = &mut self.current
                {
                    body.valued_return.get_or_insert(r.range());
                }
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    visitor::walk_stmt(self, stmt)
                });
            }
        }
    }

    fn visit_annotation(&mut self, expr: &'a Expr) {
        if self.has_named_bindings {
            // Annotation evaluation has its own suspension rules and cannot make the
            // enclosing function a generator. Comprehension binding errors still apply.
            let enclosing = std::mem::replace(&mut self.annotation, true);
            self.visit_expr(expr);
            self.annotation = enclosing;
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match expr {
            Expr::Named(named) => {
                if self.in_iterable {
                    self.record(
                        expr.range(),
                        "assignment expression cannot be used in a comprehension iterable expression",
                        false,
                    );
                } else if let Expr::Name(name) = named.target.as_ref()
                    && !self.comprehensions.is_empty()
                {
                    let id = &name.id;
                    let symbol =
                        crate::private_name(self.private, id).unwrap_or_else(|| id.clone());
                    if self
                        .comprehensions
                        .iter()
                        .any(|scope| scope.targets.contains(&symbol))
                    {
                        self.record(name.range(), format!("assignment expression cannot rebind comprehension iteration variable '{id}'"), false);
                    } else if let Some(reason) = self.forbidden_binding {
                        self.record(name.range(), reason, false);
                    } else {
                        self.comprehensions
                            .last_mut()
                            .unwrap()
                            .writes
                            .insert(symbol);
                        if self.in_target {
                            self.inner_loop_conflict(name);
                        }
                    }
                }
                self.visit_expr(&named.value);
            }
            Expr::Name(name) if self.in_target => {
                let scope = self
                    .comprehensions
                    .last_mut()
                    .expect("comprehension target");
                let symbol =
                    crate::private_name(self.private, &name.id).unwrap_or_else(|| name.id.clone());
                if scope.writes.contains(&symbol) {
                    self.inner_loop_conflict(name);
                } else if name.ctx == ast::ExprContext::Store {
                    scope.targets.insert(symbol);
                }
            }
            Expr::Yield(_) | Expr::YieldFrom(_) if !self.annotation => {
                if let Some(message) = self.comprehension_yield_error {
                    self.record(expr.range(), message, false);
                } else if let Some(body) = &mut self.current {
                    body.yields = true;
                    if expr.is_yield_from_expr() {
                        self.record(expr.range(), "'yield from' inside async function", true);
                    }
                } else if self.outer_iterable && !self.function {
                    self.record(
                        expr.range(),
                        if expr.is_yield_from_expr() {
                            "'yield from' outside function"
                        } else {
                            "'yield' outside function"
                        },
                        true,
                    );
                }
                visitor::walk_expr(self, expr);
            }
            Expr::Await(_)
                if !self.annotation
                    && self.outer_iterable
                    && self.current.is_none()
                    && self.comprehension_yield_error.is_none() =>
            {
                self.record(
                    expr.range(),
                    if self.function {
                        "'await' outside async function"
                    } else {
                        "'await' outside function"
                    },
                    false,
                );
            }
            Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                // Defaults belong to the parent; a lambda body is an ordinary
                // function, even when the expression appears in an async scope.
                let mut lambda_scope = Self {
                    has_named_bindings: self.has_named_bindings,
                    private: self.private,
                    annotation: self.annotation,
                    function: true,
                    in_iterable: self.in_iterable,
                    ..Self::default()
                };
                lambda_scope.visit_expr(&lambda.body);
                if let Some(e) = lambda_scope.error {
                    self.record(e.range, e.message, e.location_in_args);
                }
            }
            expr if let Some(plan) = Plan::from_expr(expr) => self.comprehension(expr, plan),
            _ => visitor::walk_expr(self, expr),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comprehension_assignment_conflicts_point_to_the_symbol() {
        for (source, name, message) in [
            (
                "print(any(0 for values[(saved := 1)] in [4, 5]))",
                "saved",
                "comprehension inner loop cannot rebind assignment expression target 'saved'",
            ),
            (
                "[0 for i, a[(i := 1)] in rows]",
                "i :=",
                "assignment expression cannot rebind comprehension iteration variable 'i'",
            ),
            (
                "[0 for a[(i := 1)] in (rows := [])]",
                "rows := []",
                "assignment expression cannot be used in a comprehension iterable expression",
            ),
        ] {
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            let error = check(parsed.syntax().into(), true).unwrap();
            let range = error.range;
            assert_eq!(error.message, message);
            assert_eq!(range.start().to_usize(), source.find(name).unwrap());
            assert_eq!(&source[range], name.trim_end_matches(" :="));
        }
        let legal = "[i for i, a[(lambda: (k := 1))()] in rows if (keep := i)]";
        let parsed = ruff_python_parser::parse_module(legal).unwrap();
        assert!(check(parsed.syntax().into(), true).is_none());
    }

    #[test]
    fn comprehension_bindings_use_the_defining_scope() {
        for (source, expected) in [
            ("def f(x: [0 for a[(saved := 1)] in []]): pass", "saved"),
            (
                "from __future__ import annotations\nx: [0 for a[(saved := 1)] in []]",
                "saved",
            ),
            (
                "class C:\n def f(self): return [(_C__item := 0) for __item in [2]]",
                "_C__item",
            ),
            (
                "@wrap([0 for a[(decor := 1)] in []])\ndef f(x=[0 for a[(default := 1)] in []]): pass",
                "default",
            ),
        ] {
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            let error = check(parsed.syntax().into(), true).unwrap();
            assert_eq!(&source[error.range], expected);
            assert!(error.message.ends_with(&format!("'{expected}'")));
        }
    }
}
