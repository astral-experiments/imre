//! Fold uniform runs and repeated cycles of pure scalar-row projections when binary64 addition is exact.
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
    /// A dense or replaced receiver follows the ordinary traversal.
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
        let other = self.label();
        let shared = self.label();
        self.branch(&eligible, &shared, &other);
        self.start(&shared);
        let at = self.borrow_slot(plan.cursor);
        let first = self.instruction(format!("icmp eq i64 {}, 1", at.operand));
        let query = self.instruction(format!("and i1 {sparse}, {first}"));
        let binary = self.label();
        self.branch(&query, &binary, &condition);
        self.start(&binary);
        self.binary_struct_sum(
            &plan,
            &list.operand,
            &view.length.operand,
            &condition,
            ordinary,
        );
        self.start(&other);
        // Uniform and sparse views have already dispatched above. Only a
        // matching periodic view can reuse a complete cycle of projections.
        let masked = self.instruction(format!("icmp ne i64 {index_mask}, -1"));
        let cyclic = self.instruction(format!("and i1 {same}, {masked}"));
        let cycle = self.label();
        self.branch(&cyclic, &cycle, ordinary);
        self.start(&cycle);
        self.periodic_struct_sum(&plan, body, &view.length.operand, &index_mask, ordinary);
        self.start(&condition);
        self.check_interrupt();
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
        // The exact bulk path is constant time. Bound only the scalar fallback
        // so a long uniform run returns regularly to a Python loop checkpoint.
        let exact = self.instruction(format!(
            "call i1 @imre_exact_fadd_bound(double {}, double {}, i64 {count})",
            sum.operand, term.operand
        ));
        let short = self.instruction(format!("icmp ule i64 {count}, 65536"));
        let whole = self.instruction(format!("or i1 {exact}, {short}"));
        let count = self.instruction(format!("select i1 {whole}, i64 {count}, i64 65536"));
        let end = self.instruction(format!("add i64 {at}, {count}"));
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

    /// Evaluate one pure projection per dictionary entry. Keep the ordinary last
    /// iteration so the cursor and observable row target have their original values.
    fn binary_struct_sum(
        &mut self,
        plan: &RunSum<'_>,
        list: &str,
        length: &str,
        ordered: &str,
        ordinary: &str,
    ) {
        self.check_interrupt();
        let rows = self.entry_alloca("{ [2 x ptr], ptr, i64 }");
        let ones = self.instruction(format!(
            "call i64 @imre_struct_binary_prefix(ptr {list}, ptr {rows})"
        ));
        let found = self.instruction(format!("icmp sge i64 {ones}, 0"));
        let check = self.label();
        self.branch(&found, &check, ordered);
        self.start(&check);
        let count = self.instruction(format!("lshr i64 {length}, 1"));
        let prefix = self.instruction(format!("sub i64 {count}, 1"));
        let zeros = self.instruction(format!("sub i64 {prefix}, {ones}"));
        let initial = self.borrow_slot(plan.accumulator);
        let terms = [0, 1].map(|index| {
            let slot = self.instruction(format!("getelementptr ptr, ptr {rows}, i64 {index}"));
            let row = self.instruction(format!("load ptr, ptr {slot}"));
            self.line(format!("store ptr {row}, ptr %local{}", plan.row));
            self.expr(plan.term).operand
        });
        let mut sum = initial.operand.clone();
        let mut exact = String::from("true");
        for (term, frequency) in terms.iter().zip([zeros, ones]) {
            let fits = self.instruction(format!(
                "call i1 @imre_exact_fadd_bound(double {}, double {}, i64 {prefix})",
                initial.operand, term
            ));
            exact = self.instruction(format!("and i1 {exact}, {fits}"));
            let n = self.instruction(format!("uitofp i64 {frequency} to double"));
            let product = self.instruction(format!("fmul double {term}, {n}"));
            sum = self.instruction(format!("fadd double {sum}, {product}"));
        }
        let (fast, slow, advance) = (self.label(), self.label(), self.label());
        self.branch(&exact, &fast, &slow);
        self.start(&fast);
        self.jump(&advance);
        self.start(&slow);
        // Ordered additions can run for arbitrarily many rows. Publish the
        // loop state at the same checkpoint interval as a uniform scalar run.
        let cursor = self.borrow_slot(plan.cursor);
        let at = self.instruction(format!("lshr i64 {}, 1", cursor.operand));
        let remaining = self.instruction(format!("sub i64 {prefix}, {at}"));
        let short = self.instruction(format!("icmp ule i64 {remaining}, 65536"));
        let count = self.instruction(format!("select i1 {short}, i64 {remaining}, i64 65536"));
        let end = self.instruction(format!("add i64 {at}, {count}"));
        let subtotal = self.borrow_slot(plan.accumulator);
        let ordered_sum = self.instruction(format!(
            "call double @imre_struct_binary_ordered(ptr {rows}, i64 {at}, i64 {end}, double {}, double {}, double {})",
            subtotal.operand, terms[0], terms[1]
        ));
        self.jump(&advance);
        self.start(&advance);
        let end = self.instruction(format!("phi i64 [{prefix}, %{fast}], [{end}, %{slow}]"));
        let result = self.instruction(format!(
            "phi double [{sum}, %{fast}], [{ordered_sum}, %{slow}]"
        ));
        self.line(format!(
            "store double {result}, ptr %local{}",
            plan.accumulator
        ));
        let last = self.instruction(format!("sub i64 {end}, 1"));
        let row = self.instruction(format!("call ptr @imre_struct_row(ptr {list}, i64 {last})"));
        self.line(format!("store ptr {row}, ptr %local{}", plan.row));
        let shifted = self.instruction(format!("shl i64 {end}, 1"));
        let tagged = self.instruction(format!("or i64 {shifted}, 1"));
        self.line(format!("store i64 {tagged}, ptr %local{}", plan.cursor));
        let complete = self.instruction(format!("icmp eq i64 {end}, {prefix}"));
        let resume = self.label();
        self.branch(&complete, ordinary, &resume);
        self.start(&resume);
        self.check_interrupt();
        self.jump(&slow);
    }

    /// Execute the first cycle in source order. Each projected term must satisfy
    /// the same exact-addition bound for the entire remaining traversal. Failure
    /// resumes at the untouched suffix with the accumulator and row already correct.
    fn periodic_struct_sum(
        &mut self,
        plan: &RunSum<'_>,
        body: &[Statement],
        tagged_length: &str,
        mask: &str,
        ordinary: &str,
    ) {
        let cursor = self.borrow_slot(plan.cursor);
        let tagged = self.instruction(format!("trunc i64 {} to i1", cursor.operand));
        let at = self.instruction(format!("lshr i64 {}, 1", cursor.operand));
        let length = self.instruction(format!("lshr i64 {tagged_length}, 1"));
        let inside = self.instruction(format!("icmp ule i64 {at}, {length}"));
        let valid = self.instruction(format!("and i1 {tagged}, {inside}"));
        let remaining = self.instruction(format!("sub i64 {length}, {at}"));
        let period = self.instruction(format!("add i64 {mask}, 1"));
        let half = self.instruction(format!("lshr i64 {remaining}, 1"));
        let multiple = self.instruction(format!("icmp uge i64 {half}, {period}"));
        let enough = self.instruction(format!("and i1 {valid}, {multiple}"));
        let (start, visit, checked, skip) =
            (self.label(), self.label(), self.label(), self.label());
        self.branch(&enough, &start, ordinary);
        self.start(&start);
        let initial = self.borrow_slot(plan.accumulator);
        let end = self.instruction(format!("add i64 {at}, {period}"));
        let exact = self.entry_alloca("i1");
        self.line(format!("store i1 true, ptr {exact}"));
        self.jump(&visit);
        self.start(&visit);
        self.check_interrupt();
        self.block(&body[..2]);
        let sum = self.borrow_slot(plan.accumulator);
        let term = self.expr(plan.term);
        let added = self.instruction(format!("fadd double {}, {}", sum.operand, term.operand));
        self.line(format!(
            "store double {added}, ptr %local{}",
            plan.accumulator
        ));
        let fits = self.instruction(format!(
            "call i1 @imre_exact_fadd_bound(double {}, double {}, i64 {remaining})",
            initial.operand, term.operand
        ));
        let previous = self.instruction(format!("load i1, ptr {exact}"));
        let all = self.instruction(format!("and i1 {previous}, {fits}"));
        self.line(format!("store i1 {all}, ptr {exact}"));
        let next = self.borrow_slot(plan.cursor);
        let next = self.instruction(format!("lshr i64 {}, 1", next.operand));
        let complete = self.instruction(format!("icmp eq i64 {next}, {end}"));
        self.branch(&complete, &checked, &visit);
        self.start(&checked);
        self.branch(&all, &skip, ordinary);
        self.start(&skip);
        let rest = self.instruction(format!("sub i64 {length}, {end}"));
        let cycles = self.instruction(format!("udiv i64 {rest}, {period}"));
        let count = self.instruction(format!("uitofp i64 {cycles} to double"));
        let delta = self.instruction(format!("fsub double {added}, {}", initial.operand));
        let product = self.instruction(format!("fmul double {delta}, {count}"));
        let bulk = self.instruction(format!("fadd double {added}, {product}"));
        // A zero-delta cycle has reached a fixed point, including the sign of zero.
        let fixed = self.instruction(format!("fcmp oeq double {delta}, 0.0"));
        let result = self.instruction(format!("select i1 {fixed}, double {added}, double {bulk}"));
        self.line(format!(
            "store double {result}, ptr %local{}",
            plan.accumulator
        ));
        let tail = self.instruction(format!("and i64 {rest}, {mask}"));
        let resume = self.instruction(format!("sub i64 {length}, {tail}"));
        let shifted = self.instruction(format!("shl i64 {resume}, 1"));
        let tagged = self.instruction(format!("or i64 {shifted}, 1"));
        self.line(format!("store i64 {tagged}, ptr %local{}", plan.cursor));
        self.jump(ordinary);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn run_folds_require_a_pure_independent_float_term() {
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
        // Only the independent projection can query a two-value dictionary.
        assert_eq!(
            llvm.matches("call i64 @imre_struct_binary_prefix(").count(),
            1
        );
        assert_eq!(llvm.matches("call i1 @imre_exact_fadd_bound(").count(), 5);
    }
}
