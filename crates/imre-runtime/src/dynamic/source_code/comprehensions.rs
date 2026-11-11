//! Collect code-object metadata for implicit comprehension scopes.

use imre_ast::comprehension::BodyOperand;

use super::*;

impl Program {
    /// Describe the implicit generator function with its outer iterator parameter.
    pub(super) fn lower_genexpr(
        &mut self,
        genexpr: &ast::ExprGenerator,
        line_at: &impl Fn(usize) -> u32,
    ) -> Result<Box<ast::Parameters>> {
        let plan = Plan::generator(genexpr);
        self.kind = if plan.body_suspends() {
            FunctionKind::AsyncGenerator
        } else {
            FunctionKind::Generator
        };
        self.lower_comprehension_body(&plan, line_at)?;
        let range = genexpr.range();
        let mut params = ast::Parameters::default();
        params.args.push(ast::ParameterWithDefault {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            default: None,
            parameter: ast::Parameter {
                range,
                node_index: ast::AtomicNodeIndex::NONE,
                name: ast::Identifier::new(".0", range),
                annotation: None,
            },
        });
        Ok(Box::new(params))
    }

    /// Visit the outer iterable before entering the implicit comprehension scope.
    pub(super) fn lower_eager_comprehension(
        &mut self,
        comp: &Plan<'_>,
        operand: &OperandPlan,
    ) -> Result<()> {
        let line = operand.line;
        let targets = comp.try_target_names().map_err(invalid_target)?;
        self.record_expression(comp.outer.clone(), line, operand.function)?;
        self.items
            .push((line, SourceItem::EnterComprehension(targets)));
        self.lower_comprehension_body(comp, &|_| line)?;
        self.items.push((line, SourceItem::LeaveComprehension));
        Ok(())
    }

    fn lower_comprehension_body(
        &mut self,
        comp: &Plan<'_>,
        line_at: &impl Fn(usize) -> u32,
    ) -> Result<()> {
        for operand in comp.body_operands() {
            match operand {
                BodyOperand::Value(expr) => {
                    self.record_expression(expr.clone(), line_at(expr.start().to_usize()), true)?;
                }
                BodyOperand::Target(target) => {
                    if matches!(comp.output, Output::Generator(_)) {
                        self.declare_target(target)?;
                    }
                    let line = line_at(target.start().to_usize());
                    if self.kind.is_async() && imre_ast::comprehension::expression_suspends(target)
                    {
                        self.lower_suspended_target(target, line)?;
                    } else {
                        self.items.push((line, SourceItem::Discard(target.clone())));
                    }
                }
            }
        }
        Ok(())
    }

    fn lower_suspended_target(&mut self, target: &ast::Expr, line: u32) -> Result<()> {
        if !imre_ast::comprehension::expression_suspends(target) {
            self.items.push((line, SourceItem::Discard(target.clone())));
            return Ok(());
        }
        match target {
            ast::Expr::Starred(star) => self.lower_suspended_target(&star.value, line)?,
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => {
                for item in elts {
                    self.lower_suspended_target(item, line)?;
                }
            }
            ast::Expr::Attribute(field) => {
                self.record_expression(*field.value.clone(), line, true)?;
                self.items
                    .push((line, SourceItem::Name(field.attr.clone())));
            }
            ast::Expr::Subscript(item) => {
                for expr in [&item.value, &item.slice] {
                    self.record_expression(*expr.clone(), line, true)?;
                }
            }
            _ => unreachable!("validated comprehension target"),
        }
        Ok(())
    }
}
