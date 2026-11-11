//! Collect code-object metadata for implicit comprehension scopes.

use imre_ast::comprehension::BodyOperand;

use super::*;

impl Program {
    /// Describe the implicit generator function with its outer iterator parameter.
    pub(super) fn lower_genexpr(&mut self, genexpr: &ast::ExprGenerator) -> Result<()> {
        let plan = Plan::generator(genexpr);
        self.kind = if plan.body_suspends() {
            FunctionKind::AsyncGenerator
        } else {
            FunctionKind::Generator
        };
        self.lower_comprehension_body(&plan)?;
        self.locals.insert(".0".into());
        Ok(())
    }

    /// Visit the outer iterable before entering the implicit comprehension scope.
    pub(super) fn lower_eager_comprehension(
        &mut self,
        comp: &Plan<'_>,
        operand: &OperandPlan,
    ) -> Result<()> {
        let targets = comp.try_target_names().map_err(invalid_target)?;
        self.record_expression(comp.outer.clone(), operand.function)?;
        self.items.push(SourceItem::EnterComprehension(targets));
        self.lower_comprehension_body(comp)?;
        self.items.push(SourceItem::LeaveComprehension);
        Ok(())
    }

    fn lower_comprehension_body(&mut self, comp: &Plan<'_>) -> Result<()> {
        for operand in comp.body_operands() {
            match operand {
                BodyOperand::Value(expr) => {
                    self.record_expression(expr.clone(), true)?;
                }
                BodyOperand::Target(target) => {
                    if matches!(comp.output, Output::Generator(_)) {
                        self.declare_target(target)?;
                    }
                    if self.kind.is_async() && imre_ast::comprehension::expression_suspends(target)
                    {
                        self.lower_suspended_target(target)?;
                    } else {
                        self.items.push(SourceItem::Discard(target.clone()));
                    }
                }
            }
        }
        Ok(())
    }

    fn lower_suspended_target(&mut self, target: &ast::Expr) -> Result<()> {
        if !imre_ast::comprehension::expression_suspends(target) {
            self.items.push(SourceItem::Discard(target.clone()));
            return Ok(());
        }
        match target {
            ast::Expr::Starred(star) => self.lower_suspended_target(&star.value)?,
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => {
                for item in elts {
                    self.lower_suspended_target(item)?;
                }
            }
            ast::Expr::Attribute(field) => {
                self.record_expression(*field.value.clone(), true)?;
                self.items.push(SourceItem::Name(field.attr.clone()));
            }
            ast::Expr::Subscript(item) => {
                for expr in [&item.value, &item.slice] {
                    self.record_expression(*expr.clone(), true)?;
                }
            }
            _ => unreachable!("validated comprehension target"),
        }
        Ok(())
    }
}
