//! Branch plans and ownership transfers shared by fixed-layout values and Python frame entries.
use super::{CheckedBody, CheckedValue, Entry, Value, ValueJoin, ValueUse};
use crate::ir::control::{Context, Depth, Step, Steps, Target, needs_truth_plan, steps};
use crate::ir::{Builtin, Expr, ExprKind, TruthPredicate, Type};

/// A logical plan owns the short-circuit edges and the selected value join.
struct Logical {
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
    fn branch(code: &mut CheckedBody, steps: Steps<'_>, yes: &str, no: &str) {
        Self::emit(
            code,
            steps,
            ResultEdges::Branch(yes.to_owned(), no.to_owned()),
        );
    }

    fn value(code: &mut CheckedBody, steps: Steps<'_>, result_type: Type) -> CheckedValue {
        let done = code.body().name("logical");
        Self::emit(
            code,
            steps,
            ResultEdges::Value {
                done,
                join: ValueJoin::default(),
                result_type,
            },
        )
        .expect("a selected operand reaches the join")
    }

    /// Request each input on its own CFG edge. The last operand in value context
    /// is returned directly, without a truth callback.
    fn emit(code: &mut CheckedBody, steps: Steps<'_>, result: ResultEdges) -> Option<CheckedValue> {
        let body = code.body();
        let labels = (1..steps.0.len()).map(|_| body.name("logical")).collect();
        let context = match result {
            ResultEdges::Branch(..) => Context::Condition,
            ResultEdges::Value { .. } => Context::Value,
        };
        let mut logical = Self { labels, result };
        for (i, step) in steps.0.iter().enumerate().rev() {
            let tested = context == Context::Value
                && (matches!(step.yes, Target::Step(_)) || matches!(step.no, Target::Step(_)));
            if let Some(block) = logical.labels.get(i) {
                code.body().start(block);
            }
            let mark = if tested { code.root_mark() } else { None };
            logical.accept(code, step, mark);
        }
        match logical.result {
            ResultEdges::Branch(..) => None,
            ResultEdges::Value { done, join, .. } => {
                let body = code.body();
                body.start(&done);
                join.finish(body)
            }
        }
    }

    fn accept(&mut self, code: &mut CheckedBody, input: &Step<'_>, mark: Option<String>) {
        let (t, f) = (input.yes, input.no);
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
        if let Entry::Registered(frame) = &mut self.0
            && let ExprKind::Builtin(Builtin::Bool, args) = &expr.kind
        {
            frame.next_fixed_site(crate::ir::Node::Expression(expr));
            return self.test_operand_in_scope(&args[0], already_rooted);
        }
        let at = match &self.0 {
            Entry::Registered(frame) => frame.operand_range(expr),
            Entry::Native(_) => None,
        };
        if let ExprKind::Bool(value) = expr.kind {
            if let Entry::Registered(frame) = &self.0 {
                frame.next_fixed_site(crate::ir::Node::Expression(expr));
            }
            return value.to_string();
        }
        if matches!(self.0, Entry::Registered(_))
            && matches!(expr.without_location().kind, ExprKind::Compare(..))
        {
            return self
                .value_for(expr, ValueUse::ComparisonTruth)
                .into_condition();
        }
        let mark = if already_rooted {
            None
        } else {
            self.root_mark()
        };
        let value = self.operand(expr);
        let bit = self.operand_truth(expr, &value, at);
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
            && let ExprKind::Builtin(Builtin::RegisteredLocation(range, anchor, ..), args) =
                &expr.kind
        {
            return frame.at_expression(*range, *anchor, |frame| {
                CheckedBody::registered(frame.reborrow()).operand_truth(
                    &args[0],
                    value,
                    Some(*range),
                )
            });
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
        if let Entry::Registered(frame) = &mut self.0
            && let Some(bit) = frame.bound_condition(expr)
        {
            self.body().branch(&bit, yes, no);
            return;
        }
        if let Some(value) = self.native_boolean(expr) {
            let bit = self.operand_truth(expr, &value, None);
            self.release_input(value);
            self.body().branch(&bit, yes, no);
            return;
        }
        if !needs_truth_plan(expr) {
            let bit = self.test_operand(expr);
            self.body().branch(&bit, yes, no);
            return;
        }
        let depth = match &self.0 {
            Entry::Native(_) => Self::logical_depth(expr),
            Entry::Registered(_) => Depth::All,
        };
        Logical::branch(self, steps(expr, Context::Condition, depth), yes, no);
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

    fn logical_depth(expr: &Expr) -> Depth {
        if expr.ty == Type::Bool && matches!(&expr.kind, ExprKind::Boolean(..)) {
            Depth::Root
        } else {
            Depth::All
        }
    }

    fn logical_value(&mut self, expr: &Expr) -> CheckedValue {
        Logical::value(
            self,
            steps(expr, Context::Value, Self::logical_depth(expr)),
            expr.ty,
        )
    }

    pub(super) fn conditional_value(&mut self, test: &Expr, yes: &Expr, no: &Expr) -> CheckedValue {
        if let Entry::Native(emitter) = &mut self.0
            && let Some(value) = emitter.absolute_choice(test, yes, no)
        {
            return CheckedValue::owned(value);
        }
        self.select_value(
            |code, t, f| code.branch_condition(test, t, f),
            |code| code.evaluate(yes),
            |code| code.evaluate(no),
        )
    }

    fn native_boolean(&mut self, expr: &Expr) -> Option<CheckedValue> {
        let Entry::Native(emitter) = &mut self.0 else {
            return None;
        };
        match &expr.kind {
            ExprKind::BooleanValue(and, left, right, None) => {
                let truth = emitter.module.types.truthiness(left.ty)?;
                let value = emitter.operand(left);
                if truth == *and {
                    emitter.release_operand(&value);
                    return Some(CheckedValue::owned(emitter.expr(right)));
                }
                let owned = emitter.own_operand(value);
                Some(CheckedValue::owned(
                    emitter.selected_operand(owned, expr.ty, truth),
                ))
            }
            ExprKind::Boolean(and, values) => emitter
                .boolean_specialization(*and, values)
                .map(CheckedValue::owned),
            _ => None,
        }
    }

    pub(super) fn boolean_expression(&mut self, expr: &Expr) -> CheckedValue {
        self.native_boolean(expr)
            .unwrap_or_else(|| self.logical_value(expr))
    }
}
