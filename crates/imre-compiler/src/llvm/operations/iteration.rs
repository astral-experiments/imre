//! Shared traversal of loop and unpacking IR for fixed and registered entries.
use super::{CheckedBody, CheckedValue, Entry, Expr, ExprKind, Statement};
use crate::ir::{Binary, Compare, ItemOrder, ItemUpdate, IterNextKind};
use crate::llvm::{Cleanup, Emitter, LoopTarget, NativeLoop, small};

#[derive(Clone)]
pub(crate) struct LoopEdges {
    pub(crate) step: String,
    pub(crate) exhausted: String,
    pub(crate) done: String,
}

/// Wire the test, body, exhaustion, and break edges for either a complete or mixed loop.
pub(crate) fn loop_region<C, T>(
    context: &mut C,
    output: fn(&mut C) -> &mut super::FunctionBody,
    label: fn(&mut C) -> String,
    enter: impl FnOnce(&mut C, &LoopEdges),
    test: impl FnOnce(&mut C, &str, &str) -> T,
    body: impl FnOnce(&mut C, &LoopEdges, T),
    otherwise: Option<impl FnOnce(&mut C)>,
) {
    let step = label(context);
    let body_label = label(context);
    let done = label(context);
    let exhausted = if otherwise.is_some() {
        label(context)
    } else {
        done.clone()
    };
    let edges = LoopEdges {
        step,
        exhausted,
        done,
    };
    enter(context, &edges);
    output(context).start(&edges.step);
    let item = test(context, &body_label, &edges.exhausted);
    output(context).start(&body_label);
    body(context, &edges, item);
    let out = output(context);
    if !out.terminated {
        out.jump(&edges.step);
    }
    if let Some(emit_else) = otherwise {
        output(context).start(&edges.exhausted);
        emit_else(context);
        let out = output(context);
        if !out.terminated {
            out.jump(&edges.done);
        }
    }
    output(context).start(&edges.done);
}

impl Emitter<'_, '_> {
    pub(in crate::llvm) fn range_loop(
        &mut self,
        small_cursor: bool,
        [target, cursor, stop, step]: [usize; 4],
        args: &[Expr],
        body: &[Statement],
        orelse: &[Statement],
    ) {
        let unit_step = match args {
            [_] | [_, _] => true,
            [_, _, step] => matches!(&step.kind, ExprKind::Int(n) if n == "1"),
            _ => false,
        };
        let mut args = args.iter().map(|arg| self.expr(arg)).collect::<Vec<_>>();
        if args.len() == 1 {
            args.insert(0, small(0));
        }
        if args.len() == 2 {
            args.push(small(1));
        }
        for (slot, value) in [cursor, stop, step].into_iter().zip(&args) {
            self.store(slot, value);
        }
        let zero = self.compare(Compare::Equal, &args[2], &small(0));
        let [fail, condition, body_label, increment, end] = std::array::from_fn(|_| self.label());
        let exhausted = if orelse.is_empty() {
            end.clone()
        } else {
            self.label()
        };
        self.uniform_append(body, &args, target, cursor, &exhausted);
        if unit_step {
            self.uniform_indexed_sum(body, target, cursor, &args, &exhausted);
        }
        self.branch(&zero.operand, &fail, &condition);
        self.start(&fail);
        self.checked_line("call void @imre_range_step_zero()");
        self.body.end("unreachable");
        self.start(&condition);
        self.check_interrupt();
        let mut current = self.borrow_slot(cursor);
        current.small_integer |= small_cursor;
        let mut stop_value = self.borrow_slot(stop);
        stop_value.small_integer |= args[1].small_integer;
        let mut step_value = self.borrow_slot(step);
        step_value.small_integer |= args[2].small_integer;
        let positive = self.compare(Compare::Greater, &step_value, &small(0));
        let below = self.compare(Compare::Less, &current, &stop_value);
        let above = self.compare(Compare::Greater, &current, &stop_value);
        let test = self.instruction(format!(
            "select i1 {}, i1 {}, i1 {}",
            positive.operand, below.operand, above.operand
        ));
        self.branch(&test, &body_label, &exhausted);
        self.start(&body_label);
        if unit_step {
            self.advance_affine(body, target, cursor, stop);
        }
        let current = self.load(cursor, small_cursor);
        self.store(target, &current);
        self.loops.push(NativeLoop {
            target: LoopTarget {
                edges: LoopEdges {
                    step: increment.clone(),
                    exhausted: exhausted.clone(),
                    done: end.clone(),
                },
                cleanup_depth: self.cleanups.len(),
            },
            raw_subtotal: None,
        });
        self.block(body);
        self.loops.pop();
        if !self.body.terminated {
            if unit_step {
                self.batch_modular_writes(body, target, cursor, stop);
            }
            self.jump(&increment);
        }
        self.start(&increment);
        let mut current = self.borrow_slot(cursor);
        current.small_integer |= small_cursor;
        let mut step_value = self.borrow_slot(step);
        step_value.small_integer |= args[2].small_integer;
        let next = if small_cursor {
            self.small_binary(Binary::Add, &current, &step_value, false, None)
        } else {
            self.binary(Binary::Add, &current, &step_value)
        };
        self.store(cursor, &next);
        self.jump(&condition);
        if !orelse.is_empty() {
            self.start(&exhausted);
            self.block(orelse);
            if !self.body.terminated {
                self.jump(&end);
            }
        }
        self.start(&end);
    }

