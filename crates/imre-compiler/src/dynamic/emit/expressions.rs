use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::FrameEmitter;
use super::{Function, permanent};
use crate::frontend::{PreparedTemplate, SourceOperand, TemplateField, prepare_boolean};
use crate::ir::control::Context as BooleanContext;
use crate::llvm::operations::branches::{Logical, LogicalEmitter, LogicalInput};
use crate::llvm::operations::{CheckedBody, CheckedValue, ComparisonChain, ValueJoin};

impl LogicalEmitter<SourceOperand<'_>> for Function<'_, '_> {
    fn logical_input<'a>(
        &mut self,
        leaf: &'a SourceOperand<'_>,
        context: BooleanContext,
    ) -> LogicalInput<'a> {
        match leaf {
            SourceOperand::Checked(value) => {
                self.prepare_checked_value(value);
                LogicalInput::Checked(value)
            }
            SourceOperand::Residual(expr) if context == BooleanContext::Condition => {
                LogicalInput::Predicate(self.test_operand(expr))
            }
            SourceOperand::Residual(expr) => LogicalInput::Handle(self.expr(expr)),
        }
    }

    fn with_logical_body<R>(&mut self, emit: impl FnOnce(&mut CheckedBody) -> R) -> R {
        self.frame.with_checked_frame(&self.sites, None, |frame| {
            emit(&mut CheckedBody::registered(frame))
        })
    }
}

impl Function<'_, '_> {
    pub(super) fn expr(&mut self, expr: &ast::Expr) -> String {
        self.with_expression_range(expr.range(), |code| {
            code.registered_region(expr, false)
                .unwrap_or_else(|| code.located_expr(expr))
        })
    }

    fn registered_region(&mut self, expr: &ast::Expr, condition: bool) -> Option<String> {
        use crate::frontend::ExpressionSelection;
        let selected = self
            .source
            .statement_sites
            .expressions
            .get(expr)
            .or_else(|| {
                expr.as_lambda_expr()
                    .map(|lambda| &self.source.lambdas[&lambda.start()].binding)
            });
        if let Some(ExpressionSelection::Checked(value)) = selected {
            self.prepare_checked_value(value);
            return Some(
                self.frame
                    .registered_expression(value, &self.sites, condition),
            );
        }
        if let Some(ExpressionSelection::Refused(range, reason)) = selected {
            return Some(self.with_expression_range(*range, |code| code.build_diagnostic(reason)));
        }
        let operand = match selected {
            Some(ExpressionSelection::Operand(operand)) => operand.as_ref(),
            _ => match self.source.strings.templates.get(&expr.range()) {
                Some(PreparedTemplate::Checked(operand)) => operand,
                _ => return None,
            },
        };
        self.prepare_checked_value(&operand.value);
        Some(self.frame.prepared_operand(operand, &self.sites, condition))
    }

    pub(super) fn with_expression_range(
        &mut self,
        range: ruff_text_size::TextRange,
        emit: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let previous = std::mem::replace(&mut self.current_range, range);
        let value = emit(self);
        self.current_range = previous;
        value
    }

    pub(super) fn located_expr(&mut self, expr: &ast::Expr) -> String {
        self.located_operand(expr, false)
    }

    fn located_operand(&mut self, expr: &ast::Expr, condition: bool) -> String {
        let physical = self.source.statement_sites.expressions.physical(expr);
        let position = physical.map_or_else(
            || {
                imre_ast::expression_position(
                    expr,
                    &self.source.method_imports,
                    Some(&self.source.private_names),
                )
            },
            |inputs| inputs.position,
        );
        let previous = self.enter_expression(expr.range(), position);
        let comparison = self
            .source
            .statement_sites
            .expressions
            .comparison(expr, self.native_suspension);
        let value = if let Some((ops, inputs)) = comparison {
            let result = ComparisonChain::emit_registered(
                self,
                ops,
                condition,
                |code| &mut code.frame,
                inputs,
                Self::source_operand,
            );
            if condition {
                result.into_condition()
            } else {
                result.into_handle()
            }
        } else if let Some(inputs) = physical {
            self.physical_value(expr, inputs)
        } else if let Some((edges, inputs)) = self.source.statement_sites.expressions.value(expr) {
            let value = Logical::value(&mut self.frame.body, edges, crate::ir::Type::PythonObject)
                .emit(self, &inputs)
                .expect("a selected operand reaches the join");
            self.frame
                .with_checked_frame(&self.sites, None, |mut frame| frame.materialize(&value))
        } else if let Some([test, yes, no]) =
            self.source.statement_sites.expressions.conditional(expr)
        {
            ValueJoin::select(
                self,
                |code| {
                    code.flush_check();
                    &mut code.frame.body
                },
                |code, yes, no| match test {
                    SourceOperand::Checked(test) => {
                        code.prepare_checked_value(test);
                        code.frame.registered_branch(test, &code.sites, yes, no);
                    }
                    SourceOperand::Residual(test) => code.branch_expr(test, yes, no),
                },
                |code| CheckedValue::handle(code.source_operand(yes)),
                |code| CheckedValue::handle(code.source_operand(no)),
            )
            .expect("value-producing conditional")
            .into_handle()
        } else {
            self.expr_inner(expr)
        };
        self.leave_expression(previous);
        value
    }

