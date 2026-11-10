//! Branch plans and ownership transfers shared by fixed-layout values and Python frame entries.
use super::{CheckedBody, CheckedValue, Entry, Value, ValueJoin};
use crate::ir::control::{Context, Depth, Edges, Input, Target, edges};
use crate::ir::{Builtin, Expr, ExprKind, TruthPredicate, Type};

/// A logical plan owns the short-circuit edges and the selected value join.
struct Logical {
    edges: Edges,
    labels: Vec<String>,
    result: ResultEdges,
}

enum ResultEdges {
    Branch(String, String),
    Value {
        done: String,
        join: ValueJoin,
        result_type: Type,
    },
}

impl Logical {
    fn new(body: &mut super::FunctionBody, edges: Edges, result: ResultEdges) -> Self {
        let labels = (1..edges.0.len()).map(|_| body.name("logical")).collect();
        Self {
            edges,
            labels,
            result,
        }
    }

    fn branch(body: &mut super::FunctionBody, edges: Edges, yes: &str, no: &str) -> Self {
        Self::new(
            body,
            edges,
            ResultEdges::Branch(yes.to_owned(), no.to_owned()),
        )
    }

    fn value(body: &mut super::FunctionBody, edges: Edges, result_type: Type) -> Self {
        let done = body.name("logical");
        Self::new(
            body,
            edges,
            ResultEdges::Value {
                done,
                join: ValueJoin::default(),
                result_type,
            },
        )
    }

    /// Request each input on its own CFG edge. The last operand in value context
    /// is returned directly, without a truth callback.
    fn emit(mut self, code: &mut CheckedBody, inputs: &[Input<'_>]) -> Option<CheckedValue> {
        let context = match self.result {
            ResultEdges::Branch(..) => Context::Condition,
            ResultEdges::Value { .. } => Context::Value,
        };
        for (i, leaf) in inputs.iter().enumerate().rev() {
            let tested = context == Context::Value && !self.final_value(i);
            if let Some(block) = self.labels.get(i) {
                code.body().start(block);
            }
            let mark = if tested { code.root_mark() } else { None };
            self.accept(code, i, leaf, mark);
        }
        match self.result {
            ResultEdges::Branch(..) => None,
            ResultEdges::Value { done, join, .. } => {
                let body = code.body();
                body.start(&done);
                join.finish(body)
            }
        }
    }

    fn final_value(&self, i: usize) -> bool {
        let (yes, no) = self.edges.0[i];
        matches!(self.result, ResultEdges::Value { .. })
            && !matches!(yes, Target::Step(_))
            && !matches!(no, Target::Step(_))
    }

    fn accept(
        &mut self,
        code: &mut CheckedBody,
        i: usize,
        input: &Input<'_>,
        mark: Option<String>,
    ) {
        let (t, f) = self.edges.0[i];
        let label = |target, yes, no| match target {
            Target::Step(i) => self.labels[i].as_str(),
            Target::Yes => yes,
            Target::No => no,
        };
        let (done, join, result_type) = match &mut self.result {
            ResultEdges::Branch(yes, no) => {
                let bit = match input.predicate {
                    None => code.test_operand(input.value),
                    Some(predicate) => {
                        let (value, bit) = code.logical_operand(input.value, Some(predicate));
                        code.release_input(value);
                        bit
                    }
                };
                code.body()
                    .branch(&bit, label(t, yes, no), label(f, yes, no));
                return;
            }
            ResultEdges::Value {
                done,
                join,
                result_type,
            } => (done, join, result_type),
        };
        let yes = label(t, done, done);
        let no = label(f, done, done);
        if yes == done && no == done {
            let value = code.evaluate(input.value);
            code.transfer(&value);
            join.jump(code.body(), done, value);
            return;
        }
        let (value, truth) = code.logical_operand(input.value, input.predicate);
        if yes != done && no != done {
            code.release_input(value);
            code.end_roots(mark);
            code.body().branch(&truth, yes, no);
            return;
        }
        code.transfer(&value);
        let take = yes == done;
        let next = if take { no } else { yes };
        // A pure i1 can enter the join directly; there is no cleanup on its
        // continuation. Other values transfer ownership exclusively on selection.
        if let CheckedValue::Native(operand, None) = &value
            && operand.value.ty == Type::Bool
            && *result_type == Type::Bool
        {
            let owned = code.fixed().own_operand(value.into_native());
            join.incoming(code.body(), CheckedValue::owned(owned));
            code.body().branch(&truth, yes, no);
            return;
        }
        let discard = code.body().name("logical");
        let keep = code.body().name("logical");
        if take {
            code.body().branch(&truth, &keep, &discard);
        } else {
            code.body().branch(&truth, &discard, &keep);
        }
        code.body().start(&discard);
        match &value {
            CheckedValue::Native(operand, None) => code.fixed().release_operand(operand),
            _ => {
                if let Some(slot) = value.frame_owner() {
                    code.python().release_word(slot);
                }
            }
        }
        code.end_roots(mark);
        code.body().jump(next);
        code.body().start(&keep);
        let value = match (&mut code.0, value) {
            (Entry::Native(emitter), CheckedValue::Native(operand, None)) => {
                let input = emitter.own_operand(operand);
                let owned = emitter.selected_operand(input, *result_type, take);
                emitter.forget(&owned);
                CheckedValue::owned(owned)
            }
            (_, value) => value,
        };
        join.jump(code.body(), done, value);
    }
}

impl CheckedBody<'_, '_, '_, '_> {
    pub(in crate::llvm) fn test_operand(&mut self, expr: &Expr) -> String {
        self.test_operand_in_scope(expr, false)
    }

