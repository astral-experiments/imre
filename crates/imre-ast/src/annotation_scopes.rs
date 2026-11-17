//! Correct scope boundaries for deferred Python annotation expressions.

use ruff_python_ast::{AnyNodeRef, ModModule, find_node::covering_node};
use ruff_python_parser::semantic_errors::{
    InvalidExpressionKind, InvalidExpressionPosition, SemanticSyntaxError, SemanticSyntaxErrorKind,
};
use ruff_text_size::Ranged;

/// A lambda's body executes in its own function scope; its defaults remain part
/// of the enclosing annotation. Ignore only a named-expression annotation error
/// whose exact syntax node is inside such a body.
pub fn named_expression_in_lambda_body(module: &ModModule, error: &SemanticSyntaxError) -> bool {
    if !matches!(
        error.kind,
        SemanticSyntaxErrorKind::InvalidExpression(
            InvalidExpressionKind::NamedExpr,
            InvalidExpressionPosition::TypeAnnotation
        )
    ) {
        return false;
    }
    let nodes = covering_node(module.into(), error.range);
    if !matches!(nodes.node(), AnyNodeRef::ExprNamed(named) if named.range() == error.range) {
        return false;
    }
    nodes.ancestors().any(|node| matches!(node, AnyNodeRef::ExprLambda(lambda) if lambda.body.range().contains_range(error.range)))
}
