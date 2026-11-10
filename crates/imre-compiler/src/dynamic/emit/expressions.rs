use crate::llvm::registered::ReadKey;
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::FrameEmitter;
use super::{Body, Function, permanent};
use crate::frontend::{RegisteredScope, SourceOperand, prepare_boolean};
use crate::ir::control::Context as BooleanContext;
use crate::llvm::operations::branches::{Logical, LogicalOperand};
use crate::llvm::operations::{CheckedValue, Choice, ComparisonChain, ValueJoin};

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
        let source_defaults = std::cell::LazyCell::new(|| {
            crate::frontend::lambda_defaults(lambda).collect::<Vec<_>>()
        });
        let defaults = plan
            .defaults
            .iter()
            .enumerate()
            .map(|(index, operand)| match operand {
                Some(value) => self.with_operand_source(value, |code| code.registered_value(value)),
                None => {
                    let operand = source_defaults[index];
                    self.with_source_range(operand.range(), |code| code.expr(operand))
                }
            })
            .collect::<Vec<_>>();
        let body = match &plan.body {
            Some(body) => Body::CheckedExpression(body),
            None => Body::Expression(&lambda.body),
        };
        let definition = self.compile_lambda(site, body);
        let scope = self.scope.clone();
        definition.bind_defaults(&mut self.frame, &scope, &defaults)
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
        } else if let Some((op, [left, right])) = crate::frontend::binary_operands(expr) {
            let left = self.expr(left);
            let right = self.expr(right);
            self.binary_handles(op, &left, &right)
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
            ast::Expr::Subscript(v) => {
                let base = self.expr(&v.value);
                let key = if let ast::Expr::Slice(slice) = &*v.slice
                    && slice.step.is_none()
                {
                    let [start, stop] =
                        [slice.lower.as_deref(), slice.upper.as_deref()].map(|bound| {
                            bound.map_or_else(|| "0".into(), |bound| self.index_operand(bound))
                        });
                    let slot = crate::frontend::registered_constant_key(
                        &v.slice,
                        self.source.registered_source(),
                    )
                    .map(|(key, _)| self.constant_slot(key));
                    ReadKey::Bounds { start, stop, slot }
                } else {
                    ReadKey::Object(self.index_operand(&v.slice))
                };
                self.read_item(&base, key, v.range(), v.value.end())
            }
            ast::Expr::Slice(slice) => {
                let bounds = [
                    slice.lower.as_deref(),
                    slice.upper.as_deref(),
                    slice.step.as_deref(),
                ]
                .map(|bound| bound.map_or_else(|| "0".into(), |bound| self.index_operand(bound)));
                self.slice_object(bounds)
            }
            ast::Expr::Dict(v) => self.dict_display(
                v.items.iter().map(|item| (item.key.as_ref(), &item.value)),
                Self::expr,
            ),
            ast::Expr::BoolOp(_) => self.boolean_value(expr),
            ast::Expr::Compare(v) => self.source_comparison(v, false),
            ast::Expr::If(branch) => self.residual_conditional(branch),
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
            ast::Expr::FString(value) => {
                let mark = self.temporary_mark();
                self.unwind_format_operands(&mark, |code| {
                    let mut parts = Vec::new();
                    for part in value.value.iter() {
                        match part {
                            ast::FStringPartRef::Literal(literal) => {
                                if let Some(value) = code.source.strings.literals.get(&literal.range()) {
                                    parts.push(code.registered_value(value));
                                }
                            }
                            ast::FStringPartRef::FString(formatted) => {
                                code.format_parts(&formatted.elements, &mut parts);
                            }
                        }
                    }
                    code.format_join(&mark, parts)
                })
            }
            expr if let Some(plan) = imre_ast::comprehension::Plan::from_expr(expr) => {
                self.comprehension(expr, plan)
            }
            ast::Expr::DictComp(_) => self.unsupported("dict unpacking comprehension"),
            _ => self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(expr).kind())),
        }
    }

    fn residual_conditional(&mut self, expr: &ast::ExprIf) -> String {
        let (yes, no) = Choice::successors(&mut self.frame.body);
        let mut choice = Choice::new(&mut self.frame.body);
        let mut values = ValueJoin::default();
        let parts = self.source.logicals.choice(expr);
        if let Some(test) = parts.and_then(|p| p[0].as_ref()) {
            self.prepare_checked_value(test);
            self.frame.registered_branch(test, &self.sites, &yes, &no);
        } else {
            self.branch_expr(&expr.test, &yes, &no);
        }
        for (i, block, operand) in [(1, &yes, &expr.body), (2, &no, &expr.orelse)] {
            self.label(block);
            let value = match parts.and_then(|p| p[i].as_ref()) {
                Some(value) => self.registered_value(value),
                None => self.expr(operand),
            };
            self.flush_check();
            values.incoming(&mut self.frame.body, CheckedValue::handle(value));
            choice.finish_arm(&mut self.frame.body);
        }
        choice.finish(&mut self.frame.body, false);
        values
            .finish(&mut self.frame.body)
            .expect("value-producing conditional")
            .into_handle()
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
        let mut plan = Logical::branch(&mut self.frame.body, edges, yes, no);
        self.logical_inputs(&mut plan, inputs, BooleanContext::Condition);
    }

    fn boolean_value(&mut self, expr: &ast::Expr) -> String {
        let (edges, inputs) = prepare_boolean(
            expr,
            BooleanContext::Value,
            self.source.registered_source(),
            &self.source.logicals,
        );
        let mut plan = Logical::value(&mut self.frame.body, edges, crate::ir::Type::PythonObject);
        self.logical_inputs(&mut plan, inputs, BooleanContext::Value);
        self.flush_check();
        plan.finish(&mut self.frame.body).into_handle()
    }

    fn logical_inputs(
        &mut self,
        plan: &mut Logical,
        inputs: Vec<SourceOperand<'_>>,
        context: BooleanContext,
    ) {
        for (i, leaf) in inputs.into_iter().enumerate().rev() {
            if let Some(block) = plan.block(i) {
                self.label(block);
            }
            let operand = if context == BooleanContext::Condition {
                let bit = match leaf {
                    SourceOperand::Checked(value) => {
                        self.prepare_checked_value(&value);
                        self.frame.registered_expression(&value, &self.sites, true)
                    }
                    SourceOperand::Residual(expr) => self.test_operand(expr),
                };
                LogicalOperand::Predicate(bit)
            } else {
                let mark = (!plan.final_value(i)).then(|| self.temporary_mark());
                let value = match leaf {
                    SourceOperand::Checked(value) => self.registered_value(&value),
                    SourceOperand::Residual(expr) => self.expr(expr),
                };
                match mark {
                    Some(mark) => {
                        let truth = self.truth(&value);
                        LogicalOperand::Tested {
                            value: CheckedValue::handle(value),
                            truth,
                            mark: Some(mark),
                        }
                    }
                    None => LogicalOperand::Selected(CheckedValue::handle(value)),
                }
            };
            self.frame.logical_operand(plan, i, operand);
        }
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

    /// Evaluate the fields and their conversions in source order before joining their text.
    fn format_parts(
        &mut self,
        elements: &[ast::InterpolatedStringElement],
        parts: &mut Vec<String>,
    ) {
        for element in elements {
            let value = match element {
                ast::InterpolatedStringElement::Literal(lit) => {
                    let Some(value) = self.source.strings.literals.get(&lit.range()) else {
                        continue;
                    };
                    self.registered_value(value)
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(prefix) = self.source.strings.literals.get(&field.range()) {
                        parts.push(self.registered_value(prefix));
                    }
                    let mark = self.temporary_mark();
                    self.with_expression_range(field.range(), |code| {
                        code.unwind_format_operands(&mark, |code| {
                            let mut value = code.expr(&field.expression);
                            code.keep_operands(&mark, &value, "0");
                            if let Some(conversion) = imre_ast::strings::interpolation_conversion(field) {
                                value = code.format_conversion(&value, conversion);
                                code.keep_operands(&mark, &value, "0");
                            }
                            let spec = match &field.format_spec {
                                Some(spec) => {
                                    let mark = code.temporary_mark();
                                    code.unwind_format_operands(&mark, |code| {
                                        let mut parts = Vec::new();
                                        code.format_parts(&spec.elements, &mut parts);
                                        code.format_join(&mark, parts)
                                    })
                                }
                                None => code.text(""),
                            };
                            let result = code.format_field(&value, &spec);
                            code.keep_operands(&mark, &result, "0");
                            result
                        })
                    })
                }
            };
            parts.push(value);
        }
    }
}

fn ast_spread(element: &ast::Expr) -> Option<&ast::Expr> {
    element.as_starred_expr().map(|star| star.value.as_ref())
}
