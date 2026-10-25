//! Scalar struct payloads may live on the stack only when every consumer reads
//! fields. Other uses get ordinary independently owned record snapshots.
use super::*;

/// ListGet also represents dictionary subscriptions; only a list carries rows.
fn packed_get(item: &Expr) -> Option<(&Builtin, &[Expr])> {
    if let ExprKind::Builtin(op @ (Builtin::SequenceGetInBounds | Builtin::ListGet), args) =
        &item.kind
        && matches!(args[0].ty, Type::List(_))
    {
        Some((op, args))
    } else {
        None
    }
}

pub(super) fn field_reader(function: &Function) -> bool {
    let &[slot] = function.params.as_slice() else {
        return false;
    };
    if !matches!(function.body.as_slice(), [Statement::Return(_)])
        || borrows::written(function)[slot]
    {
        return false;
    }
    let (mut reads, mut fields) = (0, 0);
    crate::ir::walk_expressions(&function.body, &mut |e| match &e.kind {
        ExprKind::Local(s) if *s == slot => reads += 1,
        ExprKind::Field(src, _) if matches!(src.kind, ExprKind::Local(s) if s == slot) => {
            fields += 1
        }
        _ => {}
    });
    reads != 0 && reads == fields
}

/// No headerless pointer may escape to a retain, identity test, or wrapper.
/// A copied local stays valid even when the loop body reallocates the list.
pub(super) fn snapshot_locals(module: &Module, f: &Function) -> Vec<bool> {
    let mut eligible: Vec<_> = f
        .locals
        .iter()
        .map(|&ty| module.types.is_value_struct(ty))
        .collect();
    if !eligible.contains(&true) {
        return eligible;
    }
    for &p in &f.params {
        eligible[p] = false;
    }
    let mut assigned = vec![false; eligible.len()];
    let mut reads = vec![0; eligible.len()];
    let mut safe = reads.clone();
    borrows::statements(&f.body, &mut |s| match s {
        Statement::Assign(slot, e) => {
            assigned[*slot] = true;
            eligible[*slot] &= packed_get(e).is_some() && e.ty == f.locals[*slot];
        }
        Statement::Move(a, b) => {
            eligible[*a] = false;
            eligible[*b] = false;
        }
        Statement::IterNext { key, value, .. } => {
            eligible[*key] = false;
            if let Some(v) = value {
                eligible[*v] = false;
            }
        }
        _ => {}
    });
    crate::ir::walk_expressions(&f.body, &mut |e| match &e.kind {
        ExprKind::Local(s) => reads[*s] += 1,
        ExprKind::Bind(s, _) => eligible[*s] = false,
        ExprKind::Field(src, _) => {
            if let ExprKind::Local(s) = src.kind {
                safe[s] += 1;
            }
        }
        ExprKind::Call(id, args) => {
            if let [arg] = args.values.as_slice()
                && let ExprKind::Local(s) = arg.kind
                && eligible[s]
                && field_reader(&module.functions[*id])
            {
                safe[s] += 1;
            }
        }
        _ => {}
    });
    for (i, v) in eligible.iter_mut().enumerate() {
        *v &= assigned[i] && reads[i] == safe[i];
    }
    eligible
}

impl Emitter<'_, '_> {
    /// Evaluate constructor operands in source order; copy only their checked
    /// scalar values to the destination in declared field order.
    pub(super) fn append_struct_constructor(&mut self, list: &Value, item: &Expr) -> bool {
        let ExprKind::Construct { args, .. } = &item.kind else {
            return false;
        };
        if !self.module.types.is_value_struct(item.ty) {
            return false;
        }
        let name = record_name(item.ty);
        let scratch = self.entry_alloca(format!("%{name}"));
        let layout = self.output.global_ref(format!("layout_{name}"));
        let values = self.call_operands(args);
        for (i, &source) in args.order.iter().enumerate() {
            let v = &values[source].value;
            let p = self.instruction(format!(
                "getelementptr inbounds %{name}, ptr {scratch}, i32 0, i32 {i}"
            ));
            let word = self.word(v);
            self.line(format!("store i64 {word}, ptr {p}"));
        }
        self.checked_status(format!(
            "call i32 @imre_struct_append(ptr {}, ptr @{layout}, ptr {scratch})",
            list.operand
        ));
        for value in &values {
            self.release_operand(value);
        }
        true
    }

