//! Shared traversal of loop and unpacking IR for fixed and registered entries.
use super::{CheckedBody, CheckedValue, Entry, Expr, ExprKind, Statement};
use crate::ir::IterNextKind;
use crate::llvm::{Cleanup, Completion, Emitter, LoopTarget};

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
    /// A specialized pull uses the same guarded-success edge as the Python iterator.
    pub(in crate::llvm) fn when(&mut self, condition: &str, success: impl FnOnce(&mut Self)) {
        CheckedBody::native(self).when(condition, |body| success(body.fixed()));
    }
}

impl CheckedBody<'_, '_, '_, '_> {
    fn block_name(&mut self) -> String {
        match &mut self.0 {
            Entry::Native(e) => e.label(),
            Entry::Registered(f) => f.block_name(),
        }
    }
    fn jump_open(&mut self, dest: &str) {
        let body = self.body();
        if !body.terminated {
            body.jump(dest);
        }
    }

    /// A failed pull leaves its outputs untouched. The same guard also
    /// branches around a filtered suite without evaluating any of its effects.
    fn when(&mut self, test: &str, yes: impl FnOnce(&mut Self)) {
        let take = self.block_name();
        let end = self.block_name();
        self.body().branch(test, &take, &end);
        self.body().start(&take);
        yes(self);
        self.jump_open(&end);
        self.body().start(&end);
    }

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
                    f.finish_unwind(unwind, |code| code.release_target(None));
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
        let optimized = match &mut self.0 {
            Entry::Native(e) => Some((
                e.snapshot_loop_lengths(stmt),
                e.int_buffer_loop(stmt),
                e.raw_subtotal(body),
            )),
            Entry::Registered(_) => None,
        };
        let mark = match &mut self.0 {
            Entry::Registered(f) => f.target_mark(),
            _ => None,
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
                    let bit = e.while_condition(
                        test,
                        optimized.as_ref().is_some_and(|(_, _, s)| s.is_some()),
                    );
                    e.branch(&bit, yes, no);
                } else {
                    if let Some(mark) = &mark {
                        this.python().begin_step(mark);
                    }
                    this.branch_condition(test, yes, no);
                }
            },
            |this, edges, ()| {
                match &mut this.0 {
                    Entry::Native(e) => e.loops.push(LoopTarget {
                        end: edges.done.clone(),
                        exhausted: edges.exhausted.clone(),
                        next: edges.step.clone(),
                        cleanup_depth: e.cleanups.len(),
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
    ) {
        if let Entry::Native(e) = &mut self.0 {
            e.iter_next(iterator, key, value, ready, kind);
            return;
        }
        debug_assert!(value.is_none());
        self.python().pull(iterator, key, ready, kind);
    }

    pub(super) fn loop_transfer(&mut self, statement: &Statement) {
        match &mut self.0 {
            Entry::Registered(f) => f.transfer_loop(statement),
            Entry::Native(e) => {
                let target = e.loops.last().expect("loop transfer");
                e.route(Completion::Jump {
                    label: match statement {
                        Statement::Break => target.end.clone(),
                        Statement::Exhaust => target.exhausted.clone(),
                        _ => target.next.clone(),
                    },
                    depth: target.cleanup_depth,
                });
            }
        }
    }

    /// A Python setter needs each real object before the next target callback can run.
    fn store_operand(&mut self, expr: &Expr) -> CheckedValue {
        let value = self.operand(expr);
        match &mut self.0 {
            Entry::Native(_) => value,
            Entry::Registered(frame) => CheckedValue::handle(frame.materialize(&value)),
        }
    }

    pub(super) fn store_item(&mut self, table: &Expr, key: &Expr, value: &Expr) {
        let fixed = match &mut self.0 {
            Entry::Native(e) => {
                if e.copy_item(table, key, value) {
                    return;
                }
                let slot = match table.kind {
                    ExprKind::Local(slot)
                        if e.fixed_int_lists[slot] && e.int_buffers.contains_key(&slot) =>
                    {
                        Some(slot)
                    }
                    _ => None,
                };
                Some((slot, e.nonnegative_index(key)))
            }
            Entry::Registered(_) => None,
        };
        let mark = match &mut self.0 {
            Entry::Registered(f) => {
                let target = f.target_mark();
                let operands = target.clone().unwrap_or_else(|| f.temporary_mark());
                Some((target, operands))
            }
            _ => None,
        };
        let value = self.store_operand(value);
        let table = self.store_operand(table);
        let key = self.store_operand(key);
        match &mut self.0 {
            Entry::Registered(f) => {
                let v = value.into_handle();
                let b = table.into_handle();
                let k = key.into_handle();
                f.op("put", &[&b, &k, &v]);
                // STORE_SUBSCR pops key and receiver before the RHS. The target
                // checkpoint also releases any earlier snapshot of a taken slot.
                let (target, operands) = mark.expect("registered store marks");
                f.release_owned_operands(&operands);
                f.release_target(target.as_deref());
            }
            Entry::Native(e) => {
                let (list, index, value) =
                    (table.into_native(), key.into_native(), value.into_native());
                let (fixed_slot, nonnegative) = fixed.expect("native store layout");
                if let Some(slot) = fixed_slot {
                    e.fixed_int_set(slot, &index.value, &value.value);
                } else {
                    e.set_item(&list.value, &index.value, &value.value, nonnegative);
                }
                if e.module.types.mapping(list.value.ty).is_some()
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
        let resumable = matches!(&self.0, Entry::Registered(frame) if frame.has_region_inputs());
        super::join::choose(
            self,
            Self::body,
            [(Some(test), (yes, facts.as_ref())), (None, (no, None))],
            |code, test, t, f| {
                if let Some(facts) = &facts {
                    let bit = code.fixed().expr(test).operand;
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
            resumable,
        );
    }
}
