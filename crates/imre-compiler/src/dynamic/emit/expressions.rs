use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::FrameEmitter;
use super::{Body, Function, permanent};
use crate::frontend::{RegisteredScope, SourceOperand, prepare_boolean};
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
            if let Some(selected) = code.source.logicals.get(expr) {
                match selected {
                    crate::frontend::LogicalSelection::Checked(value) => {
                        return code.registered_value(value);
                    }
                    _ if !code.native_suspension => return code.located_expr(expr),
                    _ => {}
                }
            }
            code.registered_region(expr, false)
                .unwrap_or_else(|| code.located_expr(expr))
        })
    }

    fn source_lambda(&mut self, lambda: &ast::ExprLambda) -> String {
        let site = lambda.start();
        let plan = &self.source.lambdas[&site];
        let defaults = plan
            .defaults
            .iter()
            .map(|default| match default {
                crate::frontend::LambdaDefault::Value(value) => {
                    self.with_operand_source(value, |code| code.registered_value(value))
                }
                crate::frontend::LambdaDefault::Operand(operand) => {
                    self.prepare_checked_value(&operand.value);
                    self.with_operand_source(&operand.value, |code| {
                        code.frame.prepared_operand(operand, &code.sites, false)
                    })
                }
                crate::frontend::LambdaDefault::Source(parameter) => {
                    let value = parameter.source(lambda);
                    self.with_source_range(value.range(), |code| code.expr(value))
                }
            })
            .collect::<Vec<_>>();
        let body = match &plan.body {
            Some(body) => Body::CheckedExpression(body),
            None => Body::Expression(&lambda.body),
        };
        self.place_lambda(site, body, false);
        let scope = self.scope.clone();
        self.sites
            .bind_lambda(&mut self.frame, site, &scope, &defaults)
    }

    fn registered_region(&mut self, expr: &ast::Expr, condition: bool) -> Option<String> {
        if let Some(operand) = self.source.strings.templates.get(&expr.range()) {
            self.prepare_checked_value(&operand.value);
            return Some(self.frame.prepared_operand(operand, &self.sites, condition));
        }
        let source = self.source.registered_source();
        let value = if self.native_suspension {
            crate::frontend::suspended_operand(expr, source)
        } else {
            crate::frontend::registered_value(expr, source)
        }?;
        self.prepare_checked_value(&value);
        Some(self.frame.registered_expression(&value, &self.sites, condition))
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
        let kind = if self.class_locals.is_some() {
            RegisteredScope::Class
        } else if self.module {
            RegisteredScope::Module
        } else {
            RegisteredScope::Function {
                is_async: self.is_async,
            }
        };
        if let Some(operand) =
            crate::frontend::inline_operand(expr, self.source.registered_source(), kind)
        {
            self.prepare_checked_value(&operand.value);
            return self.frame.prepared_operand(&operand, &self.sites, false);
        }
        let position = imre_ast::expression_position(
            expr,
            &self.source.method_imports,
            Some(&self.source.private_names),
        );
        let previous = self.enter_expression(expr.range(), position);
        // The display itself is covered even if an operand still needs the syntax adapter.
        let value = if let Some((items, display)) = crate::frontend::display_elements(expr) {
            self.sequence_display(items, display, None, ast_spread, Self::expr)
        } else if let Some(index) =
            crate::frontend::index_operands(expr, self.source.registered_source())
        {
            let inputs = index
                .inputs
                .into_iter()
                .map(|operand| self.source_operand(operand))
                .collect::<Vec<_>>();
            self.frame
                .registered_with_inputs(&index.value, &inputs, &self.sites)
        } else if let Some((op, [left, right])) = crate::frontend::binary_operands(expr) {
            let left = self.expr(left);
            let right = self.expr(right);
            self.binary_handles(op, &left, &right)
        } else if let Some((edges, inputs)) = self
            .source
            .logicals
            .value(expr, self.source.registered_source())
        {
            let value = Logical::value(&mut self.frame.body, edges, crate::ir::Type::PythonObject)
                .emit(self, &inputs)
                .expect("a selected operand reaches the join");
            self.frame
                .with_checked_frame(&self.sites, None, |mut frame| frame.materialize(&value))
        } else if let Some([test, yes, no]) = self.source.logicals.conditional(expr) {
            ValueJoin::select(
                self,
                |code| {
                    code.flush_check();
                    &mut code.frame.body
                },
                |code, yes, no| match test {
                    SourceOperand::Checked(test) => {
                        code.prepare_checked_value(&test);
                        code.frame.registered_branch(&test, &code.sites, yes, no);
                    }
                    SourceOperand::Residual(test) => code.branch_expr(test, yes, no),
                },
                |code| CheckedValue::handle(code.source_operand(yes)),
                |code| CheckedValue::handle(code.source_operand(no)),
            )
            .expect("value-producing conditional")
            .into_handle()
        } else if let Some(operation) = self.source.registered_source().object_operation(expr) {
            if let crate::ir::Builtin::RegisteredUnary(op) = operation.opcode {
                let scope = self.begin_unary();
                let operand = self.expr(operation.target);
                let result = self.frame.unary_handle(op, operand);
                self.finish_unary(scope, op, result).into_handle()
            } else {
                self.object_operation(
                    &operation.opcode,
                    operation.target,
                    operation.arguments(),
                    Self::expr,
                )
            }
        } else {
            self.expr_inner(expr)
        };
        self.leave_expression(previous);
        value
    }

    fn expr_inner(&mut self, expr: &ast::Expr) -> String {
        if let Some(operands) = crate::frontend::template_operands(expr) {
            let Some(plan) = self.source.strings.residual_templates.get(&expr.range()) else {
                return self.unsupported("t-string expression spelling");
            };
            let template = self.frame.begin_template(plan.segments.clone());
            for (operand, field) in operands.zip(&plan.fields) {
                let interpolation = match template.field(&mut self.frame, field, &self.sites) {
                    Ok(interpolation) => interpolation,
                    Err(reason) => return self.unsupported(reason),
                };
                let value = self.expr(operand);
                interpolation.capture(&mut self.frame, &value);
            }
            return template.finish(&mut self.frame);
        }
        if self.native_suspension
            && let Some((kind, value)) = crate::ir::SuspendValue::operand(expr)
        {
            return self.suspend_value(kind, expr.range(), |code| {
                value.map_or_else(|| permanent::NONE.to_string(), |value| code.expr(value))
            });
        }
        match expr {
            ast::Expr::Dict(v) => self.dict_display(
                v.items.iter().map(|item| (item.key.as_ref(), &item.value)),
                Self::expr,
            ),
            ast::Expr::Compare(v) => self.source_comparison(v, false),
            ast::Expr::Named(v) => {
                let ast::Expr::Name(target) = &*v.target else {
                    return self.unsupported("assignment expression target");
                };
                let mark = self.temporary_mark();
                let value = self.expr(&v.value);
                let name = self.code_name(target.id.as_str());
                let scope = self.scope.clone();
                self.assign_expression_name(&scope, &name, &value, &mark);
                value
            }
            ast::Expr::Lambda(lambda) => self.source_lambda(lambda),
            ast::Expr::FString(_) => {
                self.build_diagnostic("cannot compile this formatted string ahead of time")
            }
            ast::Expr::Generator(generator)
                if self.source.generators.contains_key(&generator.range()) =>
            {
                self.prepared_generator(generator)
            }
            ast::Expr::Generator(generator) => self.residual_generator(generator),
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
            SourceOperand::Checked(value) => self.registered_value(&value),
            SourceOperand::Residual(expr) => self.expr(expr),
        }
    }

    /// The enclosing source operation supplies evaluated leaves. Its branch graph
    /// and ownership policy are the same ones used by a wholly checked suite.
    pub(super) fn branch_expr(&mut self, expr: &ast::Expr, yes: &str, no: &str) {
        if let Some(crate::frontend::LogicalSelection::Checked(condition)) =
            self.source.logicals.get(expr)
        {
            self.prepare_checked_value(condition);
            self.frame
                .registered_branch(condition, &self.sites, yes, no);
            return;
        }
        let (edges, inputs) = prepare_boolean(
            expr,
            BooleanContext::Condition,
            self.source.registered_source(),
            &self.source.logicals,
        );
        Logical::branch(&mut self.frame.body, edges, yes, no).emit(self, &inputs);
    }

    fn test_operand(&mut self, expr: &ast::Expr) -> String {
        self.test_with_temporaries(|f| {
            if let ast::Expr::Compare(cmp) = expr {
                f.registered_region(expr, true).unwrap_or_else(|| {
                    f.with_expression_range(cmp.range(), |f| {
                        let previous = f.enter_expression(cmp.range(), cmp.start());
                        let bit = f.source_comparison(cmp, true);
                        f.leave_expression(previous);
                        bit
                    })
                })
            } else {
                let value = f.expr(expr);
                f.truth_at(&value, Some(expr.range()))
            }
        })
    }

    fn source_comparison(&mut self, cmp: &ast::ExprCompare, condition: bool) -> String {
        if let Some((needle, items, negate)) = crate::frontend::literal_membership(cmp) {
            let needle = self.expr(needle);
            let items = items.iter().map(|item| self.expr(item)).collect::<Vec<_>>();
            let value = self.contains_literal(&needle, &items, negate);
            return if condition { self.truth(&value) } else { value };
        }
        let mut chain = ComparisonChain::registered(&mut self.frame, &cmp.ops, condition);
        for operand in &*cmp.operands {
            let handle = self.expr(operand);
            if let Some(result) = chain.push_handle(&mut self.frame, handle) {
                return if condition {
                    result.into_condition()
                } else {
                    result.into_handle()
                };
            }
        }
        unreachable!("a comparison has a final link")
    }

}

fn ast_spread(element: &ast::Expr) -> Option<&ast::Expr> {
    element.as_starred_expr().map(|star| star.value.as_ref())
}
