//! Declaration dependencies of registered IR in evaluation order.
use crate::dynamic::literal::LiteralSites;
use crate::ir::{self, Builtin, Expr, ExprKind, Statement};
use ruff_text_size::{TextRange, TextSize};

/// A declaration used by an operand in a registered frame.
#[derive(Clone, Copy)]
pub(crate) enum RequiredSite {
    Definition(TextSize),
    /// The body follows header operands supplied by the enclosing registered suite.
    DefinitionAfterInputs(TextSize),
    ClassNamespace(TextSize),
    Lambda(TextSize, bool),
    Generator(TextRange),
    Alias(TextSize),
    AnnotationString(TextSize),
    ExecLiteral(TextRange),
}

fn visit_expression_site(
    expr: &Expr,
    literals: Option<&LiteralSites>,
    visit: &mut impl FnMut(RequiredSite),
) {
    let call = match &expr.kind {
        ExprKind::Invoke { args, .. } => args.site.python().map(|site| site.call.range),
        ExprKind::Builtin(Builtin::RegisteredCall(call), _) => Some(call.range),
        ExprKind::Builtin(op, operands) => {
            visit(match op {
                Builtin::RegisteredDefinition(site) => RequiredSite::definition(site, operands),
                Builtin::RegisteredClassNamespace(site, _) => RequiredSite::ClassNamespace(*site),
                Builtin::RegisteredLambda(site, scoped) => RequiredSite::Lambda(*site, *scoped),
                Builtin::RegisteredGenerator(site, ..) => RequiredSite::Generator(*site),
                Builtin::RegisteredTypeAlias(site) => RequiredSite::Alias(*site),
                Builtin::BindAnnotationExpression { site, .. } => {
                    RequiredSite::AnnotationString(*site)
                }
                _ => return,
            });
            return;
        }
        _ => return,
    };
    if let Some(range) = call
        && literals.is_some_and(|sites| sites.contains(range))
    {
        visit(RequiredSite::ExecLiteral(range));
    }
}

impl RequiredSite {
    /// Class bodies follow any header inputs evaluated by their enclosing suite.
    pub(crate) fn definition(site: &ir::DefinitionSite, header: &[Expr]) -> Self {
        if matches!(site, ir::DefinitionSite::Class { .. })
            && header.iter().any(|value| {
                matches!(value.kind, ExprKind::Builtin(Builtin::RegisteredInput(_), _))
            })
        {
            Self::DefinitionAfterInputs(site.start())
        } else {
            Self::Definition(site.start())
        }
    }

    pub(crate) fn visit_operand(
        value: &Expr,
        literals: Option<&LiteralSites>,
        visit: &mut impl FnMut(RequiredSite),
    ) {
        ir::walk_operand_expressions(value, &mut |expr| {
            visit_expression_site(expr, literals, visit)
        });
    }

    #[cfg(test)]
    pub(crate) fn visit(
        statements: &[Statement],
        literals: Option<&LiteralSites>,
        visit: &mut impl FnMut(RequiredSite),
    ) -> bool {
        Self::visit_block(statements, false, literals, visit)
    }

    /// A block stops emitting after a transfer. Unreachable declaration bodies must
    /// not compile (or raise diagnostics) merely because their identity is in the IR.
    pub(super) fn visit_block(
        statements: &[Statement],
        physical: bool,
        literals: Option<&LiteralSites>,
        visit: &mut impl FnMut(RequiredSite),
    ) -> bool {
        for stmt in statements {
            match stmt {
                Statement::If(test, yes, no) => {
                    Self::visit_operand(test, literals, visit);
                    let yes_continues = Self::visit_block(yes, physical, literals, visit);
                    let no_continues = Self::visit_block(no, physical, literals, visit);
                    let literal = if physical {
                        ir::control::boolean_literal(test)
                    } else {
                        None
                    };
                    let continues = match literal {
                        Some(true) => yes_continues,
                        Some(false) => no_continues,
                        None => yes_continues || no_continues,
                    };
                    if !continues {
                        return false;
                    }
                }
                Statement::While(test, body, otherwise) => {
                    Self::visit_operand(test, literals, visit);
                    Self::visit_block(body, physical, literals, visit);
                    Self::visit_block(otherwise, physical, literals, visit);
                }
                Statement::Scope { body, .. } => {
                    if !Self::visit_block(body, physical, literals, visit) {
                        return false;
                    }
                }
                _ => {
                    ir::walk_expressions(std::slice::from_ref(stmt), &mut |expr| {
                        visit_expression_site(expr, literals, visit)
                    });
                    if matches!(
                        stmt,
                        Statement::Return(_)
                            | Statement::Break(_)
                            | Statement::Continue(_)
                            | Statement::Exhaust
                    ) {
                        return false;
                    }
                }
            }
        }
        true
    }
}