    fn physical_value(
        &mut self,
        node: &ast::Expr,
        inputs: &crate::frontend::PhysicalInputs,
    ) -> String {
        use crate::ir::{Builtin, ExprKind};
        let mut operands = inputs.operands(node, self.native_suspension);
        match &inputs.operation.kind {
            ExprKind::Builtin(Builtin::RegisteredUnary(op), _) => {
                let scope = self.begin_unary();
                let operand = self.source_operand(operands.next().expect("unary operand"));
                let result = self.frame.unary_handle(*op, operand);
                self.finish_unary(scope, *op, result).into_handle()
            }
            ExprKind::Builtin(
                op @ (Builtin::RegisteredCall(_) | Builtin::RegisteredAttribute { .. }),
                _,
            ) => {
                let target = operands.next().expect("object operation target");
                let args: Vec<_> = operands.collect();
                self.object_operation(op, &target, args.iter(), |code, child| {
                    code.source_operand(*child)
                })
            }
            _ => {
                let handles: Vec<_> = operands.map(|child| self.source_operand(child)).collect();
                self.frame
                    .registered_with_inputs(&inputs.operation, &handles, &self.sites)
            }
        }
    }

    fn expr_inner(&mut self, expr: &ast::Expr) -> String {
        if let Some(PreparedTemplate::Mixed { segments, fields }) =
            self.source.strings.templates.get(&expr.range())
        {
            return self.template(segments, fields, |code, field| {
                use std::ops::ControlFlow;
                let value = match field {
                    TemplateField::Checked(operand) => {
                        code.prepare_checked_value(&operand.value);
                        code.frame.prepared_operand(operand, &code.sites, false)
                    }
                    TemplateField::External {
                        value,
                        interpolation,
                    } => {
                        let input = code.expr(value);
                        code.frame
                            .registered_with_inputs(interpolation, &[input], &code.sites)
                    }
                    TemplateField::Unsupported(reason) => {
                        return ControlFlow::Break(code.unsupported(reason));
                    }
                };
                ControlFlow::Continue(value)
            });
        }
        if self.native_suspension
            && let Some((kind, value)) = crate::ir::SuspendValue::operand(expr)
        {
            return self.suspend_value(kind, expr.range(), |code| {
                value.map_or_else(|| permanent::NONE.to_string(), |value| code.expr(value))
            });
        }
        match expr {
            ast::Expr::TString(_) => self.unsupported("t-string expression spelling"),
            ast::Expr::FString(_) => {
                self.build_diagnostic("cannot compile this formatted string ahead of time")
            }
            ast::Expr::Generator(_) => {
                self.build_diagnostic("cannot compile this generator expression ahead of time")
            }
            ast::Expr::DictComp(display) if display.key.is_none() => {
                self.unsupported("dict unpacking comprehension")
            }
            ast::Expr::ListComp(_) | ast::Expr::SetComp(_) | ast::Expr::DictComp(_) => {
                self.build_diagnostic("cannot compile this comprehension ahead of time")
            }
            _ => self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(expr).kind())),
        }
    }

    fn source_operand(&mut self, operand: SourceOperand<'_>) -> String {
        match operand {
            SourceOperand::Checked(value) => self.registered_value(value),
            SourceOperand::Residual(expr) => self.expr(expr),
        }
    }

    /// The enclosing source operation supplies evaluated leaves. Its branch graph
    /// and ownership policy are the same ones used by a wholly checked suite.
    pub(super) fn branch_expr(&mut self, expr: &ast::Expr, yes: &str, no: &str) {
        if let Some(crate::frontend::ExpressionSelection::Checked(condition)) =
            self.source.statement_sites.expressions.get(expr)
        {
            self.prepare_checked_value(condition);
            self.frame
                .registered_branch(condition, &self.sites, yes, no);
            return;
        }
        let (edges, inputs) = prepare_boolean(
            expr,
            BooleanContext::Condition,
            &self.source.statement_sites.expressions,
        );
        Logical::branch(&mut self.frame.body, edges, yes, no).emit(self, &inputs);
    }

    fn test_operand(&mut self, expr: &ast::Expr) -> String {
        self.test_with_temporaries(|f| {
            if expr.is_compare_expr() {
                f.registered_region(expr, true).unwrap_or_else(|| {
                    f.with_expression_range(expr.range(), |f| f.located_operand(expr, true))
                })
            } else {
                let value = f.expr(expr);
                f.truth_at(&value, Some(expr.range()))
            }
        })
    }
}
