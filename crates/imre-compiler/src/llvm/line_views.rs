//! Keep short decoded file lines in value slots when their only users inspect
//! prefixes, emptiness, or integer fields. Every assignment copies the view,
//! so an older binding remains independent across iterations and exceptions.
use super::*;
use ir::TextOperation;

fn source(e: &Expr) -> Option<usize> {
    match &e.kind {
        ExprKind::Local(slot) => Some(*slot),
        ExprKind::Builtin(Builtin::SplitIntSource(_), args) => local(&args[0]),
        _ => None,
    }
}

fn local(e: &Expr) -> Option<usize> {
    if let ExprKind::Local(slot) = e.kind {
        Some(slot)
    } else {
        None
    }
}

fn reader(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Builtin(Builtin::FileReadline, args) if args[0].ty == Type::TextIO)
}

fn affix(op: &Builtin, args: &[Expr]) -> bool {
    matches!(
        op,
        Builtin::Text(TextOperation::StartsWith | TextOperation::EndsWith)
    ) && matches!(args, [_, Expr { kind: ExprKind::Str(s), .. }] if s.len() <= 8)
}

pub(super) fn select(f: &Function) -> Vec<bool> {
    let mut slots = vec![false; f.locals.len()];
    ir::walk_statements(&f.body, &mut |s| {
        if let Statement::Assign(slot, value) = s
            && reader(value)
        {
            slots[*slot] = true;
        }
    });
    if !slots.iter().any(|x| *x) {
        return slots;
    }
    loop {
        let mut changed = false;
        ir::walk_statements(&f.body, &mut |s| {
            if let Statement::Assign(slot, value) = s
                && source(value).is_some_and(|s| slots[s])
                && !slots[*slot]
            {
                slots[*slot] = true;
                changed = true;
            }
        });
        if !changed {
            break;
        }
    }

    let mut safe = !f.params.iter().any(|&s| slots[s]);
    let mut uses = vec![0_isize; slots.len()];
    ir::walk_statements(&f.body, &mut |s| match s {
        Statement::Assign(slot, value) if slots[*slot] => {
            if let Some(from) = source(value) {
                safe &= slots[from];
                uses[from] -= 1;
            } else {
                safe &= reader(value);
            }
        }
        Statement::Move(a, b) => safe &= !slots[*a] && !slots[*b],
        Statement::For {
            target,
            cursor,
            stop,
            step,
            ..
        } => {
            safe &= [target, cursor, stop, step].into_iter().all(|s| !slots[*s]);
        }
        Statement::Try { handlers, .. } => {
            safe &= handlers
                .iter()
                .filter_map(|handler| handler.name)
                .all(|slot| !slots[slot]);
        }
        Statement::IterNext {
            iterator,
            key,
            value,
            ready,
            ..
        } => {
            safe &= [Some(*iterator), Some(*key), *value, Some(*ready)]
                .into_iter()
                .flatten()
                .all(|s| !slots[s]);
        }
        _ => {}
    });
    ir::walk_expressions(&f.body, &mut |e| match &e.kind {
        ExprKind::Local(s) => uses[*s] += 1,
        ExprKind::Bind(s, _) => safe &= !slots[*s],
        ExprKind::Builtin(op, args)
            if matches!(
                op,
                Builtin::Bool | Builtin::SplitIntField(_) | Builtin::SplitIntCached(_)
            ) || affix(op, args) =>
        {
            if let Some(s) = args.first().and_then(local) {
                uses[s] -= 1;
            }
        }
        _ => {}
    });
    if !safe || slots.iter().zip(uses).any(|(&chosen, n)| chosen && n != 0) {
        slots.fill(false);
    }
    slots
}

impl Emitter<'_, '_> {
    pub(super) fn copy_line(&mut self, from: usize, to: usize) {
        let bits = self.instruction(format!("load [4 x i64], ptr %line{from}"));
        self.line(format!("store [4 x i64] {bits}, ptr %line{to}"));
    }

