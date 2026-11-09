//! Shared traversal of loop and unpacking IR for fixed and registered entries.
use super::{Builtin, CheckedBody, CheckedValue, Entry, Expr, ExprKind, Statement};
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
        test: impl FnOnce(&mut Self) -> String,
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
        let condition = test(self);
        self.body()
            .branch(&condition, &body_label, &edges.exhausted);
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

    pub(super) fn object_operand(&mut self, expr: &Expr) -> String {
        let v = self.operand(expr);
        self.python().materialize(&v)
    }

    pub(super) fn iteration_operation(&mut self, op: &Builtin, operands: &[Expr]) -> CheckedValue {
        let result = match (op, operands) {
            (Builtin::RegisteredIter, [value]) => {
                let value = self.object_operand(value);
                self.python().op("iter", &[&value])
            }
            (Builtin::RegisteredUnpack { count, star }, [value]) => {
                let value = self.object_operand(value);
                self.python().unpack(&value, *count, *star)
            }
            (Builtin::RegisteredNamespace(names), args) => {
                let parent = args.first().map(|v| self.object_operand(v));
                self.python().comprehension_scope(parent, names)
            }
            (Builtin::ListGet, [base, index]) => {
                let base = self.object_operand(base);
                let key = self.object_operand(index);
                self.python().op("get", &[&base, &key])
            }
            (Builtin::Bool, [value]) => {
                let v = self.object_operand(value);
                self.python().op("truth", &[&v])
            }
            (Builtin::Append | Builtin::SetAdd, [out, item]) => {
                let out = self.object_operand(out);
                let item = self.object_operand(item);
                self.python().op("append", &[&out, &item])
            }
            _ => unreachable!("invalid checked iteration operands"),
        };
        CheckedValue::handle(result)
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
        let mark = match &mut self.0 {
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
        if let Some((mark, outer)) = mark {
            self.python().finish_expression(&mark, outer, &result);
        }
        result
    }

    fn object_condition(&mut self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Bool(value) => return value.to_string(),
            ExprKind::Not(value) => {
                let inner = self.object_condition(value);
                return self.python().invert(&inner);
            }
            ExprKind::Builtin(Builtin::Bool, args) => return self.object_condition(&args[0]),
            _ => {}
        }
        let mark = self.python().mark();
        let value = self.object_operand(expr);
        let condition = self.python().truth(&value);
        self.python().drop_truth(&mark);
        condition
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
            |this| {
                if let Entry::Native(e) = &mut this.0 {
                    e.check_interrupt();
                    e.while_condition(
                        test,
                        optimized.as_ref().is_some_and(|(_, _, s)| s.is_some()),
                    )
                } else {
                    if let Some(mark) = &mark {
                        this.python().begin_step(mark);
                    }
                    this.object_condition(test)
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
                f.target_release(mark.as_deref());
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
        statement: &Statement,
        test: &Expr,
        yes: &[Statement],
        no: &[Statement],
    ) {
        let (yes_label, no_label, end) = (self.block_name(), self.block_name(), self.block_name());
        let state = if let Entry::Native(e) = &mut self.0 {
            let mut bounded = false;
            if crate::list_specialization::is_window_range_guard(test)
                && let [Statement::While(_, body, _)] = yes
            {
                crate::ir::walk_expressions(body, &mut |value| {
                    bounded |= matches!(
                        value.kind,
                        ExprKind::Builtin(Builtin::GetBoundedInAppendWindow { .. }, _)
                    );
                });
            }
            let slots = if bounded {
                e.ranges
                    .checked_small_slots(yes, &e.locals, &e.small_locals)
            } else {
                Vec::new()
            };
            let reserve = bounded
                .then(|| crate::list_specialization::reserved_append_window(yes))
                .flatten();
            let reserve = reserve.and_then(|(slot, cursor)| {
                e.forward_cursors.get(&cursor).cloned().map(|c| (slot, c))
            });
            let previous = e.reserved_append_window.clone();
            let carried = reserve
                .as_ref()
                .map(|(slot, _)| (*slot, e.entry_alloca("i64")));
            if carried.is_some() {
                e.reserved_append_window.clone_from(&carried);
            }
            let condition = e.expr(test);
            e.reserved_append_window.clone_from(&previous);
            let mut guard = condition.operand;
            let unknown = slots
                .iter()
                .copied()
                .filter(|slot| !e.ranges.starts_small(statement, *slot))
                .collect::<Vec<_>>();
            if let Some((first, rest)) = unknown.split_first() {
                let mut words = e.borrow_slot(*first).operand;
                for slot in rest {
                    let word = e.borrow_slot(*slot).operand;
                    words = e.instruction(format!("and i64 {words}, {word}"));
                }
                let tagged = e.instruction(format!("trunc i64 {words} to i1"));
                guard = e.instruction(format!("and i1 {guard}, {tagged}"));
            }
            if let Some((slot, cursor)) = reserve {
                let check = e.label();
                e.branch(&guard, &check, &no_label);
                e.start(&check);
                let bytes = e.remaining_cursor_bytes(&cursor);
                let list = e.borrow_slot(slot).operand;
                let window = e.append_window(slot);
                let success = e.instruction(format!(
                        "call i32 @imre_list_try_open_append_window(ptr {list}, i64 {bytes}, ptr {window})"
                    ));
                let opened = e.instruction(format!("icmp ne i32 {success}, 0"));
                e.branch(&opened, &yes_label, &no_label);
                e.reserved_append_window.clone_from(&carried);
            } else {
                e.branch(&guard, &yes_label, &no_label);
            }
            e.start(&yes_label);
            if let Some((slot, tail)) = carried {
                // Opening the reserved window proves the list has an immediate tail.
                let window = e.append_window(slot);
                let cursor = e.instruction(format!(
                    "getelementptr [3 x ptr], ptr {window}, i32 0, i32 1"
                ));
                let next = e.instruction(format!("load ptr, ptr {cursor}"));
                let last = e.instruction(format!("getelementptr i64, ptr {next}, i64 -1"));
                let word = e.instruction(format!("load i64, ptr {last}"));
                e.line(format!("store i64 {word}, ptr {tail}"));
            }
            let prior = e.guarded_integer_lists.len();
            e.borrow_guarded_integer_lists(test);
            for slot in &slots {
                e.small_locals[*slot] = true;
            }
            Some((previous, slots, prior))
        } else {
            let condition = self.object_condition(test);
            self.body().branch(&condition, &yes_label, &no_label);
            self.body().start(&yes_label);
            None
        };
        self.emit_block(yes);
        if let Some((previous, slots, prior)) = state {
            let e = self.fixed();
            e.reserved_append_window = previous;
            for slot in slots {
                e.small_locals[slot] = false;
            }
            e.guarded_integer_lists.truncate(prior);
        }
        let yes_ends = self.body().terminated;
        self.jump_open(&end);
        self.body().start(&no_label);
        self.emit_block(no);
        let no_ends = self.body().terminated;
        self.jump_open(&end);
        self.body().start(&end);
        if yes_ends && no_ends {
            self.body().end("unreachable");
        }
    }
}
