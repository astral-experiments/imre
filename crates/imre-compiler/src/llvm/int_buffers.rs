//! Borrow the backing storage of stable integer-list parameters during loops
//! that only read or overwrite integer elements. Aliases keep seeing each store.
use super::*;
use std::collections::BTreeSet;

/// A fresh local integer list can keep its backing buffer across calls when its
/// only observed uses are scalar reads, overwrites, or length checks.
pub(super) fn fixed_locals(f: &Function) -> Vec<bool> {
    let mut starts = HashSet::new();
    let mut fixed = vec![false; f.locals.len()];
    for statement in &f.body {
        if let Statement::Assign(slot, value) = statement
            && value.ty == Type::ListInt
            && matches!(
                value.kind,
                ExprKind::List(_) | ExprKind::Binary(Binary::Multiply, _, _)
            )
            && !f.params.contains(slot)
        {
            starts.insert(std::ptr::from_ref(value));
            fixed[*slot] = true;
        }
    }
    if starts.is_empty() {
        return fixed;
    }
    let changed = borrows::written_except(f, |value| starts.contains(&std::ptr::from_ref(value)));
    let mut uses = vec![0_isize; f.locals.len()];
    ir::walk_statements(&f.body, &mut |s| {
        if let Statement::SetItem(
            _,
            Expr {
                kind: ExprKind::Local(slot),
                ..
            },
            _,
            _,
        ) = s
        {
            uses[*slot] -= 1;
        }
    });
    ir::walk_expressions(&f.body, &mut |value| match &value.kind {
        ExprKind::Local(slot) => uses[*slot] += 1,
        ExprKind::Builtin(Builtin::ListGet(_) | Builtin::Len | Builtin::Bool, args) => {
            if let Some(Expr {
                kind: ExprKind::Local(slot),
                ..
            }) = args.first()
            {
                uses[*slot] -= 1;
            }
        }
        _ => {}
    });
    for (slot, fixed) in fixed.iter_mut().enumerate() {
        *fixed &= !changed[slot] && uses[slot] == 0;
    }
    fixed
}

fn scalar(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) => matches!(e.ty, Type::Int | Type::Bool | Type::ListInt),
        ExprKind::Int(_) | ExprKind::Bool(_) => true,
        ExprKind::Binary(_, a, b) => {
            a.ty == Type::Int && b.ty == Type::Int && scalar(a) && scalar(b)
        }
        ExprKind::Unary(_, v) => !e.uses_python_unary_protocol() && scalar(v),
        ExprKind::Boolean(_, args) => args.iter().all(scalar),
        ExprKind::Compare(args, ops) => {
            !Compare::uses_python_protocol(e.ty, ops) && args.iter().all(scalar)
        }
        ExprKind::Builtin(Builtin::ListGet(_) | Builtin::Len | Builtin::Bool, args) => {
            args.iter().all(scalar)
        }
        _ => false,
    }
}

fn updates(emit: &Emitter<'_, '_>, st: &Statement, targets: &mut BTreeSet<usize>) -> bool {
    match st {
        Statement::SetItem(_, receiver, index, item) => {
            let ExprKind::Local(slot) = receiver.kind else {
                return false;
            };
            let ExprKind::Builtin(Builtin::ListGet(_), args) = &item.kind else {
                return false;
            };
            let [source, from] = args.as_slice() else {
                return false;
            };
            if receiver.ty != Type::ListInt
                || !emit.borrowed_params[slot]
                || !matches!(source.kind, ExprKind::Local(s) if s == slot)
                || ![index, from].into_iter().all(|e| {
                    matches!(e.kind, ExprKind::Int(_) | ExprKind::Local(_))
                        && emit.nonnegative_index(e)
                })
            {
                return false;
            }
            targets.insert(slot);
            true
        }
        Statement::Assign(slot, v) => {
            matches!(emit.locals[*slot], Type::Int | Type::Bool) && scalar(v)
        }
        Statement::While(test, a, b) | Statement::If(test, a, b) => {
            scalar(test) && a.iter().chain(b).all(|s| updates(emit, s, targets))
        }
        Statement::Position(..)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::Exhaust => true,
        _ => false,
    }
}