    pub(in crate::llvm) fn test_operand_in_scope(
        &mut self,
        expr: &Expr,
        already_rooted: bool,
    ) -> String {
        if let ExprKind::Bool(value) = expr.kind {
            return value.to_string();
        }
        if let ExprKind::Builtin(Builtin::RegisteredLocation(range, position), operands) =
            &expr.kind
            && let ExprKind::Builtin(Builtin::RegisteredCompare(ops), values) = &operands[0].kind
        {
            let previous = self.python().enter_test(*range, *position);
            let bit = self
                .registered_comparisons(values, ops, true)
                .into_condition();
            self.python().leave_test(previous);
            return bit;
        }
        if let ExprKind::Builtin(Builtin::RegisteredCompare(ops), values) = &expr.kind {
            return self
                .registered_comparisons(values, ops, true)
                .into_condition();
        }
        let mark = if already_rooted {
            None
        } else {
            self.root_mark()
        };
        let value = self.operand(expr);
        let bit = self.operand_truth(expr, &value, None);
        self.release_input(value);
        self.end_roots(mark);
        bit
    }

    fn operand_truth(
        &mut self,
        expr: &Expr,
        value: &CheckedValue,
        at: Option<ruff_text_size::TextRange>,
    ) -> String {
        if let Entry::Registered(frame) = &mut self.0
            && let ExprKind::Builtin(Builtin::RegisteredLocation(range, anchor), args) = &expr.kind
        {
            let previous = frame.enter_test(*range, *anchor);
            let bit = self.operand_truth(&args[0], value, Some(*range));
            self.python().leave_test(previous);
            return bit;
        }
        match (&mut self.0, value) {
            (Entry::Native(emitter), CheckedValue::Native(input, None)) => {
                if input.value.ty == Type::Bool {
                    input.value.operand.clone()
                } else {
                    emitter.truth(&input.value).operand
                }
            }
            (Entry::Registered(frame), CheckedValue::Handle { id, exact_word }) => {
                if let Some(word) = exact_word {
                    return frame.body().instruction(format!("icmp ne i64 {word}, 1"));
                }
                if expr.ty == Type::Int {
                    let word = frame.exact_word(id);
                    return self.word_or_truth(&word, id, at);
                }
                frame.truth_at(id, at)
            }
            (Entry::Registered(_), CheckedValue::IntOrHandle { word, fallback, .. }) => {
                self.word_or_truth(word, fallback, at)
            }
            (Entry::Registered(_), CheckedValue::Native(input, Some(_))) => self
                .body()
                .instruction(format!("icmp ne i64 {}, 1", input.value.operand)),
            _ => unreachable!("operand representation belongs to its checked entry"),
        }
    }
    fn root_mark(&mut self) -> Option<String> {
        match &mut self.0 {
            Entry::Native(_) => None,
            Entry::Registered(frame) => Some(frame.temporary_mark()),
        }
    }

    fn end_roots(&mut self, mark: Option<String>) {
        if let Some(mark) = mark {
            self.python().discard_truth(&mark);
        }
    }

    fn transfer(&mut self, value: &CheckedValue) {
        if let (Entry::Native(emitter), CheckedValue::Native(value, None)) = (&mut self.0, value) {
            emitter.forget(&value.value);
        }
    }

    fn bit(value: String) -> CheckedValue {
        CheckedValue::owned(Value {
            ty: Type::Bool,
            operand: value,
            small_integer: false,
        })
    }

