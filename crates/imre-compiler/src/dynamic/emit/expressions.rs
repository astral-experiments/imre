use ruff_python_ast as ast;
use ruff_text_size::TextRange;

use super::FrameEmitter;
use super::Function;
use crate::frontend::{ExpressionInput, ExpressionSelection};

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
        let value = if let ExpressionSelection::Suspension(kind, child) = selected {
            self.suspend_value(*kind, range, |code| code.input_value(child.as_input()))
        } else if let Some(inputs) = physical {
            crate::llvm::registered::Frame::prepared_operation(self, inputs, condition)
        } else {
            self.build_diagnostic("expression has no checked operand")
        };
        self.leave_expression(previous);
        value
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
            if input.kind == ast::NodeKind::ExprCompare || input.selected.physical().is_some() {
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