    pub(super) fn line_data(&mut self, slot: usize) -> (String, String) {
        let len = self.instruction(format!("load i64, ptr %line{slot}"));
        let inline = self.instruction(format!(
            "getelementptr inbounds i64, ptr %line{slot}, i64 1"
        ));
        let spilled = self.instruction(format!("load ptr, ptr {inline}"));
        let short = self.instruction(format!("icmp ule i64 {len}, 24"));
        let data = self.instruction(format!("select i1 {short}, ptr {inline}, ptr {spilled}"));
        (data, len)
    }

    pub(super) fn assign_line(&mut self, slot: usize, e: &Expr) {
        if let Some(from) = source(e) {
            self.copy_line(from, slot);
            if let ExprKind::Builtin(Builtin::SplitIntSource(mask), _) = &e.kind {
                let (data, len) = self.line_data(slot);
                self.line(format!("call void @imre_span_prepare_int_fields(ptr {data}, i64 {len}, ptr %split_fields{slot}, i32 {mask})"));
            }
            let owner = self.borrow_slot(from);
            self.retain(&owner);
            self.store(slot, &owner);
        } else {
            let ExprKind::Builtin(Builtin::FileReadline, args) = &e.kind else {
                unreachable!()
            };
            let file = self.operand(&args[0]);
            let handle = self.checked_instruction(format!(
                "call ptr @imre_text_readline_inline(ptr {}, ptr %line{slot})",
                file.value.operand
            ));
            self.release_operand(&file);
            self.store(
                slot,
                &Value {
                    ty: Type::Str,
                    operand: handle,
                    small_integer: false,
                },
            );
        }
    }

    pub(super) fn line_builtin(&mut self, op: &Builtin, args: &[Expr]) -> Option<Value> {
        let slot = local(args.first()?)?;
        if !self.line_slots[slot] {
            return None;
        }
        let word = if matches!(op, Builtin::Bool) {
            let len = self.instruction(format!("load i64, ptr %line{slot}"));
            self.instruction(format!("icmp ne i64 {len}, 0"))
        } else if affix(op, args) {
            let ExprKind::Str(text) = &args[1].kind else {
                unreachable!()
            };
            let n = text.len();
            if n == 0 {
                "true".to_owned()
            } else {
                let (start, len) = self.line_data(slot);
                let (before, compare, end) =
                    (self.body.current.clone(), self.label(), self.label());
                let long = self.instruction(format!("icmp uge i64 {len}, {n}"));
                self.branch(&long, &compare, &end);
                self.start(&compare);
                let data = if matches!(op, Builtin::Text(TextOperation::EndsWith)) {
                    let offset = self.instruction(format!("sub i64 {len}, {n}"));
                    self.instruction(format!(
                        "getelementptr inbounds i8, ptr {start}, i64 {offset}"
                    ))
                } else {
                    start
                };
                let mut same = "true".to_owned();
                for (index, byte) in text.bytes().enumerate() {
                    let addr = self.instruction(format!(
                        "getelementptr inbounds i8, ptr {data}, i64 {index}"
                    ));
                    let actual = self.instruction(format!("load i8, ptr {addr}"));
                    let eq = self.instruction(format!("icmp eq i8 {actual}, {byte}"));
                    same = self.instruction(format!("and i1 {same}, {eq}"));
                }
                self.jump(&end);
                self.start(&end);
                self.instruction(format!("phi i1 [false, %{before}], [{same}, %{compare}]"))
            }
        } else if let Builtin::SplitIntField(index) = op {
            let (data, len) = self.line_data(slot);
            let operand = self.checked_instruction(format!(
                "call i64 @imre_span_split_int_field(ptr {data}, i64 {len}, i32 {index})"
            ));
            return Some(Value {
                ty: Type::Int,
                operand,
                small_integer: false,
            });
        } else {
            return None;
        };
        Some(Value {
            ty: Type::Bool,
            operand: word,
            small_integer: false,
        })
    }
}