    /// The slow edge can invoke user code. Keeping integer ownership in its existing
    /// slot allows the selected operand to escape without materializing it for truth.
    fn word_or_truth(
        &mut self,
        word: &str,
        fallback: &str,
        at: Option<ruff_text_size::TextRange>,
    ) -> String {
        let exact = self.body().instruction(format!("icmp ne i64 {word}, 0"));
        self.select_value(
            |code, yes, no| code.body().branch(&exact, yes, no),
            |code| Self::bit(code.body().instruction(format!("icmp ne i64 {word}, 1"))),
            |code| Self::bit(code.python().truth_at(fallback, at)),
        )
        .into_native()
        .value
        .operand
    }

    fn select_value(
        &mut self,
        branch: impl FnOnce(&mut Self, &str, &str),
        yes: impl FnOnce(&mut Self) -> CheckedValue,
        no: impl FnOnce(&mut Self) -> CheckedValue,
    ) -> CheckedValue {
        ValueJoin::select(
            self,
            Self::body,
            branch,
            |code| {
                let value = yes(code);
                code.transfer(&value);
                value
            },
            |code| {
                let value = no(code);
                code.transfer(&value);
                value
            },
        )
        .expect("expression has a normal result")
    }

    pub(crate) fn branch_condition(&mut self, expr: &Expr, yes: &str, no: &str) {
        let (edges, inputs) = edges(expr, Context::Condition, Depth::All);
        Logical::branch(self.body(), edges, yes, no).emit(self, &inputs);
    }

    pub(super) fn condition(&mut self, expr: &Expr, yes: &str, no: &str) {
        if let Entry::Native(code) = &mut self.0 {
            let bit = code.expr(expr);
            code.branch(&bit.operand, yes, no);
        } else {
            self.branch_condition(expr, yes, no);
        }
    }

    pub(crate) fn predicate(&mut self, expr: &Expr, negate: bool) -> CheckedValue {
        let bit = self.test_operand(expr);
        match &mut self.0 {
            Entry::Native(_) => {
                let result = if negate {
                    self.body().instruction(format!("xor i1 {bit}, true"))
                } else {
                    bit
                };
                Self::bit(result)
            }
            Entry::Registered(frame) => frame.boolean_result(&bit, negate),
        }
    }

    fn logical_operand(
        &mut self,
        expr: &Expr,
        predicate: Option<&TruthPredicate>,
    ) -> (CheckedValue, String) {
        let value = self.operand(expr);
        let Some(predicate) = predicate else {
            let truth = self.operand_truth(expr, &value, None);
            return (value, truth);
        };
        let emitter = self.fixed();
        let slot = predicate.receiver;
        let receiver = emitter.own_operand(value.into_native());
        debug_assert_eq!(receiver.ty, emitter.locals[slot]);
        emitter
            .cleanups
            .push(crate::llvm::Cleanup::Slots(vec![slot]));
        emitter.store(slot, &receiver);
        let truth = emitter.expr(&predicate.condition);
        emitter.cleanups.pop();
        (CheckedValue::owned(emitter.take_slot(slot)), truth.operand)
    }

    fn logical_value(&mut self, expr: &Expr) -> CheckedValue {
        let depth = if expr.ty == Type::Bool && matches!(&expr.kind, ExprKind::Boolean(..)) {
            Depth::Root
        } else {
            Depth::All
        };
        let (edges, inputs) = edges(expr, Context::Value, depth);
        Logical::value(self.body(), edges, expr.ty)
            .emit(self, &inputs)
            .expect("a selected operand reaches the join")
    }

    pub(super) fn conditional_value(&mut self, test: &Expr, yes: &Expr, no: &Expr) -> CheckedValue {
        if let Entry::Native(emitter) = &mut self.0
            && let Some(value) = emitter.absolute_choice(test, yes, no)
        {
            return CheckedValue::owned(value);
        }
        self.select_value(
            |code, t, f| code.condition(test, t, f),
            |code| code.evaluate(yes),
            |code| code.evaluate(no),
        )
    }

    pub(super) fn boolean_expression(&mut self, expr: &Expr) -> CheckedValue {
        if let (Entry::Native(emitter), ExprKind::BooleanValue(and, left, right, None)) =
            (&mut self.0, &expr.kind)
            && let Some(truth) = emitter.module.types.truthiness(left.ty)
        {
            let value = emitter.operand(left);
            if truth == *and {
                emitter.release_operand(&value);
                return CheckedValue::owned(emitter.expr(right));
            }
            let owned = emitter.own_operand(value);
            return CheckedValue::owned(emitter.selected_operand(owned, expr.ty, truth));
        }
        if let (Entry::Native(emitter), ExprKind::Boolean(and, values)) = (&mut self.0, &expr.kind)
            && let Some(value) = emitter.boolean_specialization(*and, values)
        {
            return CheckedValue::owned(value);
        }
        self.logical_value(expr)
    }
}
