//! Execute comprehension steps directly or lower them into a resumable source frame.

use imre_ast::comprehension::Step as CompStep;

use super::*;

impl Program {
    /// Lower a generator expression to resumable for loops. Its frame already
    /// owns the outer iterator; only inner clauses evaluate Iter instructions.
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

    /// Inline the clauses in the current resumable frame. Its operand stack and
    /// protected regions already survive suspension, cancellation and exceptions.
    pub(super) fn lower_eager_comprehension(
        &mut self,
        comp: &Plan<'_>,
        operand: &OperandPlan,
    ) -> Result<()> {
        let line = operand.line;
        let targets = comp.try_target_names().map_err(invalid_target)?;
        let iterable = self.lower_suspension(comp.outer.clone(), line, operand.function)?;
        self.instructions
            .push((line, Instruction::Iter(iterable, comp.outer_async)));
        let enter = self.instructions.len();
        self.instructions.push((
            line,
            Instruction::EnterComprehension {
                kind: comp.output.kind(),
                targets,
                end: 0,
            },
        ));
        self.lower_comprehension_body(comp, &|_| line)?;
        self.instructions
            .push((line, Instruction::LeaveComprehension));
        let after = self.instructions.len();
        let Instruction::EnterComprehension { end, .. } = &mut self.instructions[enter].1 else {
            unreachable!("comprehension scope")
        };
        *end = after;
        Ok(())
    }

    /// Translate plan destinations to resumable instruction offsets. The source
    /// frame already owns the first iterator when it enters this body.
    fn lower_comprehension_body(
        &mut self,
        comp: &Plan<'_>,
        line_at: &impl Fn(usize) -> u32,
    ) -> Result<()> {
        let mut offsets = Vec::with_capacity(comp.steps.len());
        let mut guard_at_close = vec![0; comp.steps.len()];
        let mut line = line_at(comp.outer.start().to_usize());
        for (pc, &step) in comp.steps.iter().enumerate() {
            offsets.push(self.instructions.len());
            match step {
                CompStep::Open {
                    iterable, is_async, ..
                } => {
                    line = line_at(iterable.start().to_usize());
                    let value = self.lower_suspension(iterable.clone(), line, true)?;
                    self.instructions
                        .push((line, Instruction::Iter(value, is_async)));
                }
                CompStep::Next {
                    target,
                    is_async,
                    exhausted,
                    ..
                } => {
                    line = line_at(target.start().to_usize());
                    if matches!(comp.output, Output::Generator(_)) {
                        self.declare_target(target)?;
                    }
                    guard_at_close[exhausted] = self.instructions.len();
                    let suspend = self.kind.is_async()
                        && imre_ast::comprehension::expression_suspends(target);
                    let direct = (!suspend).then(|| target.clone());
                    if is_async {
                        self.instructions
                            .push((line, Instruction::EnterExcept { handler: 0, end: 0 }));
                        self.lower_async_next(line, direct);
                    } else {
                        self.instructions.push((line, Instruction::Next(direct, 0)));
                    }
                    if suspend {
                        self.instructions
                            .push((line, Instruction::SaveOperand(None)));
                        self.lower_suspended_target(target, line)?;
                    }
                }
                CompStep::Filter { test, reject } => {
                    line = line_at(test.start().to_usize());
                    let exits = self.false_branches(test.clone(), line, true)?;
                    self.resolve_branches(exits, offsets[reject]);
                }
                CompStep::Emit { output, resume } => {
                    if let Some(key) = output.key() {
                        line = line_at(key.start().to_usize());
                        let value = self.lower_suspension(key.clone(), line, true)?;
                        self.instructions
                            .push((line, Instruction::SaveOperand(value)));
                    }
                    let element = output.value();
                    line = line_at(element.start().to_usize());
                    let value = self.lower_suspension(element.clone(), line, true)?;
                    let op = if matches!(output, Output::Generator(_)) {
                        match value {
                            Some(expr) => Instruction::Yield(Some(expr), element.range()),
                            None => Instruction::YieldResult(element.range()),
                        }
                    } else {
                        Instruction::CollectItem {
                            value,
                            keyed: output.key().is_some(),
                        }
                    };
                    self.instructions.push((line, op));
                    self.instructions
                        .push((line, Instruction::Jump(offsets[resume])));
                }
                CompStep::Close { next, .. } => {
                    self.finish_iterator_loop(guard_at_close[pc], line);
                    if next != pc + 1 {
                        self.instructions
                            .push((line, Instruction::Jump(offsets[next])));
                    }
                }
                CompStep::End => {}
            }
        }
        Ok(())
    }