    /// Snapshot one complete row before releasing either operand or allowing
    /// user code to mutate the list. LLVM can scalarize the copy at its uses.
    pub(super) fn struct_snapshot(&mut self, item: &Expr) -> Option<Value> {
        if !self.module.types.is_value_struct(item.ty) {
            return None;
        }
        let (op, args) = packed_get(item)?;
        let list = self.operand(&args[0]);
        let index = self.operand(&args[1]);
        let name = record_name(item.ty);
        let src = if matches!(op, Builtin::SequenceGetInBounds) {
            let ExprKind::Local(slot) = args[0].kind else {
                unreachable!()
            };
            let native = self.instruction(format!("lshr i64 {}, 1", index.value.operand));
            let view = self.length_roots[slot].and_then(|root| self.length_snapshots[root].clone());
            if let Some(view) = view
                && let Some(ListBuffer::Struct {
                    data,
                    index_mask,
                    cursor,
                }) = view.data
            {
                let current = self.instruction(format!(
                    "icmp eq ptr {}, {}",
                    list.value.operand, view.receiver.operand
                ));
                let packed = self.instruction(format!("icmp ne ptr {data}, null"));
                let cached = self.instruction(format!("and i1 {current}, {packed}"));
                let (yes, no, end) = (self.label(), self.label(), self.label());
                self.branch(&cached, &yes, &no);
                self.start(&yes);
                // A repeated list uses one immutable row (mask zero) for every index.
                let physical = self.instruction(format!("and i64 {native}, {index_mask}"));
                let p = self.instruction(format!(
                    "getelementptr inbounds %{name}, ptr {data}, i64 {physical}"
                ));
                self.jump(&end);
                self.start(&no);
                let (same, other) = (self.label(), self.label());
                self.branch(&current, &same, &other);
                self.start(&same);
                let Type::Class(class) = item.ty else {
                    unreachable!()
                };
                let width = self.module.class_fields[class].len();
                let sparse = self.instruction(format!(
                    "call ptr @imre_struct_cursor_row(ptr {}, i64 {native}, ptr {cursor}, i64 {width})",
                    list.value.operand
                ));
                self.jump(&end);
                self.start(&other);
                let alt = self.instruction(format!(
                    "call ptr @imre_struct_row(ptr {}, i64 {native})",
                    list.value.operand
                ));
                self.jump(&end);
                self.start(&end);
                self.instruction(format!(
                    "phi ptr [{p}, %{yes}], [{sparse}, %{same}], [{alt}, %{other}]"
                ))
            } else {
                self.instruction(format!(
                    "call ptr @imre_struct_row(ptr {}, i64 {native})",
                    list.value.operand
                ))
            }
        } else {
            self.checked_pointer(format!(
                "call ptr @imre_struct_row_checked(ptr {}, i64 {})",
                list.value.operand, index.value.operand
            ))
        };
        let scratch = self.entry_alloca(format!("%{name}"));
        let value = self.instruction(format!("load %{name}, ptr {src}"));
        self.line(format!("store %{name} {value}, ptr {scratch}"));
        self.release_operand(&index);
        self.release_operand(&list);
        Some(Value {
            ty: item.ty,
            operand: scratch,
            small_integer: false,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn packed_build_and_scan_have_no_per_row_record_allocations() {
        let source = r#"
from imre import struct
@struct
class Point:
    x: float
    y: float
    def norm2(self) -> float:
        return self.x * self.x + self.y * self.y
def build(n: int) -> list[Point]:
    result: list[Point] = []
    for i in range(n):
        result.append(Point(float(i), 1.0))
    return result
def scan(points: list[Point]) -> float:
    total = 0.0
    for p in points:
        total += p.norm2()
    return total
print(scan(build(8)))
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        let bodies: Vec<_> = llvm
            .split("\ndefine ")
            .skip(1)
            .map(|s| s.split_once("\n}").unwrap().0)
            .collect();
        let build = bodies
            .iter()
            .find(|f| f.contains("call i32 @imre_struct_append("))
            .unwrap();
        let scan = bodies
            .iter()
            .find(|f| f.contains("call ptr @imre_struct_read_view("))
            .unwrap();
        assert!(build.contains("call ptr @imre_struct_list_new("));
        assert!(scan.contains("getelementptr inbounds %class_0, ptr"));
        for routine in [build, scan] {
            for heap_op in [
                "call ptr @imre_record_try_new(",
                "call void @imre_record_retain(",
                "call i64 @imre_seq_get_index(",
            ] {
                assert!(
                    !routine.contains(heap_op),
                    "found {heap_op} in struct hot loop"
                );
            }
        }
    }
}
