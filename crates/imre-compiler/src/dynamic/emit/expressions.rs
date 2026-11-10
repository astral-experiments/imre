use crate::llvm::registered::IndexKey;
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::FrameEmitter;
use super::boolean::{Context as BooleanContext, Flow, Target};
use super::{Body, Function, permanent};
use crate::llvm::operations::{CheckedValue, ComparisonChain, ValueJoin};

impl Function<'_, '_> {
    pub(super) fn expr(&mut self, expr: &ast::Expr) -> String {
        self.with_expression_range(expr.range(), |code| {
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
            .map(|(index, checked)| match checked {
                Some(value) => self.registered_value(value),
                None => self.expr(source_defaults[index]),
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
        let position = imre_ast::expression_position(
            expr,
            &self.source.method_imports,
            Some(&self.source.private_names),
        );
        let previous = self.enter_expression(position);
        // The display itself is covered even if an operand still needs the syntax adapter.
        let value = if let Some((items, display)) = crate::frontend::display_elements(expr) {
            self.sequence_display(items, display, None, ast_spread, Self::expr)
        } else if let Some((op, [left, right])) = crate::frontend::binary_operands(expr) {
            let left = self.expr(left);
            let right = self.expr(right);
            self.binary_handles(op, &left, &right)
        } else if let Some(operation) = self.source.registered_source().object_operation(expr) {
            if let crate::ir::Builtin::RegisteredUnary(op) = operation.opcode {
                let scope = self.begin_operand_scope();
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
        if self.native_suspension
            && let Some((kind, value)) = crate::ir::SuspendValue::operand(expr)
        {
            return self.suspend_value(kind, expr.range(), |code| {
                value.map_or_else(|| permanent::NONE.to_string(), |value| code.expr(value))
            });
        }
        match expr {
            ast::Expr::Subscript(v) => {
                if let Some(index) = self.source.registered_source().partition_index(v) {
                    let span = self.expression_span(expr, &v.value, b'[');
                    let position =
                        span.map_or_else(|| "null".into(), |span| self.output.position(span));
                    let call = v.value.as_call_expr().expect("selected partition call");
                    let (site, target) = self.source.registered_source().call(call);
                    return self.call_value(
                        &site,
                        target,
                        crate::frontend::registered_arguments(&call.arguments),
                        Some((index, position)),
                        Self::expr,
                    );
                }
                let constant = crate::frontend::registered_constant_key(
                    &v.slice,
                    self.source.registered_source(),
                );
                let base = self.expr(&v.value);
                let key = match v.slice.as_ref() {
                    ast::Expr::Slice(slice) if slice.step.is_none() => IndexKey::Bounds(
                        slice.lower.as_deref(),
                        slice.upper.as_deref(),
                        constant.as_ref().map(|(k, _)| k),
                    ),
                    _ if let Some((_, value)) = &constant => {
                        IndexKey::Prepared(self.registered_value(value))
                    }
                    key => IndexKey::Object(key),
                };
                self.subscript(&base, key, v.range(), v.value.end(), Self::index_operand)
            }
            ast::Expr::Slice(slice) => self.slice_object(
                [
                    slice.lower.as_deref(),
                    slice.upper.as_deref(),
                    slice.step.as_deref(),
                ],
                Self::expr,
            ),
            ast::Expr::Dict(v) => self.dict_display(
                v.items.iter().map(|item| (item.key.as_ref(), &item.value)),
                Self::expr,
            ),
            ast::Expr::BoolOp(_) => self.boolean_value(expr),
            ast::Expr::Compare(v) => self.source_comparison(v, false),
            ast::Expr::If(v) => ValueJoin::select(
                self,
                |f| {
                    f.flush_check();
                    &mut f.frame.body
                },
                |f, yes, no| f.branch_expr(&v.test, yes, no),
                |f| CheckedValue::handle(f.expr(&v.body)),
                |f| CheckedValue::handle(f.expr(&v.orelse)),
            )
            .expect("value-producing registered conditional")
            .into_handle(),
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
            ast::Expr::TString(template) => self.residual_template(template),
            ast::Expr::FString(v) => {
                let mut parts = Vec::new();
                for part in v.value.iter() {
                    match part {
                        ast::FStringPartRef::Literal(lit) => {
                            if !lit.value.is_empty() {
                                let value = crate::frontend::registered_string(
                                    &lit.value,
                                    std::iter::once(lit),
                                    &self.source.replacements,
                                    self.source.has_templates,
                                );
                                parts.push(self.registered_value(&value));
                            }
                        }
                        ast::FStringPartRef::FString(s) => {
                            self.format_parts(&s.elements, &mut parts)
                        }
                    }
                }
                self.join_format_parts(parts)
            }
            expr if let Some(plan) = imre_ast::comprehension::Plan::from_expr(expr) => {
                self.comprehension(expr, plan)
            }
            ast::Expr::DictComp(_) => self.unsupported("dict unpacking comprehension"),
            _ => self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(expr).kind())),
        }
    }

    /// Jump to the selected suite without materializing or retesting a short-circuit operand.
    pub(super) fn branch_expr(&mut self, expr: &ast::Expr, yes: &str, no: &str) {
        let test = |f: &mut Self, expr: &ast::Expr, yes: &str, no: &str| {
            let condition = f.test_operand(expr);
            f.branch(&condition, yes, no);
        };
        if BooleanContext::Condition.splits(expr) {
            self.boolean_steps(expr, BooleanContext::Condition, yes, no, test);
        } else {
            test(self, expr, yes, no);
        }
    }

    fn test_operand(&mut self, expr: &ast::Expr) -> String {
        self.test_with_temporaries(|f| {
            if let ast::Expr::Compare(cmp) = expr {
                f.registered_region(expr, true).unwrap_or_else(|| {
                    f.with_expression_range(cmp.range(), |f| {
                        let previous = f.enter_expression(cmp.start());
                        let bit = f.source_comparison(cmp, true);
                        f.leave_expression(previous);
                        bit
                    })
                })
            } else {
                let value = f.expr(expr);
                f.truth(&value)
            }
        })
    }

    /// Assign a block to each graph step. Multiple predecessors can share a single operand.
    fn boolean_steps(
        &mut self,
        expr: &ast::Expr,
        context: BooleanContext,
        yes: &str,
        no: &str,
        mut emit: impl FnMut(&mut Self, &ast::Expr, &str, &str),
    ) {
        let flow = Flow::new(expr, context);
        let labels: Vec<_> = flow.steps.iter().map(|_| self.name("b")).collect();
        let label = |target| match target {
            Target::Step(i) => labels[i].as_str(),
            Target::Truth(true) => yes,
            Target::Truth(false) => no,
            Target::Operand => {
                debug_assert_eq!(yes, no);
                yes
            }
        };
        for (i, step) in flow.steps.iter().enumerate().rev() {
            if i + 1 != flow.steps.len() {
                self.label(&labels[i]);
            }
            emit(self, step.expr, label(step.yes), label(step.no));
        }
    }

    /// Keep the selected object at the join. Discard other operand roots before visiting the next.
    fn boolean_value(&mut self, expr: &ast::Expr) -> String {
        let done = self.name("b");
        let mut incoming = Vec::new();
        self.boolean_steps(
            expr,
            BooleanContext::Value,
            &done,
            &done,
            |f, expr, yes, no| {
                if yes == no {
                    let value = f.expr(expr);
                    incoming.push(format!("[ {value}, %{} ]", f.current_block()));
                    f.jump(&done);
                    return;
                }
                let mark = f.temporary_mark();
                let value = f.expr(expr);
                let truth = f.truth(&value);
                if yes != done && no != done {
                    f.discard_truth(&mark);
                    f.branch(&truth, yes, no);
                    return;
                }
                incoming.push(format!("[ {value}, %{} ]", f.current_block()));
                let discard = f.name("b");
                let next = if yes == done {
                    f.branch(&truth, &done, &discard);
                    no
                } else {
                    f.branch(&truth, &discard, &done);
                    yes
                };
                f.label(&discard);
                f.discard_truth(&mark);
                f.jump(next);
            },
        );
        self.label(&done);
        self.value(&format!("phi i64 {}", incoming.join(", ")))
    }

    /// A failed intermediate comparison returns its object in a value context and false in a branch.
    /// Evaluate a residual operand only in the reached link, keeping its result rooted.
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

    /// A residual field keeps its whole template on the original literal-spec path.
    fn residual_template(&mut self, template: &ast::ExprTString) -> String {
        let Some(segments) = crate::frontend::template_segments(
            template,
            self.source.registered_source(),
            &self.source.template_fields,
        ) else {
            return self.unsupported("t-string expression spelling");
        };
        let strings = self.emit_string_tuple(segments);
        let parts = self.op("list", &[]);
        for field in template
            .value
            .iter()
            .flat_map(|t| t.elements.interpolations())
        {
            let spec = match imre_ast::strings::template_format_spec(field) {
                Ok(Some(literal)) => self.format_literal(literal),
                Ok(None) => self.text(""),
                Err(reason) => return self.unsupported(reason),
            };
            let Some(spelling) = self.source.template_fields.get(&field.start()) else {
                return self.unsupported("t-string expression spelling");
            };
            let value = self.expr(&field.expression);
            let part = self.interpolation(
                &value,
                &spelling.expression,
                field.conversion.to_char(),
                &spec,
            );
            self.op("append", &[&parts, &part]);
        }
        self.op("build_template", &[&strings, &parts])
    }

    /// Evaluate the fields and their conversions in source order before joining their text.
    fn format_parts(
        &mut self,
        elements: &[ast::InterpolatedStringElement],
        parts: &mut Vec<String>,
    ) {
        for element in elements {
            let value = match element {
                ast::InterpolatedStringElement::Literal(lit) if lit.value.is_empty() => continue,
                ast::InterpolatedStringElement::Literal(lit) => self.format_literal(lit),
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(debug) = &field.debug_text {
                        parts.push(self.text(debug.as_str()));
                    }
                    let mut value = self.expr(&field.expression);
                    if let Some(conversion) = imre_ast::strings::interpolation_conversion(field) {
                        value = self.convert_format_value(&value, conversion);
                    }
                    let spec = match &field.format_spec {
                        Some(s) => {
                            let mut spec = Vec::new();
                            self.format_parts(&s.elements, &mut spec);
                            self.join_format_parts(spec)
                        }
                        None => self.text(""),
                    };
                    self.format_value(&value, &spec)
                }
            };
            parts.push(value);
        }
    }
}

fn ast_spread(element: &ast::Expr) -> Option<&ast::Expr> {
    element.as_starred_expr().map(|star| star.value.as_ref())
}