    /// A specialized pull uses the same guarded-success edge as the Python iterator.
    pub(in crate::llvm) fn when(&mut self, condition: &str, success: impl FnOnce(&mut Self)) {
        CheckedBody::native(self).when(condition, |body| success(body.fixed()));
    }
}

impl CheckedBody<'_, '_, '_, '_> {
    pub(super) fn clear_iteration_slots(&mut self, slots: &[usize]) {
        match &mut self.0 {
            Entry::Native(e) => e.clear_slots(slots),
            Entry::Registered(f) => f.clear_slots(slots),
        }
    }

    pub(super) fn scoped_suite(&mut self, statements: &[Statement], clear: &[usize]) {
        let cursor_unwind = match &mut self.0 {
            Entry::Native(e) => {
                e.cleanups.push(Cleanup::Slots(clear.to_vec()));
                None
            }
            Entry::Registered(f) => {
                let unwind = f.owns_sync_cursor(clear).then(|| f.begin_operand_scope());
                f.enter_scope(clear);
                unwind
            }
        };
        self.emit_block(statements);
        match &mut self.0 {
            Entry::Native(e) => {
                e.cleanups.pop();
            }
            Entry::Registered(f) => {
                f.leave_scope();
                if let Some(unwind) = cursor_unwind {
                    // The landing block clears this loop's slots. Publish the
                    // surviving roots before the enclosing cleanup runs.
                    let terminated = f.terminated();
                    f.finish_unwind(unwind, |code| code.release_target());
                    if terminated {
                        f.end("unreachable");
                    }
                }
            }
        }
        if !self.body().terminated {
            self.clear_iteration_slots(clear);
        }
    }

    pub(super) fn scoped_expression(
        &mut self,
        statements: &[Statement],
        value: &Expr,
        clear: &[usize],
    ) -> CheckedValue {
        let scope = match &mut self.0 {
            Entry::Native(e) => {
                e.cleanups.push(Cleanup::Slots(clear.to_vec()));
                None
            }
            Entry::Registered(f) => {
                f.unwind_scope(clear);
                Some(f.start_expression())
            }
        };
        self.emit_block(statements);
        if let Entry::Native(e) = &mut self.0
            && let ExprKind::Local(slot) = value.kind
            && clear == [slot]
            && e.owns_slot(slot)
        {
            e.cleanups.pop();
            return CheckedValue::owned(e.take_slot(slot));
        }
        let result = self.evaluate(value);
        if let Entry::Native(e) = &mut self.0 {
            e.cleanups.pop();
        }
        self.clear_iteration_slots(clear);
        if let Some(scope) = scope {
            self.python().finish_expression(scope, clear, &result);
        }
        result
    }

