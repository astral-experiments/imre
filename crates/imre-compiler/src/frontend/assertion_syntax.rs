//! Validate forbidden bindings before omitting an optimized assertion.
use super::ast;
use crate::frontend::diagnostic_message;

pub(super) fn validate(source: &str, assertion: &ast::StmtAssert) -> Result<(), String> {
    match imre_ast::executable::assertion_binding_error(assertion) {
        Some((range, message)) => Err(diagnostic_message(source, &range, message)),
        None => Ok(()),
    }
}
