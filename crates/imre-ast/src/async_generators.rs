//! Validate suspension in functions, comprehensions and their enclosing scopes.
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

use crate::comprehension::{Output, Plan};

#[derive(Clone, Copy)]
pub struct Error {
    pub range: TextRange,
    pub message: &'static str,
    /// Code-generation errors include the location tuple in SyntaxError.args.
    /// Symbol-table errors set the location attributes separately.
    pub location_in_args: bool,
}

pub fn syntax_error(module: &ast::ModModule) -> Option<(TextRange, &'static str)> {
    check(module.into()).map(|error| (error.range, error.message))
}

pub fn check(module: ast::ModRef<'_>) -> Option<Error> {
    let mut check = Check::default();
    match module {
        ast::ModRef::Module(module) => check.visit_body(&module.body),
        ast::ModRef::Expression(expr) => check.visit_expr(&expr.body),
    }
    let error = check.error?;
    if matches!(module, ast::ModRef::Module(m) if crate::annotation_scope_error(&m.body).is_some())
        || crate::repeated_keyword(module.into()).is_some_and(|kw| kw.start() < error.range.start())
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
struct Check {
    current: Option<AsyncBody>,
    function: bool,
    comprehension_yield_error: Option<&'static str>,
    outer_iterable: bool,
    error: Option<Error>,
}

impl Check {
    fn record(&mut self, range: TextRange, message: &'static str, location_in_args: bool) {
        if self.error.is_none_or(|previous| {
            previous.location_in_args
                && (!location_in_args || range.start() < previous.range.start())
        }) {
            self.error = Some(Error {
                range,
                message,
                location_in_args,
            });
        }
    }

    fn scope(&mut self, body: &[Stmt], function: bool, is_async: bool) {
        let outer = std::mem::replace(&mut self.current, is_async.then(AsyncBody::default));
        let enclosing_function = std::mem::replace(&mut self.function, function);
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.visit_body(body));
        self.function = enclosing_function;
        if let Some(AsyncBody {
            yields: true,
            valued_return: Some(range),
        }) = std::mem::replace(&mut self.current, outer)
        {
            self.record(range, "'return' with value in async generator", true);
        }
    }

    fn comprehension(&mut self, expr: &Expr, plan: Plan<'_>) {
        let previous = self.error.take();
        let outer = std::mem::replace(&mut self.outer_iterable, true);
        self.visit_expr(plan.outer);
        self.outer_iterable = false;
        let parent = self.comprehension_yield_error.replace(match plan.output {
            Output::Generator(_) => "'yield' inside generator expression",
            Output::List(_) => "'yield' inside list comprehension",
            Output::Set(_) => "'yield' inside set comprehension",
            Output::Dict(..) => "'yield' inside dict comprehension",
        });
        plan.visit_body(|operand| self.visit_expr(operand));
        self.comprehension_yield_error = parent;
        self.outer_iterable = outer;
        // A deferred frame may suspend anywhere. Nested eager displays propagate
        // their suspension to the outer display, whose scope determines legality.
        if !matches!(plan.output, Output::Generator(_))
            && parent.is_none()
            && self.current.is_none()
            && plan.body_suspends()
            && self.error.is_none_or(|e| e.location_in_args)
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

impl<'a> Visitor<'a> for Check {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                for decorator in &f.decorator_list {
                    self.visit_decorator(decorator);
                }
                self.visit_parameters(&f.parameters);
                self.scope(&f.body, true, f.is_async);
            }
            Stmt::ClassDef(c) => {
                for decorator in &c.decorator_list {
                    self.visit_decorator(decorator);
                }
                if c.type_params.is_none()
                    && let Some(args) = &c.arguments
                {
                    self.visit_arguments(args);
                }
                self.scope(&c.body, false, false);
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

    fn visit_annotation(&mut self, _: &'a Expr) {}

    fn visit_expr(&mut self, expr: &'a Expr) {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match expr {
            Expr::Yield(_) | Expr::YieldFrom(_) => {
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
                if self.outer_iterable
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
                    function: true,
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