    pub(super) fn while_suite(
        &mut self,
        stmt: &Statement,
        test: &Expr,
        body: &[Statement],
        otherwise: &[Statement],
    ) {
        if crate::ir::control::boolean_literal(test) == Some(false) {
            self.emit_block(otherwise);
            return;
        }
        let optimized = match &mut self.0 {
            Entry::Native(e) => Some((
                e.snapshot_loop_lengths(stmt),
                e.int_buffer_loop(stmt),
                e.raw_subtotal(body),
            )),
            Entry::Registered(_) => None,
        };
        let (mark, outer_mark) = match &mut self.0 {
            Entry::Registered(f) => {
                let mark = f.iteration_mark();
                let outer = f.replace_expression_mark(mark.clone());
                (mark, outer)
            }
            _ => (None, None),
        };
        loop_region(
            self,
            Self::body,
            Self::block_name,
            |this, edges| {
                if let Entry::Native(e) = &mut this.0
                    && e.struct_run_loop(test, body, &edges.step, &edges.exhausted)
                {
                    return;
                }
                this.jump_open(&edges.step);
            },
            |this, yes, no| {
                if let Entry::Native(e) = &mut this.0 {
                    e.check_interrupt();
                    if let Some(bit) = e.contiguous_sum_condition(
                        test,
                        optimized.as_ref().is_some_and(|(_, _, s)| s.is_some()),
                    ) {
                        e.branch(&bit, yes, no);
                        return;
                    }
                } else if let Some(mark) = &mark {
                    this.python().begin_step(mark);
                }
                this.branch_condition(test, yes, no);
            },
            |this, edges, ()| {
                match &mut this.0 {
                    Entry::Native(e) => e.loops.push(NativeLoop {
                        target: LoopTarget {
                            edges: edges.clone(),
                            cleanup_depth: e.cleanups.len(),
                        },
                        raw_subtotal: optimized.as_ref().and_then(|(_, _, s)| s.clone()),
                    }),
                    Entry::Registered(f) => f.enter_loop(edges),
                }
                this.emit_block(body);
                match &mut this.0 {
                    Entry::Native(e) => {
                        e.loops.pop();
                    }
                    Entry::Registered(f) => f.leave_loop(),
                }
            },
            (!otherwise.is_empty()).then_some(|this: &mut Self| this.emit_block(otherwise)),
        );
        if let Entry::Registered(f) = &mut self.0 {
            f.replace_expression_mark(outer_mark);
        }
        if let Some((lengths, buffers, _)) = optimized {
            let e = self.fixed();
            e.length_snapshots = lengths;
            e.int_buffers = buffers;
        }
    }

    pub(super) fn pull(
        &mut self,
        iterator: usize,
        key: usize,
        value: Option<usize>,
        ready: usize,
        kind: IterNextKind,
        tail: &mut &[Statement],
    ) {
        if let Entry::Native(e) = &mut self.0 {
            e.iter_next(iterator, key, value, ready, kind);
            return;
        }
        self.python().pull(iterator, key, value, ready, kind, tail);
    }

    pub(in crate::llvm) fn loop_transfer(&mut self, statement: &Statement) {
        let target = match &self.0 {
            Entry::Native(e) => e.loops.last().map(|active| &active.target),
            Entry::Registered(f) => f.loops.last().map(|(target, _)| target),
        }
        .expect("loop transfer");
        let label = match statement {
            Statement::Break(_) => &target.edges.done,
            Statement::Continue(_) => &target.edges.step,
            Statement::Exhaust => &target.edges.exhausted,
            _ => unreachable!("loop transfer"),
        }
        .clone();
        let depth = target.cleanup_depth;
        self.jump_from_regions(label, depth);
    }

    pub(super) fn augment_item(&mut self, table: &Expr, key: &Expr, op: ItemUpdate, rhs: &Expr) {
        let nonnegative = matches!(&self.0, Entry::Native(e) if e.nonnegative_index(key));
        let table = self.rooted_operand(table);
        let key = self.rooted_operand(key);
        if let ItemUpdate::Fixed(op) = op
            && self
                .fixed()
                .accumulate_int_item(table.fixed_value(), key.fixed_value(), op, rhs)
        {
            self.release_input(key);
            self.release_input(table);
            return;
        }

        // The RHS can replace the entry or raise. Keep the target and its old value
        // in their original representation until the in-place result has been stored.
        let old = match op {
            ItemUpdate::Fixed(_) => {
                let e = self.fixed();
                let table = table.fixed_value();
                let element = e
                    .module
                    .types
                    .indexed_element(table.ty)
                    .or_else(|| e.module.types.mapping(table.ty).map(|(_, value)| value))
                    .unwrap();
                CheckedValue::owned(e.get_item(table, key.fixed_value(), element, nonnegative))
            }
            ItemUpdate::Protocol(_) => CheckedValue::handle(
                self.python()
                    .op("get", &[table.rooted_handle(), key.rooted_handle()]),
            ),
        };
        let right = self.rooted_operand(rhs);
        let value = match op {
            ItemUpdate::Fixed(op) => CheckedValue::owned(self.fixed().binary(
                op,
                old.fixed_value(),
                right.fixed_value(),
            )),
            ItemUpdate::Protocol(op) => CheckedValue::handle(self.python().inplace(
                op,
                old.rooted_handle(),
                right.rooted_handle(),
            )),
        };
        self.write_item(&table, &key, &value, None, nonnegative);
        for operand in [value, right, old, key, table] {
            self.release_input(operand);
        }
    }