impl Emitter<'_, '_> {
    /// After the last resize or call, root each incoming list once for a
    /// scalar loop and its surrounding reads and writes. Parameters may alias:
    /// the view caches addresses and bounds, never element values.
    pub(super) fn int_buffer_region(&self, f: &Function) -> Option<(usize, BTreeSet<usize>)> {
        fn fixed_region(s: &Statement, types: &[Type]) -> bool {
            match s {
                Statement::Assign(slot, value) => {
                    matches!(types[*slot], Type::Int | Type::Bool) && scalar(value)
                }
                Statement::SetItem(_, receiver, index, value) => {
                    matches!(receiver.kind, ExprKind::Local(_))
                        && receiver.ty == Type::ListInt
                        && scalar(index)
                        && scalar(value)
                }
                Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
                    scalar(test) && yes.iter().chain(no).all(|s| fixed_region(s, types))
                }
                Statement::Return(value) => scalar(value) || matches!(value.kind, ExprKind::None),
                Statement::Position(..)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::Exhaust => true,
                _ => false,
            }
        }
        let start = f
            .body
            .iter()
            .rposition(|s| !fixed_region(s, &f.locals))
            .map_or(0, |i| i + 1);
        let tail = &f.body[start..];
        let mut has_loop = false;
        ir::walk_statements(tail, &mut |s| has_loop |= matches!(s, Statement::While(..)));
        if !has_loop {
            return None;
        }
        let mut slots = BTreeSet::new();
        ir::walk_expressions(tail, &mut |e| {
            if let ExprKind::Local(slot) = e.kind
                && e.ty == Type::ListInt
                && self.borrowed_params[slot]
            {
                slots.insert(slot);
            }
        });
        (!slots.is_empty()).then_some((start, slots))
    }

    pub(super) fn enter_int_buffer_region(&mut self, slots: BTreeSet<usize>) {
        for slot in slots {
            let list = self.borrow_slot(slot);
            self.remember_int_buffer(slot, &list);
            self.fixed_int_lists[slot] = true;
        }
    }

    /// Return the stable backing storage selected for a local integer list.
    pub(super) fn fixed_int_buffer_slot(&self, source: &Expr) -> Option<usize> {
        match source.kind {
            ExprKind::Local(slot)
                if self.fixed_int_lists[slot] && self.int_buffers.contains_key(&slot) =>
            {
                Some(slot)
            }
            _ => None,
        }
    }

    pub(super) fn remember_int_buffer(&mut self, slot: usize, list: &Value) {
        let count = self.entry_alloca("i64");
        let data = self.instruction(format!(
            "call ptr @imre_list_update_view(ptr {}, ptr {count})",
            list.operand
        ));
        let len = self.instruction(format!("load i64, ptr {count}"));
        self.int_buffers.insert(slot, (data, len));
    }

    /// The backing storage is stable. Normalize uncommon indices at the
    /// original subscript, using the same bounds and overflow errors as a list.
    fn int_buffer_address(&mut self, slot: usize, index: &Value) -> String {
        let (data, len) = self.int_buffers[&slot].clone();
        let word = self.word(index);
        let tag = self.instruction(format!("trunc i64 {word} to i1"));
        let at = self.instruction(format!("lshr i64 {word}, 1"));
        let within = self.instruction(format!("icmp ult i64 {at}, {len}"));
        let valid = self.instruction(format!("and i1 {tag}, {within}"));
        let (fast, slow, done) = (self.body.current.clone(), self.label(), self.label());
        self.branch(&valid, &done, &slow);
        self.start(&slow);
        let normalized = self.checked_int_list_word(format!(
            "call i64 @imre_list_view_index(i64 {word}, i64 {len})"
        ));
        let other = self.instruction(format!("lshr i64 {normalized}, 1"));
        let fallback = self.body.current.clone();
        self.jump(&done);
        self.start(&done);
        let i = self.instruction(format!("phi i64 [{at}, %{fast}], [{other}, %{fallback}]"));
        self.instruction(format!("getelementptr inbounds i64, ptr {data}, i64 {i}"))
    }

    pub(super) fn fixed_int_get(&mut self, slot: usize, at: &Value) -> Value {
        let address = self.int_buffer_address(slot, at);
        let word = self.instruction(format!("load i64, ptr {address}"));
        let value = self.decode_word(word, Type::Int);
        self.retain(&value);
        value
    }

    pub(super) fn fixed_int_set(&mut self, slot: usize, at: &Value, value: &Value) {
        let address = self.int_buffer_address(slot, at);
        let word = self.word(value);
        self.replace_int_word(&address, &word);
    }

    /// Acquiring first also covers shared heap integers and self-assignment.
    fn replace_int_word(&mut self, address: &str, word: &str) {
        let old = self.instruction(format!("load i64, ptr {address}"));
        self.line(format!("call void @retain_int_nonzero(i64 {word})"));
        self.line(format!("store i64 {word}, ptr {address}"));
        self.line(format!("call void @release_int_nonzero(i64 {old})"));
    }

    pub(super) fn int_buffer_loop(
        &mut self,
        statement: &Statement,
    ) -> BTreeMap<usize, (String, String)> {
        let previous = self.int_buffers.clone();
        let mut writes = BTreeSet::new();
        if !updates(self, statement, &mut writes) || writes.is_empty() {
            return previous;
        }
        // Incoming parameters with no rebinding stay rooted for the whole call.
        // Taking two views of the same list is valid; neither caches any elements.
        for slot in writes {
            if self.int_buffers.contains_key(&slot) {
                continue;
            }
            let receiver = self.borrow_slot(slot);
            self.remember_int_buffer(slot, &receiver);
        }
        previous
    }

    /// The slow edge preserves the original checked operation and error. A
    /// returned word is still borrowed only for the original comparison.
    pub(super) fn int_buffer_borrow(
        &mut self,
        slot: usize,
        list: &Value,
        index: &Value,
    ) -> Option<Value> {
        let (data, len) = self.int_buffers.get(&slot)?.clone();
        let at = self.index_word(index, true);
        let within = self.instruction(format!("icmp ult i64 {at}, {len}"));
        let (read, slow, done) = (self.label(), self.label(), self.label());
        self.branch(&within, &read, &slow);
        self.start(&read);
        let value = self.load_list_word(&data, &at);
        self.jump(&done);
        self.start(&slow);
        let other = self.checked_int_list_word(format!(
            "call i64 @imre_list_borrow_index(ptr {}, i64 {at})",
            list.operand
        ));
        let fallback = self.body.current.clone();
        self.jump(&done);
        self.start(&done);
        let word = self.instruction(format!(
            "phi i64 [{value}, %{read}], [{other}, %{fallback}]"
        ));
        Some(self.decode_word(word, Type::Int))
    }

    /// Immediate-to-immediate copies bypass reference counting. Invalid indices
    /// use the original source-before-destination check and leave the loop.
    pub(super) fn int_buffer_copy(
        &mut self,
        slot: usize,
        list: &Value,
        source: &Value,
        target: &Value,
    ) -> bool {
        let Some((data, len)) = self.int_buffers.get(&slot).cloned() else {
            return false;
        };
        let from = self.index_word(source, true);
        let to = self.index_word(target, true);
        let source_ok = self.instruction(format!("icmp ult i64 {from}, {len}"));
        let target_ok = self.instruction(format!("icmp ult i64 {to}, {len}"));
        let both = self.instruction(format!("and i1 {source_ok}, {target_ok}"));
        let (read, write, owners, slow, done) = (
            self.label(),
            self.label(),
            self.label(),
            self.label(),
            self.label(),
        );
        self.branch(&both, &read, &slow);
        self.start(&read);
        let value = self.load_list_word(&data, &from);
        let address = self.instruction(format!("getelementptr inbounds i64, ptr {data}, i64 {to}"));
        let old = self.instruction(format!("load i64, ptr {address}"));
        let tags = self.instruction(format!("and i64 {old}, {value}"));
        let immediate = self.instruction(format!("trunc i64 {tags} to i1"));
        self.branch(&immediate, &write, &owners);
        self.start(&write);
        self.line(format!("store i64 {value}, ptr {address}"));
        self.jump(&done);
        self.start(&owners);
        // Acquire before replacing so even a BigInt self-copy stays rooted.
        // The only mutable element access in this region uses the exported view.
        self.line(format!("call void @imre_int_retain(i64 {value})"));
        self.line(format!("store i64 {value}, ptr {address}"));
        self.line(format!("call void @imre_int_release(i64 {old})"));
        self.jump(&done);
        self.start(&slow);
        self.checked_status(format!(
            "call i32 @imre_list_copy_index(ptr {}, i64 {to}, i64 {from})",
            list.operand
        ));
        self.jump(&done);
        self.start(&done);
        true
    }
}
