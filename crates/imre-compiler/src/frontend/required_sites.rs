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
        ExprKind::Invoke { args, .. } => args.site.python().map(|site| site.range),
        ExprKind::Builtin(Builtin::RegisteredCall(call), _) => Some(call.range),
        ExprKind::Builtin(op, operands) => {
            visit(match op {
                Builtin::RegisteredDefinition(ir::DefinitionSite::Class { start, .. })
                    if operands.iter().any(|value| {
                        matches!(
                            value.kind,
                            ExprKind::Builtin(Builtin::RegisteredInput(_), _)
                        )
                    }) =>
                {
                    RequiredSite::DefinitionAfterInputs(*start)
                }
                Builtin::RegisteredDefinition(site) => RequiredSite::Definition(site.start()),
                Builtin::RegisteredClassNamespace(site, _) => RequiredSite::ClassNamespace(*site),
                Builtin::RegisteredLambda(site, scoped) => RequiredSite::Lambda(*site, *scoped),
                Builtin::RegisteredGenerator(site, _) => RequiredSite::Generator(*site),
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
    pub(crate) fn visit_operand(
        value: &Expr,
        literals: Option<&LiteralSites>,
        visit: &mut impl FnMut(RequiredSite),
    ) {
        ir::walk_operand_expressions(value, &mut |expr| {
            visit_expression_site(expr, literals, visit)
        });
    }

    /// A block stops emitting after a transfer. Unreachable declaration bodies must
    /// not compile (or raise diagnostics) merely because their identity is in the IR.
    pub(crate) fn visit(
        statements: &[Statement],
        literals: Option<&LiteralSites>,
        visit: &mut impl FnMut(RequiredSite),
    ) -> bool {
        for stmt in statements {
            match stmt {
                Statement::If(test, yes, no) => {
                    Self::visit_operand(test, literals, visit);
                    let yes_continues = Self::visit(yes, literals, visit);
                    let no_continues = Self::visit(no, literals, visit);
                    if !yes_continues && !no_continues {
                        return false;
                    }
                }
                Statement::While(test, body, otherwise) => {
                    Self::visit_operand(test, literals, visit);
                    Self::visit(body, literals, visit);
                    Self::visit(otherwise, literals, visit);
                }
                Statement::Scope { body, .. } => {
                    if !Self::visit(body, literals, visit) {
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
                            | Statement::Break
                            | Statement::Continue
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
