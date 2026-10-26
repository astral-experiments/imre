//! Validate forbidden bindings before omitting an optimized assertion.
use super::{ast, error};
use ast::visitor::{Visitor, walk_expr, walk_parameter};

pub(super) fn validate(source: &str, assertion: &ast::StmtAssert) -> Result<(), String> {
    struct Scan<'src> {
        source: &'src str,
        error: Option<String>,
    }
    impl<'ast> Visitor<'ast> for Scan<'_> {
        fn visit_expr(&mut self, expr: &'ast ast::Expr) {
            let forbidden = match expr {
                ast::Expr::Name(name) => {
                    name.ctx != ast::ExprContext::Load && name.id == "__debug__"
                }
                ast::Expr::Attribute(attr) => {
                    attr.ctx != ast::ExprContext::Load && attr.attr.as_str() == "__debug__"
                }
                _ => false,
            };
            if forbidden {
                self.error = Some(error(self.source, expr, "cannot assign to __debug__"));
            } else if self.error.is_none() {
                walk_expr(self, expr);
            }
        }
        fn visit_parameter(&mut self, parameter: &'ast ast::Parameter) {
            if parameter.name.as_str() == "__debug__" {
                self.error = Some(error(self.source, parameter, "cannot assign to __debug__"));
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