    pub(super) fn store_item(&mut self, order: ItemOrder, table: &Expr, key: &Expr, value: &Expr) {
        let (buffer, nonnegative, mark) = match &mut self.0 {
            Entry::Native(e) => {
                if e.copy_item(table, key, value) {
                    return;
                }
                (
                    e.fixed_int_buffer_slot(table),
                    e.nonnegative_index(key),
                    None,
                )
            }
            Entry::Registered(f) => (None, false, Some(f.temporary_mark())),
        };
        // A display can discard intermediate Python values as soon as each operand
        // is ready. Keep the key alive across the value, including a raising value.
        let display = mark.as_ref().filter(|_| order == ItemOrder::Display);
        let unwind = display.map(|_| self.python().begin_operand_scope());
        let mut next = display.cloned();
        let (table, key, value) = order.evaluate(table, key, value, |expr| {
            let operand = self.rooted_operand(expr);
            if let Some(mark) = &mut next {
                *mark = self
                    .python()
                    .keep_operands(mark, operand.rooted_handle(), "0");
            }
            operand
        });
        self.write_item(&table, &key, &value, buffer, nonnegative);
        if let Some(operands) = mark {
            // STORE_SUBSCR pops key and receiver before the RHS. The target
            // checkpoint also releases any earlier snapshot of a taken slot.
            let f = self.python();
            if let Some(unwind) = unwind {
                f.finish_unwind(unwind, |frame| frame.release_owned_operands(&operands));
            }
            f.release_owned_operands(&operands);
            if order == ItemOrder::Assignment {
                f.release_target();
            }
        } else {
            let e = self.fixed();
            let (list, index, value) =
                (table.into_native(), key.into_native(), value.into_native());
            if order == ItemOrder::Assignment
                && e.module.types.mapping(list.value.ty).is_some()
                && e.module
                    .types
                    .may_own_file(value.value.ty, &e.module.class_fields, true)
            {
                // The assignment's stack keeps the RHS alive while the
                // temporary dictionary releases its remaining entries.
                e.release_operand(&list);
                e.release_operand(&index);
                e.release_operand(&value);
            } else {
                e.release_operand(&value);
                e.release_operand(&index);
                e.release_operand(&list);
            }
        }
    }

    /// Write already evaluated operands while their statement still holds its owners.
    pub(super) fn write_item(
        &mut self,
        table: &CheckedValue,
        key: &CheckedValue,
        value: &CheckedValue,
        buffer: Option<usize>,
        nonnegative: bool,
    ) {
        match &mut self.0 {
            Entry::Native(e) => {
                let key = key.fixed_value();
                let value = value.fixed_value();
                if let Some(slot) = buffer {
                    e.fixed_int_set(slot, key, value);
                } else {
                    e.set_item(table.fixed_value(), key, value, nonnegative);
                }
            }
            Entry::Registered(f) => {
                f.put_item(
                    table.rooted_handle(),
                    key.rooted_handle(),
                    value.rooted_handle(),
                );
            }
        }
    }

    pub(super) fn conditional_suite(
        &mut self,
        stmt: &Statement,
        test: &Expr,
        yes: &[Statement],
        no: &[Statement],
    ) {
        let facts = match &mut self.0 {
            Entry::Native(emitter) => Some(emitter.if_facts(test, yes)),
            Entry::Registered(_) => None,
        };
        super::join::choose(
            self,
            Self::body,
            [(Some(test), (yes, facts.as_ref())), (None, (no, None))],
            |code, test, t, f| {
                if let Some(facts) = &facts
                    && (!facts.small_slots.is_empty() || facts.window.is_some())
                {
                    let bit = code.test_operand(test);
                    code.fixed().if_edges(stmt, &bit, facts, t, f);
                } else {
                    code.branch_condition(test, t, f);
                }
            },
            |code, (body, facts)| {
                if let Some(facts) = facts {
                    code.fixed().enter_if_body(test, facts);
                }
                code.emit_block(body);
                if let Some(facts) = facts {
                    code.fixed().leave_if_body(facts);
                }
            },
        );
    }
}
