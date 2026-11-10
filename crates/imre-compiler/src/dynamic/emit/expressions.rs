use ruff_python_ast as ast;
use ruff_text_size::TextRange;

use super::FrameEmitter;
use super::Function;
use crate::frontend::{ExpressionInput, ExpressionSelection, LogicalOperand, SourceOperand};
use crate::ir::control::Context as BooleanContext;
use crate::llvm::operations::branches::{Logical, LogicalEmitter, LogicalInput};
use crate::llvm::operations::{CheckedBody, CheckedValue, ComparisonChain, ValueJoin};
use crate::llvm::registered::PreparedSuiteEmitter;

impl LogicalEmitter<LogicalOperand> for Function<'_, '_> {
    fn logical_input<'a>(
        &mut self,
        leaf: &'a LogicalOperand,
        context: BooleanContext,
    ) -> LogicalInput<'a> {
        match leaf.operand() {
            SourceOperand::Checked(value) => {
                self.prepare_checked_value(value);
                LogicalInput::Checked(value)
            }
            SourceOperand::Residual(expr) if context == BooleanContext::Condition => {
                LogicalInput::Predicate(self.test_operand(expr))
            }
            SourceOperand::Residual(expr) => LogicalInput::Handle(self.input_value(expr)),
        }
    }

    fn with_logical_body<R>(&mut self, emit: impl FnOnce(&mut CheckedBody) -> R) -> R {
        self.frame.with_checked_frame(&self.sites, None, |frame| {
            emit(&mut CheckedBody::registered(frame))
        })
    }
}

impl Function<'_, '_> {
    pub(super) fn input_value(&mut self, input: ExpressionInput<'_>) -> String {
        self.with_expression_range(input.range, |code| {
            code.registered_region(input.selected, false)
                .unwrap_or_else(|| code.located_operand(input, false))
        })
    }

    fn registered_region(
        &mut self,
        selected: &ExpressionSelection,
        condition: bool,
    ) -> Option<String> {
        match selected {
            ExpressionSelection::Checked(value) => {
                self.prepare_checked_value(value);
                Some(
                    self.frame
                        .registered_expression(value, &self.sites, condition),
                )
            }
            ExpressionSelection::Operand(operand) => {
                self.prepare_checked_value(&operand.value);
                Some(self.frame.prepared_operand(operand, &self.sites, condition))
            }
            ExpressionSelection::Refused(range, reason) => {
                Some(self.with_expression_range(*range, |code| code.build_diagnostic(reason)))
            }
            _ => None,
        }
    }

    pub(super) fn with_expression_range(
        &mut self,
        range: TextRange,
        emit: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let previous = std::mem::replace(&mut self.current_range, range);
        let value = emit(self);
        self.current_range = previous;
        value
    }

    fn located_operand(&mut self, input: ExpressionInput<'_>, condition: bool) -> String {
        let selected = input.selected;
        let range = input.range;
        let physical = selected.physical();
        let position = physical.map_or(range.start(), |inputs| inputs.position);
        let previous = self.enter_expression(range, position);
        let value = if let ExpressionSelection::Comparison { ops, operands, .. } = selected {
            let result = ComparisonChain::emit_registered(
                self,
                ops,
                condition,
                |code| &mut code.frame,
                operands.iter().map(|child| child.operand()),
                Self::source_operand,
            );
            if condition {
                result.into_condition()
            } else {
                result.into_handle()
            }
        } else if let ExpressionSelection::Suspension(kind, child) = selected {
            self.suspend_value(*kind, range, |code| code.input_value(child.as_input()))
        } else if let Some(inputs) = physical {
            self.physical_value(inputs)
        } else if let ExpressionSelection::Mixed {
            value: Some(plan), ..
        } = selected
        {
            let value = Logical::value(
                &mut self.frame.body,
                plan.edges.clone(),
                crate::ir::Type::PythonObject,
            )
            .emit(self, &plan.operands)
            .expect("a selected operand reaches the join");
            self.frame
                .with_checked_frame(&self.sites, None, |mut frame| frame.materialize(&value))
        } else if let ExpressionSelection::Mixed {
            parts: Some(parts), ..
        } = selected
        {
            let [test, yes, no] = parts.each_ref().map(|child| child.operand());
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
                    SourceOperand::Residual(test) => code.branch_input(test, yes, no),
                },
                |code| CheckedValue::handle(code.source_operand(yes)),
                |code| CheckedValue::handle(code.source_operand(no)),
            )
            .expect("value-producing conditional")
            .into_handle()
        } else {
            self.build_diagnostic("expression has no checked operand")
        };
        self.leave_expression(previous);
        value
    }

    fn physical_value(&mut self, inputs: &crate::frontend::PhysicalInputs) -> String {
        use crate::ir::{Builtin, ExprKind};
        let mut operands = inputs.operands();
        match &inputs.operation.kind {
            ExprKind::Builtin(Builtin::RegisteredUnary(_), _) => {
                crate::llvm::registered::Frame::prepared_operation(self, inputs)
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

    /// The enclosing source operation supplies evaluated leaves. Its branch graph
    /// and ownership policy are the same ones used by a wholly checked suite.
    pub(super) fn branch_input(&mut self, input: ExpressionInput<'_>, yes: &str, no: &str) {
        // Evaluating a lexical binding may create temporaries before its truth test.
        if input.kind == ast::NodeKind::ExprLambda {
            let bit = self.test_operand(input);
            self.branch(&bit, yes, no);
            return;
        }
        match input.selected {
            ExpressionSelection::Checked(condition) => {
                self.prepare_checked_value(condition);
                self.frame
                    .registered_branch(condition, &self.sites, yes, no);
            }
            ExpressionSelection::Mixed { branch, .. } => {
                Logical::branch(&mut self.frame.body, branch.edges.clone(), yes, no)
                    .emit(self, &branch.operands);
            }
            ExpressionSelection::Operand(operand) => {
                self.prepare_checked_value(&operand.value);
                let bit = self.frame.prepared_operand(operand, &self.sites, true);
                self.branch(&bit, yes, no);
            }
            _ => {
                let bit = self.test_operand(input);
                self.branch(&bit, yes, no);
            }
        }
    }

    fn test_operand(&mut self, input: ExpressionInput<'_>) -> String {
        self.test_with_temporaries(|f| {
            let range = input.range;
            if input.kind == ast::NodeKind::ExprCompare {
                f.registered_region(input.selected, true)
                    .unwrap_or_else(|| {
                        f.with_expression_range(range, |f| f.located_operand(input, true))
                    })
            } else {
                let value = f.input_value(input);
                f.truth_at(&value, Some(range))
            }
        })
    }
}
