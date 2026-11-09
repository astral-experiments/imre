//! CPython 3.14 `code.co_flags` values shared by the compiler and boxed runtime.
use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

pub const OPTIMIZED: u32 = 1;
pub const NEWLOCALS: u32 = 2;
pub const VARARGS: u32 = 4;
pub const VARKEYWORDS: u32 = 8;
pub const NESTED: u32 = 0x10;
pub const GENERATOR: u32 = 0x20;
pub const NOFREE: u32 = 0x40;
pub const COROUTINE: u32 = 0x80;
pub const ITERABLE_COROUTINE: u32 = 0x100;
pub const ASYNC_GENERATOR: u32 = 0x200;
pub const FUTURE_BARRY_AS_BDFL: u32 = 0x0040_0000;
pub const FUTURE_ANNOTATIONS: u32 = 0x0100_0000;
pub const HAS_DOCSTRING: u32 = 0x0400_0000;
pub const METHOD: u32 = 0x0800_0000;

/// Names exported by CPython 3.14 inspect from dis.COMPILER_FLAG_NAMES.
pub const INSPECT_FLAGS: &[(&str, u32)] = &[
    ("CO_OPTIMIZED", OPTIMIZED),
    ("CO_NEWLOCALS", NEWLOCALS),
    ("CO_VARARGS", VARARGS),
    ("CO_VARKEYWORDS", VARKEYWORDS),
    ("CO_NESTED", NESTED),
    ("CO_GENERATOR", GENERATOR),
    ("CO_NOFREE", NOFREE),
    ("CO_COROUTINE", COROUTINE),
    ("CO_ITERABLE_COROUTINE", ITERABLE_COROUTINE),
    ("CO_ASYNC_GENERATOR", ASYNC_GENERATOR),
    ("CO_HAS_DOCSTRING", HAS_DOCSTRING),
    ("CO_METHOD", METHOD),
];

/// Flags determined by a function's signature and lexical scope.
pub fn function_flags(params: Option<&ast::Parameters>, nested: bool, method: bool) -> u32 {
    let mut flags = OPTIMIZED | NEWLOCALS;
    if params.is_some_and(|p| p.vararg.is_some()) {
        flags |= VARARGS;
    }
    if params.is_some_and(|p| p.kwarg.is_some()) {
        flags |= VARKEYWORDS;
    }
    if nested {
        flags |= NESTED;
    }
    if method {
        flags |= METHOD;
    }
    flags
}

#[derive(Default)]
struct Yields(bool);
impl<'a> Visitor<'a> for Yields {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if !self.0 && !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_stmt(self, stmt)
            });
        }
    }
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        match expr {
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => self.0 = true,
            ast::Expr::Lambda(lambda) if !self.0 => {
                // Defaults run in the enclosing activation. Only the nested body is deferred.
                if let Some(parameters) = &lambda.parameters {
                    for parameter in parameters.iter_non_variadic_params() {
                        if let Some(default) = &parameter.default {
                            self.visit_expr(default);
                        }
                    }
                }
            }
            _ if !self.0 => stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_expr(self, expr)
            }),
            _ => {}
        }
    }
}

/// Whether a function body yields in its own scope.
pub fn is_generator(body: &[ast::Stmt]) -> bool {
    let mut yields = Yields::default();
    yields.visit_body(body);
    yields.0
}

/// Whether an expression-bodied function yields in its own scope.
pub fn yields_in_expression(expr: &ast::Expr) -> bool {
    let mut yields = Yields::default();
    yields.visit_expr(expr);
    yields.0
}