    /// Bind the value on the operand stack. Unpack a complete level before
    /// evaluating any receiver in its child targets, even across an await.
    fn lower_suspended_target(&mut self, target: &ast::Expr, line: u32) -> Result<()> {
        if !imre_ast::comprehension::expression_suspends(target) {
            self.instructions
                .push((line, Instruction::BindOperand(target.clone())));
            return Ok(());
        }
        match target {
            ast::Expr::Starred(star) => self.lower_suspended_target(&star.value, line)?,
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => {
                let star = elts.iter().position(ast::Expr::is_starred_expr);
                self.instructions.push((
                    line,
                    Instruction::UnpackTarget {
                        count: elts.len(),
                        star,
                    },
                ));
                for item in elts {
                    self.lower_suspended_target(item, line)?;
                }
            }
            ast::Expr::Attribute(field) => {
                let receiver = self.lower_suspension(*field.value.clone(), line, true)?;
                self.instructions
                    .push((line, Instruction::SaveOperand(receiver)));
                self.instructions
                    .push((line, Instruction::StoreAttribute(field.attr.clone())));
            }
            ast::Expr::Subscript(item) => {
                for expr in [&item.value, &item.slice] {
                    let value = self.lower_suspension(*expr.clone(), line, true)?;
                    self.instructions
                        .push((line, Instruction::SaveOperand(value)));
                }
                self.instructions.push((line, Instruction::StoreSubscript));
            }
            _ => unreachable!("validated comprehension target"),
        }
        Ok(())
    }
}

impl Context {
    /// Obtain an iterator in the selected scope and drop its consumed expression operands.
    fn source_comprehension_iterator(
        &mut self,
        code: &Program,
        scope: Id,
        expression: &ast::Expr,
    ) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let source = self.source_expr(code, scope, expression)?;
        let iterator = self.iterator(source)?;
        self.keep_source_result(mark, iterator)
    }

    /// Append each element to the result before advancing its innermost iterator.
    pub(super) fn source_eager_comprehension(
        &mut self,
        code: &Program,
        parent: Id,
        comp: &Plan<'_>,
        targets: &HashSet<String>,
    ) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let result = (|| -> Result<Id> {
            let first = self.source_comprehension_iterator(code, parent, comp.outer)?;
            let scope = self.comprehension_scope_with_targets(parent, targets.clone());
            let collection = self.op(comp.output.kind(), NONE, NONE, NONE)?;
            let mut levels = Vec::new();
            levels
                .try_reserve_exact(comp.depths)
                .map_err(|_| crate::Error::memory_error())?;
            levels.push((first, self.heap.temporary_mark()));
            let mut pc = 0;
            loop {
                match comp.steps[pc] {
                    CompStep::Open { iterable, .. } => {
                        let iter = self.source_comprehension_iterator(code, scope, iterable)?;
                        levels.push((iter, self.heap.temporary_mark()));
                    }
                    CompStep::Next {
                        depth,
                        target,
                        exhausted,
                        ..
                    } => {
                        let (iter, mark) = levels[depth];
                        self.comprehension_step(mark);
                        let Some(item) = self.next_item(iter)? else {
                            pc = exhausted;
                            continue;
                        };
                        self.heap.keep(item);
                        self.bind_target(code, scope, target, item)?;
                    }
                    CompStep::Filter { test, reject } => {
                        if !self.source_condition(code, scope, test)? {
                            pc = reject;
                            continue;
                        }
                    }
                    CompStep::Emit { output, resume } => {
                        let key = output
                            .key()
                            .map(|expr| self.source_expr(code, scope, expr))
                            .transpose()?;
                        let value = self.source_expr(code, scope, output.value())?;
                        if let Some(key) = key {
                            self.put_item(collection, key, value)?;
                        } else {
                            self.op("append", collection, value, NONE)?;
                        }
                        pc = resume;
                        continue;
                    }
                    CompStep::Close { next, .. } => {
                        levels.pop();
                        pc = next;
                        continue;
                    }
                    CompStep::End => return Ok(collection),
                }
                pc += 1;
            }
        })();
        match result {
            Ok(collection) => self.keep_source_result(mark, collection),
            Err(error) => {
                let collect = self.heap.discard_truth_operands(mark, self.modules);
                for root in error.roots() {
                    self.heap.keep(root);
                }
                if collect {
                    self.collect_unreachable();
                }
                Err(error)
            }
        }
    }

    pub(super) fn source_genexpr(
        &mut self,
        code: &Program,
        scope: Id,
        expr: &ast::Expr,
        outer: Option<Id>,
    ) -> Result<Id> {
        let ast::Expr::Generator(genexpr) = expr else {
            unreachable!("generator expression")
        };
        let (site, definition) = &code.expression_functions[&expr.range()];
        let function = self.source_definition(
            code,
            self.heap.current_code(),
            scope,
            code.instructions.len() + site,
            definition,
        )?;
        self.heap.keep(function);
        let first = outer_clause(&genexpr.generators);
        let iterable = match outer {
            Some(value) => value,
            None => self.source_expr(code, scope, &first.iter)?,
        };
        let iter = if first.is_async {
            self.async_for_iter(iterable)?
        } else {
            self.iterator(iterable)?
        };
        let generator = self.call(function, vec![iter], Vec::new())?;
        let frame = match *self.heap.get(generator) {
            Value::AsyncGenerator { frame, .. } => frame,
            _ => generator,
        };
        let Value::Generator { iterators, .. } = self.heap.get_mut(frame) else {
            unreachable!("source generator expression")
        };
        iterators.push(iter);
        Ok(generator)
    }
}
