//! A type alias evaluator has its own Python call frame and a lazy return expression.
use super::*;

pub(crate) fn type_alias_body(
    alias: &ast::StmtTypeAlias,
    source: values::RegisteredSource<'_>,
) -> Option<RegisteredFunction> {
    let value = values::Region::Suite.operand(&alias.value, source)?;
    let format = Statement::Expr(operation(
        Builtin::RegisteredAnnotationFormat,
        Type::None,
        vec![values::name_binding(".format")],
    ));
    Some(RegisteredFunction::expression_with_prologue(
        alias.value.range(),
        value,
        Some(format),
    ))
}
