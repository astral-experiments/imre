//! Sum identical scalar struct rows in a single step when binary64 addition is exact.
use super::*;

struct RunSum<'a> {
    list: usize,
    cursor: usize,
    row: usize,
    accumulator: usize,
    term: &'a Expr,
}

/// Restrict the projection to total floating-point arithmetic and immutable fields.
/// A direct single-argument method may contain the same arithmetic in its return.
fn pure_term(
    module: Option<&Module>,
    value: &Expr,
    row: usize,
    accumulator: Option<usize>,
) -> bool {
    if value.ty != Type::Float {
        return false;
    }
    match &value.kind {
        ExprKind::Float(_) => true,
        ExprKind::Local(slot) => accumulator.is_some_and(|sum| *slot != sum),
        ExprKind::Field(receiver, _) => {
            matches!(receiver.kind, ExprKind::Local(slot) if slot == row)
        }
        ExprKind::Negate(inner) => pure_term(module, inner, row, accumulator),
        ExprKind::Binary(Binary::Add | Binary::Subtract | Binary::Multiply, left, right) => {
            pure_term(module, left, row, accumulator) && pure_term(module, right, row, accumulator)
        }
        ExprKind::Call(id, args) => {
            let Some(module) = module else { return false };
            let f = &module.functions[*id];
            matches!((args.values.as_slice(), f.params.as_slice(), f.body.as_slice()),
                ([arg], [receiver], [Statement::Return(result)])
                if matches!(arg.kind, ExprKind::Local(slot) if slot == row)
                    && pure_term(None, result, *receiver, None))
        }
        _ => false,
    }
}

/// Match an actual list traversal whose only observable body update is a floating sum.
fn run_sum<'a>(module: &Module, test: &Expr, body: &'a [Statement]) -> Option<RunSum<'a>> {
    let [
        Statement::Assign(row, item),
        Statement::Assign(cursor, advance),
        Statement::Assign(sum, update),
    ] = body
    else {
        return None;
    };
    let ExprKind::Builtin(Builtin::SequenceGetInBounds, args) = &item.kind else {
        return None;
    };
    let [receiver, index] = args.as_slice() else {
        return None;
    };
    let ExprKind::Local(list) = receiver.kind else {
        return None;
    };
    let local =
        |expr: &Expr, expected| matches!(expr.kind, ExprKind::Local(slot) if slot == expected);
    let ExprKind::Compare(parts, ops) = &test.kind else {
        return None;
    };
    let ([start, bound], [Compare::Less]) = (parts.as_slice(), ops.as_slice()) else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, old_index, one) = &advance.kind else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, old_sum, term) = &update.kind else {
        return None;
    };
    (module.types.is_value_struct(item.ty)
        && local(index, *cursor) && local(start, *cursor) && local(old_index, *cursor)
        && matches!(&bound.kind, ExprKind::Builtin(Builtin::Len, a) if matches!(a.as_slice(), [v] if local(v, list)))
        && matches!(&one.kind, ExprKind::Int(n) if n == "1")
        && local(old_sum, *sum) && old_sum.ty == Type::Float
        && pure_term(Some(module), term, *row, Some(*sum)))
    .then_some(RunSum { list, cursor: *cursor, row: *row, accumulator: *sum, term })
}

impl Emitter<'_, '_> {
    /// Advance over the current read-only run after copying its observable loop target.
    /// Dense storage and a receiver that differs from the snapshot take the original loop.
    pub(super) fn struct_run_loop(
        &mut self,
        test: &Expr,
        body: &[Statement],
        ordinary: &str,
        exhausted: &str,
    ) -> bool {
        let Some(plan) = run_sum(self.module, test, body) else {
            return false;
        };
        if !self.struct_locals[plan.row] || !self.module.source_frames.is_empty() {
            return false;
        }
        let Some(view) =
            self.length_roots[plan.list].and_then(|r| self.length_snapshots[r].clone())
        else {
            return false;
        };
        let Some(ListBuffer::Struct {
            data,
            index_mask,
            cursor,
        }) = view.data
        else {
            return false;
        };

        // Keep the original dense traversal intact. A packed repeated list
        // broadcasts its first row, while a null view uses the sparse cursor.
        let list = self.borrow_slot(plan.list);
        let same = self.instruction(format!(
            "icmp eq ptr {}, {}",
            list.operand, view.receiver.operand
        ));
        let sparse = self.instruction(format!("icmp eq ptr {data}, null"));
        let repeated = self.instruction(format!("icmp eq i64 {index_mask}, 0"));
        let compressed = self.instruction(format!("or i1 {sparse}, {repeated}"));
        let eligible = self.instruction(format!("and i1 {same}, {compressed}"));
        let (condition, visit) = (self.label(), self.label());
        self.branch(&eligible, &condition, ordinary);
        self.start(&condition);
        let more = self.while_condition(test, false);
        self.branch(&more, &visit, exhausted);
        self.start(&visit);

        self.block(&body[..2]);
        let next = self.borrow_slot(plan.cursor);
        let next = self.instruction(format!("lshr i64 {}, 1", next.operand));
        let at = self.instruction(format!("sub i64 {next}, 1"));
        let length = self.instruction(format!("lshr i64 {}, 1", view.length.operand));
        let sparse_end = self.instruction(format!(
            "call i64 @imre_struct_uniform_end(ptr {cursor}, i64 {next})"
        ));
        let end = self.instruction(format!(
            "select i1 {sparse}, i64 {sparse_end}, i64 {length}"
        ));
        let count = self.instruction(format!("sub i64 {end}, {at}"));

        let sum = self.borrow_slot(plan.accumulator);
        let term = self.expr(plan.term);
        let result = self.instruction(format!(
            "call double @imre_repeat_fadd(double {}, double {}, i64 {count})",
            sum.operand, term.operand
        ));
        self.line(format!(
            "store double {result}, ptr %local{}",
            plan.accumulator
        ));
        let shifted = self.instruction(format!("shl i64 {end}, 1"));
        let tagged = self.instruction(format!("or i64 {shifted}, 1"));
        self.line(format!("store i64 {tagged}, ptr %local{}", plan.cursor));
        self.jump(&condition);
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uniform_runs_require_a_pure_independent_float_term() {
        let source = r#"
from imre import struct
@struct
class Reading:
    x: float
    def scaled(self) -> float:
        return self.x * 0.5
    def noisy(self) -> float:
        print(self.x)
        return self.x
def eligible(rows: list[Reading], offset: float) -> float:
    total = 0.0
    for item in rows:
        total += item.scaled() - offset
    return total
def dependent(rows: list[Reading]) -> float:
    total = 1.0
    for item in rows:
        total += total * item.x
    return total
def effectful(rows: list[Reading]) -> float:
    total = 0.0
    for item in rows:
        total += item.noisy()
    return total
def fallible(rows: list[Reading]) -> float:
    total = 0.0
    for item in rows:
        total += 1.0 / item.x
    return total
rows = [Reading(2.0)] * 3
print(eligible(rows, 1.0))
print(dependent(rows))
print(effectful(rows))
print(fallible(rows))
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call double @imre_repeat_fadd(").count(), 1);
    }
}
