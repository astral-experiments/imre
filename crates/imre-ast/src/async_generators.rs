//! Semantic errors from yielding or returning within an asynchronous function's own scope.
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

/// Find an illegal `yield from` or a valued return in an async generator, even in dead code.
pub fn syntax_error(module: &ast::ModModule) -> Option<(TextRange, &'static str)> {
    let mut check = Check::default();
    check.visit_body(&module.body);
    let (range, message) = check.error?;
    if crate::type_alias_error(&module.body).is_some()
        || crate::repeated_keyword(module.into()).is_some_and(|kw| kw.start() < range.start())
    {
        return None;
    }
    Some((range, message))
}

#[derive(Default)]
struct AsyncBody {
    yields: bool,
    valued_return: Option<TextRange>,
}

#[derive(Default)]
struct Check {
    current: Option<AsyncBody>,
    error: Option<(TextRange, &'static str)>,
}

impl Check {
    fn outer_iterable(&mut self, clauses: &[ast::Comprehension]) {
        if let Some(first) = clauses.first() {
            self.visit_expr(&first.iter);
        }
    }

    fn record(&mut self, range: TextRange, message: &'static str) {
        if self.error.is_none_or(|(previous, _)| range.start() < previous.start())
        {
            self.error = Some((range, message));
        }
    }

    fn scope(&mut self, body: &[Stmt], is_async: bool) {
        let outer = std::mem::replace(&mut self.current, is_async.then(AsyncBody::default));
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.visit_body(body));
        if let Some(AsyncBody {
            yields: true,
            valued_return: Some(range),
        }) = std::mem::replace(&mut self.current, outer)
        {
            self.record(range, "'return' with value in async generator");
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
                self.scope(&f.body, f.is_async);
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
                self.scope(&c.body, false);
            }
            Stmt::TypeAlias(_) => {}
            _ => {
                if let Stmt::Return(r) = stmt
                    && r.value.is_some()
                    && let Some(body) = &mut self.current
                {
                    body.valued_return.get_or_insert(r.range());
                }
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || visitor::walk_stmt(self, stmt));
            }
        }
    }

    fn visit_annotation(&mut self, _: &'a Expr) {}

    fn visit_expr(&mut self, expr: &'a Expr) {
        let Some(body) = &mut self.current else {
            return;
        };
        match expr {
            Expr::Yield(y) => {
                body.yields = true;
                if let Some(value) = &y.value {
                    self.visit_expr(value);
                }
            }
            Expr::YieldFrom(y) => {
                body.yields = true;
                self.record(y.range(), "'yield from' inside async function");
            }
            Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
            }
            // Only the outermost iterable is evaluated in the enclosing scope.
            Expr::Generator(g) => self.outer_iterable(&g.generators),
            Expr::ListComp(c) => self.outer_iterable(&c.generators),
            Expr::SetComp(c) => self.outer_iterable(&c.generators),
            Expr::DictComp(c) => self.outer_iterable(&c.generators),
            _ => stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || visitor::walk_expr(self, expr)),
        }
    }
}
