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

    fn emit_loop(
        &mut self,
        otherwise: &[Statement],
        enter: impl FnOnce(&mut Self, &LoopEdges),
        test: impl FnOnce(&mut Self, &str, &str),
        body: impl FnOnce(&mut Self, &LoopEdges),
    ) {
        let step = self.block_name();
        let body_label = self.block_name();
        let done = self.block_name();
        let exhausted = if otherwise.is_empty() {
            done.clone()
        } else {
            self.block_name()
        };
        let edges = LoopEdges {
            step,
            exhausted,
            done,
        };
        enter(self, &edges);
        self.body().start(&edges.step);
        test(self, &body_label, &edges.exhausted);
        self.body().start(&body_label);
        body(self, &edges);
        self.jump_open(&edges.step);
        if !otherwise.is_empty() {
            self.body().start(&edges.exhausted);
            self.emit_block(otherwise);
            self.jump_open(&edges.done);
        }
        self.body().start(&edges.done);
    }

    pub(super) fn clear_iteration_slots(&mut self, slots: &[usize]) {
        match &mut self.0 {
            Entry::Native(e) => e.clear_slots(slots),
            Entry::Registered(f) => f.clear_slots(slots),
        }
    }

    pub(super) fn scoped_suite(&mut self, statements: &[Statement], clear: &[usize]) {
        match &mut self.0 {
            Entry::Native(e) => e.cleanups.push(Cleanup::Slots(clear.to_vec())),
            Entry::Registered(f) => f.enter_scope(clear),
        }
        self.emit_block(statements);
        match &mut self.0 {
            Entry::Native(e) => {
                e.cleanups.pop();
            }
            Entry::Registered(f) => f.leave_scope(),
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
            Entry::Registered(f) => Some(f.start_expression()),
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
        self.emit_loop(
            otherwise,
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
            |this, edges| {
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
        debug_assert!(value.is_none() && matches!(kind, IterNextKind::Generic));
        let (cursor, condition) = self.python().pull_ready(iterator, ready);
        self.when(&condition, |this| this.python().pull_item(&cursor, key));
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
            Entry::Registered(f) => f.target_mark(),
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
                f.release_target(mark.as_deref());
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
        super::join::choose(
            self,
            Self::body,
            |code, t, f| {
                if let Some(facts) = &facts {
                    let bit = code.fixed().expr(test).operand;
                    code.fixed().if_edges(stmt, &bit, facts, t, f);
                } else {
                    code.branch_condition(test, t, f);
                }
            },
            |code| {
                if let Some(facts) = &facts {
                    code.fixed().enter_if_body(test, facts);
                }
                code.emit_block(yes);
                if let Some(facts) = &facts {
                    code.fixed().leave_if_body(facts);
                }
            },
            |code| code.emit_block(no),
            |_, ()| {},
        );
    }
}
