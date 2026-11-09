//! Validate forbidden bindings before omitting an optimized assertion.
use super::ast;
use crate::frontend::diagnostic_message;
use ast::visitor::{Visitor, walk_expr, walk_parameter};

pub(super) fn validate(source: &str, assertion: &ast::StmtAssert) -> Result<(), String> {
    struct Scan<'src> {
        source: &'src str,
        error: Option<String>,
    }
    impl<'ast> Visitor<'ast> for Scan<'_> {
        fn visit_expr(&mut self, expr: &'ast ast::Expr) {
            if let Some(message) = imre_ast::executable::debug_binding_error(expr) {
                self.error = Some(diagnostic_message(self.source, expr, message));
            } else if self.error.is_none() {
                walk_expr(self, expr);
            }
        }
        fn visit_parameter(&mut self, parameter: &'ast ast::Parameter) {
            if parameter.name.as_str() == "__debug__" {
                self.error = Some(diagnostic_message(
                    self.source,
                    parameter,
                    "cannot assign to __debug__",
                ));
            } else if self.error.is_none() {
                walk_parameter(self, parameter);
            }
        }
    }
    let mut scan = Scan {
        source,
        error: None,
    };
    scan.visit_expr(&assertion.test);
    if let Some(message) = &assertion.msg {
        scan.visit_expr(message);
    }
    scan.error.map_or(Ok(()), Err)
}
