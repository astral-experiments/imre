//! Conservative integer intervals over the checked, immutable IR.
//!
//! Facts describe values at their evaluation points, not mutable heap locations.
//! Calls cannot rebind a caller's integer locals, but every field, list item, and
//! user-function result starts unknown. Container lengths have a storage-size
//! bound, but no relationship to a container's later length is retained. Indexed
//! accesses must keep checking current bounds, including after alias mutation.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::ir::{Binary, Builtin, Compare, Expr, ExprKind, Function, Module, Statement, Type};

const MIN_SMALL: i128 = -(1_i128 << 62);
const MAX_SMALL: i128 = (1_i128 << 62) - 1;
// The 64-bit runtime asserts that list slots and mapping entries occupy at least
// eight bytes. Rust allocations cannot exceed isize::MAX bytes, including their
// Vec buffers.
const MAX_LIST_LEN: i128 = (1_i128 << 60) - 1;
const WIDENING_POINTS: [i128; 7] = [MIN_SMALL, -MAX_LIST_LEN, -1, 0, 1, MAX_LIST_LEN, MAX_SMALL];

/// A value guaranteed to use the runtime's canonical tagged-small representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IntRange {
    pub min: i64,
    pub max: i64,
}

/// Expression identities remain valid while this immutable module borrow lives.
/// Missing facts, including unreachable or arbitrary-precision expressions, must
/// use the ordinary checked lowering path.
pub(crate) struct RangeFacts<'ir> {
    intervals: HashMap<*const Expr, Interval>,
    cursors: HashMap<*const Statement, Interval>,
    module: PhantomData<&'ir Module>,
}

impl RangeFacts<'_> {
    pub(crate) fn integer(&self, expression: &Expr) -> Option<IntRange> {
        self.intervals.get(&std::ptr::from_ref(expression))?.small()
    }

    /// Covers the independent range cursor's initial value and final increment,
    /// including zero iterations. A proof also guarantees that all captured
    /// start/stop/step arguments are small and the step has a nonzero fixed sign.
    pub(crate) fn range_cursor(&self, statement: &Statement) -> Option<IntRange> {
        self.cursors.get(&std::ptr::from_ref(statement))?.small()
    }

    /// Finds slots whose initial value and every possible replacement are small.
    /// Parameters start unknown; other integer slots start at the emitter's zero.
    /// Inspecting even unreachable writes makes missing expression facts reject
    /// eligibility. Move edges propagate unknown ownership through temporary
    /// chains, while Clear and a Move's source reset only introduce zero.
    pub(crate) fn small_locals(&self, function: &Function) -> Vec<bool> {
        let mut small = function
            .locals
            .iter()
            .map(|ty| *ty == Type::Int)
            .collect::<Vec<_>>();
        for &parameter in &function.params {
            small[parameter] = false;
        }
        let mut moves = Vec::new();
        inspect_writes(self, &function.body, &mut small, &mut moves);
        loop {
            let mut changed = false;
            for &(source, target) in &moves {
                if !small[source] && small[target] {
                    small[target] = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        small
    }
}

/// A function-wide storage invariant needs every writer, independently of the
/// analysis control-flow path. Hidden range slots include the final increment;
/// proving only the yielded target would miss a possible BigInt cursor owner.
fn inspect_writes(
    facts: &RangeFacts<'_>,
    body: &[Statement],
    small: &mut [bool],
    moves: &mut Vec<(usize, usize)>,
) {
    for statement in body {
        match statement {
            Statement::Assign(slot, value) => {
                if facts.integer(value).is_none() {
                    small[*slot] = false;
                }
            }
            Statement::Move(target, source) => moves.push((*source, *target)),
            Statement::IterNext {
                key, value, ready, ..
            } => {
                // Mapping storage can supply an arbitrary-precision integer.
                for slot in [Some(*key), *value, Some(*ready)].into_iter().flatten() {
                    small[slot] = false;
                }
            }
            Statement::If(_, yes, no) => {
                inspect_writes(facts, yes, small, moves);
                inspect_writes(facts, no, small, moves);
            }
            Statement::While(_, body) => inspect_writes(facts, body, small, moves),
            Statement::Scope { body, .. } => inspect_writes(facts, body, small, moves),
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody] {
                    inspect_writes(facts, body, small, moves);
                }
                for handler in handlers {
                    inspect_writes(facts, &handler.body, small, moves);
                }
            }
            Statement::For {
                target,
                cursor,
                stop,
                step,
                body,
                ..
            } => {
                if facts.range_cursor(statement).is_none() {
                    for slot in [target, cursor, stop, step] {
                        small[*slot] = false;
                    }
                }
                inspect_writes(facts, body, small, moves);
            }
            Statement::Clear(_)
            | Statement::SetItem(_, _, _)
            | Statement::AugmentItem(_, _, _, _)
            | Statement::SetField(_, _, _)
            | Statement::AugmentField(_, _, _, _)
            | Statement::Expr(_)
            | Statement::Return(_)
            | Statement::Raise(_)
            | Statement::Break
            | Statement::Continue
            | Statement::Assert(_) => {}
        }
    }
}

/// Computes facts after ownership-preserving IR rewrites have finished. Loop
/// bodies are recorded only after their entry invariant reaches a fixed point.
pub(crate) fn analyze(module: &Module) -> RangeFacts<'_> {
    let mut analyzer = Analyzer {
        intervals: HashMap::new(),
        cursors: HashMap::new(),
    };
    for function in module.functions.iter().chain([&module.entry]) {
        analyzer.function(function);
    }
    RangeFacts {
        intervals: analyzer.intervals,
        cursors: analyzer.cursors,
        module: PhantomData,
    }
}

/// An absent endpoint denotes infinity. Checked i128 arithmetic discards bounds
/// on analysis overflow; it never truncates Python's arbitrary-precision values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Interval {
    min: Option<i128>,
    max: Option<i128>,
}

impl Interval {
    const UNKNOWN: Self = Self {
        min: None,
        max: None,
    };

    fn exact(value: i128) -> Self {
        Self {
            min: Some(value),
            max: Some(value),
        }
    }

    fn bounded(min: i128, max: i128) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
        }
    }

    fn singleton(self) -> Option<i128> {
        (self.min == self.max).then_some(self.min).flatten()
    }

    fn small(self) -> Option<IntRange> {
        let (min, max) = (self.min?, self.max?);
        (min >= MIN_SMALL && max <= MAX_SMALL).then_some(IntRange {
            min: min as i64,
            max: max as i64,
        })
    }

    fn join(self, other: Self) -> Self {
        Self {
            min: self.min.zip(other.min).map(|(a, b)| a.min(b)),
            max: self.max.zip(other.max).map(|(a, b)| a.max(b)),
        }
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let result = Self {
            min: match (self.min, other.min) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
            max: match (self.max, other.max) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        };
        if result
            .min
            .zip(result.max)
            .is_some_and(|(min, max)| min > max)
        {
            None
        } else {
            Some(result)
        }
    }

    /// Widen only expanding bounds, using a finite set of useful ABI thresholds.
    /// For example, a nonnegative heap cursor widens to MAX_LIST_LEN before its
    /// `2 * cursor + 1 < count` guard narrows the body again.
    fn widen(self, other: Self) -> Self {
        let joined = self.join(other);
        Self {
            min: if joined.min == self.min {
                self.min
            } else {
                joined.min.and_then(|min| {
                    WIDENING_POINTS
                        .into_iter()
                        .rev()
                        .find(|&point| point <= min)
                })
            },
            max: if joined.max == self.max {
                self.max
            } else {
                joined
                    .max
                    .and_then(|max| WIDENING_POINTS.into_iter().find(|&point| point >= max))
            },
        }
    }

    fn negate(self) -> Self {
        Self {
            min: self.max.and_then(i128::checked_neg),
            max: self.min.and_then(i128::checked_neg),
        }
    }

    fn binary(self, op: Binary, other: Self) -> Self {
        match op {
            Binary::Add => Self {
                min: self.min.zip(other.min).and_then(|(a, b)| a.checked_add(b)),
                max: self.max.zip(other.max).and_then(|(a, b)| a.checked_add(b)),
            },
            Binary::Subtract => self.binary(Binary::Add, other.negate()),
            Binary::Multiply => {
                if self.singleton() == Some(0) || other.singleton() == Some(0) {
                    return Self::exact(0);
                }
                let (Some(a), Some(b), Some(c), Some(d)) =
                    (self.min, self.max, other.min, other.max)
                else {
                    return Self::UNKNOWN;
                };
                let Some(products) = [
                    a.checked_mul(c),
                    a.checked_mul(d),
                    b.checked_mul(c),
                    b.checked_mul(d),
                ]
                .into_iter()
                .collect::<Option<Vec<_>>>() else {
                    return Self::UNKNOWN;
                };
                Self::bounded(
                    *products.iter().min().unwrap(),
                    *products.iter().max().unwrap(),
                )
            }
            Binary::FloorDivide => {
                let Some(divisor) = other.singleton().filter(|value| *value != 0) else {
                    return Self::UNKNOWN;
                };
                let (min, max) = if divisor > 0 {
                    (self.min, self.max)
                } else {
                    (self.max, self.min)
                };
                Self {
                    min: min.and_then(|value| divide(value, divisor, false)),
                    max: max.and_then(|value| divide(value, divisor, false)),
                }
            }
            Binary::Modulo => match other.singleton() {
                Some(divisor) if divisor > 0 => Self::bounded(0, divisor - 1),
                Some(divisor) if divisor < 0 => Self::bounded(divisor + 1, 0),
                _ => Self::UNKNOWN,
            },
            Binary::Repeat | Binary::Concat => Self::UNKNOWN,
        }
    }
}

/// Signed division with Python's floor rounding, or ceiling for lower-bound
/// inversion. Zero and i128 overflow lose precision instead of affecting code.
fn divide(numerator: i128, denominator: i128, ceiling: bool) -> Option<i128> {
    let quotient = numerator.checked_div(denominator)?;
    let remainder = numerator.checked_rem(denominator)?;
    if remainder == 0 {
        Some(quotient)
    } else if (remainder > 0) == (denominator > 0) {
        quotient.checked_add(i128::from(ceiling))
    } else {
        quotient.checked_sub(i128::from(!ceiling))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct State(Vec<Interval>);

impl State {
    fn unknown(count: usize) -> Self {
        Self(vec![Interval::UNKNOWN; count])
    }

    fn join(&self, other: &Self) -> Self {
        Self(
            self.0
                .iter()
                .zip(&other.0)
                .map(|(&a, &b)| a.join(b))
                .collect(),
        )
    }

    fn widen(&self, other: &Self) -> Self {
        Self(
            self.0
                .iter()
                .zip(&other.0)
                .map(|(&a, &b)| a.widen(b))
                .collect(),
        )
    }
}

fn merge(left: Option<State>, right: Option<State>) -> Option<State> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.join(&right)),
        (left, right) => left.or(right),
    }
}

/// Break and continue states belong to the nearest enclosing loop; returning
/// paths contribute no state to later statements or loop backedges.
#[derive(Default)]
struct Flow {
    next: Option<State>,
    breaks: Option<State>,
    continues: Option<State>,
}

impl Flow {
    fn next(state: State) -> Self {
        Self {
            next: Some(state),
            ..Self::default()
        }
    }

    fn join(self, other: Self) -> Self {
        Self {
            next: merge(self.next, other.next),
            breaks: merge(self.breaks, other.breaks),
            continues: merge(self.continues, other.continues),
        }
    }
}

struct Analyzer {
    intervals: HashMap<*const Expr, Interval>,
    cursors: HashMap<*const Statement, Interval>,
}

impl Analyzer {
    fn function(&mut self, function: &Function) {
        self.block(&function.body, State::unknown(function.locals.len()), true);
    }

    fn block(&mut self, statements: &[Statement], state: State, record: bool) -> Flow {
        let mut flow = Flow::next(state);
        for statement in statements {
            let Some(state) = flow.next.take() else { break };
            let current = self.statement(statement, state, record);
            flow.next = current.next;
            flow.breaks = merge(flow.breaks, current.breaks);
            flow.continues = merge(flow.continues, current.continues);
        }
        flow
    }

    fn statement(&mut self, statement: &Statement, mut state: State, record: bool) -> Flow {
        match statement {
            Statement::Assign(slot, value) => {
                state.0[*slot] = self.expression(value, &state, record)
            }
            Statement::Move(target, source) => {
                state.0[*target] = state.0[*source];
                state.0[*source] = Interval::UNKNOWN;
            }
            Statement::Clear(slot) => state.0[*slot] = Interval::UNKNOWN,
            Statement::Scope { body, clear } => {
                let mut flow = self.block(body, state, record);
                for state in [&mut flow.next, &mut flow.breaks, &mut flow.continues]
                    .into_iter()
                    .flatten()
                {
                    for &slot in clear {
                        state.0[slot] = Interval::UNKNOWN;
                    }
                }
                return flow;
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                let unknown = State::unknown(state.0.len());
                let mut flow = self.block(body, state, record);
                if let Some(next) = flow.next.take() {
                    flow = flow.join(self.block(orelse, next, record));
                }
                // A handler sees any completed prefix, not the normal exit.
                // Finally may also run on return or an uncaught exception.
                for handler in handlers {
                    flow = flow.join(self.block(&handler.body, unknown.clone(), record));
                }
                let final_flow = self.block(finalbody, unknown.clone(), record);
                if !finalbody.is_empty() {
                    if final_flow.next.is_none() {
                        flow.next = None;
                    }
                    flow.breaks = merge(flow.breaks, final_flow.breaks);
                    flow.continues = merge(flow.continues, final_flow.continues);
                }
                for state in [&mut flow.next, &mut flow.breaks, &mut flow.continues]
                    .into_iter()
                    .flatten()
                {
                    *state = unknown.clone();
                }
                return flow;
            }
            Statement::IterNext {
                key, value, ready, ..
            } => {
                // An exhausted iterator preserves its outputs, while success
                // can replace them with any value of the checked storage type.
                state.0[*key] = Interval::UNKNOWN;
                if let Some(value) = value {
                    state.0[*value] = Interval::UNKNOWN;
                }
                state.0[*ready] = Interval::bounded(0, 1);
            }
            Statement::SetItem(list, index, value) => {
                self.expression(value, &state, record);
                self.expression(list, &state, record);
                self.expression(index, &state, record);
            }
            Statement::AugmentItem(list, index, _, value) => {
                self.expression(list, &state, record);
                self.expression(index, &state, record);
                self.expression(value, &state, record);
            }
            Statement::SetField(receiver, _, value) => {
                self.expression(value, &state, record);
                self.expression(receiver, &state, record);
            }
            Statement::AugmentField(receiver, _, _, value) => {
                self.expression(receiver, &state, record);
                self.expression(value, &state, record);
            }
            Statement::Expr(value) => {
                self.expression(value, &state, record);
            }
            Statement::Return(value) => {
                self.expression(value, &state, record);
                return Flow::default();
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    self.expression(value, &state, record);
                }
                return Flow::default();
            }
            Statement::Assert(test) => {
                self.expression(test, &state, record);
                return Flow {
                    next: self.assume(test, true, state),
                    ..Flow::default()
                };
            }
            Statement::If(test, yes, no) => {
                self.expression(test, &state, record);
                let yes = self
                    .assume(test, true, state.clone())
                    .map(|state| self.block(yes, state, record))
                    .unwrap_or_default();
                let no = self
                    .assume(test, false, state)
                    .map(|state| self.block(no, state, record))
                    .unwrap_or_default();
                return yes.join(no);
            }
            Statement::While(test, body) => return self.while_loop(test, body, state, record),
            Statement::For {
                target, args, body, ..
            } => {
                let args = args
                    .iter()
                    .map(|arg| self.expression(arg, &state, record))
                    .collect::<Vec<_>>();
                if record {
                    let interval = cursor_values(&args).unwrap_or(Interval::UNKNOWN);
                    self.cursors
                        .entry(std::ptr::from_ref(statement))
                        .and_modify(|old| *old = old.join(interval))
                        .or_insert(interval);
                }
                return self.for_loop(*target, &args, body, state, record);
            }
            Statement::Break => {
                return Flow {
                    breaks: Some(state),
                    ..Flow::default()
                };
            }
            Statement::Continue => {
                return Flow {
                    continues: Some(state),
                    ..Flow::default()
                };
            }
        }
        Flow::next(state)
    }

    /// The entry and every normal/continue backedge must fit the invariant.
    /// A bounded iteration budget falls back to the universal invariant, never
    /// to an unproved partially converged approximation.
    fn while_loop(&mut self, test: &Expr, body: &[Statement], entry: State, record: bool) -> Flow {
        let mut header = entry.clone();
        for iteration in 0..64 {
            let body_flow = self
                .assume(test, true, header.clone())
                .map(|state| self.block(body, state, false))
                .unwrap_or_default();
            let back = merge(body_flow.next, body_flow.continues);
            let candidate = merge(Some(entry.clone()), back).unwrap();
            let widened = header.widen(&candidate);
            if widened == header {
                break;
            }
            header = if iteration == 63 {
                State::unknown(entry.0.len())
            } else {
                widened
            };
        }
        self.expression(test, &header, record);
        let body_flow = self
            .assume(test, true, header.clone())
            .map(|state| self.block(body, state, record))
            .unwrap_or_default();
        Flow {
            next: merge(self.assume(test, false, header), body_flow.breaks),
            ..Flow::default()
        }
    }

    /// Range arguments are snapshots evaluated before iteration. The visible
    /// target is reset from the independent cursor on every entry, even when the
    /// Python body reassigns it. Zero iterations preserve the incoming locals.
    fn for_loop(
        &mut self,
        target: usize,
        args: &[Interval],
        body: &[Statement],
        entry: State,
        record: bool,
    ) -> Flow {
        let Some(values) = range_values(args) else {
            return Flow::next(entry);
        };
        let mut header = entry.clone();
        for iteration in 0..64 {
            let mut body_state = header.clone();
            body_state.0[target] = values;
            let body_flow = self.block(body, body_state, false);
            let candidate = merge(
                Some(entry.clone()),
                merge(body_flow.next, body_flow.continues),
            )
            .unwrap();
            let widened = header.widen(&candidate);
            if widened == header {
                break;
            }
            header = if iteration == 63 {
                State::unknown(entry.0.len())
            } else {
                widened
            };
        }
        header.0[target] = values;
        let body_flow = self.block(body, header, record);
        Flow {
            next: merge(
                Some(entry),
                merge(body_flow.breaks, merge(body_flow.next, body_flow.continues)),
            ),
            ..Flow::default()
        }
    }

    /// Evaluates children in Python order. Short-circuit successors receive only
    /// facts established by earlier tests; earlier operands keep their original
    /// ranges even when the successful comparison later narrows their locals.
    fn expression(&mut self, expression: &Expr, state: &State, record: bool) -> Interval {
        let result = match &expression.kind {
            ExprKind::Int(value) => value
                .parse::<i128>()
                .map(Interval::exact)
                .unwrap_or(Interval::UNKNOWN),
            ExprKind::Bool(value) => Interval::exact(i128::from(*value)),
            ExprKind::Local(slot) => state.0[*slot],
            ExprKind::Binary(op, left, right) => {
                let left = self.expression(left, state, record);
                let right = self.expression(right, state, record);
                left.binary(*op, right)
            }
            ExprKind::Negate(value) => self.expression(value, state, record).negate(),
            ExprKind::Boolean(and, values) => {
                let mut current = Some(state.clone());
                for value in values {
                    let Some(state) = current else { break };
                    self.expression(value, &state, record);
                    current = self.assume(value, *and, state);
                }
                Interval::bounded(0, 1)
            }
            ExprKind::Conditional(test, yes, no) => {
                self.expression(test, state, record);
                let yes_state = self.assume(test, true, state.clone());
                let no_state = self.assume(test, false, state.clone());
                let yes = yes_state.map(|state| self.expression(yes, &state, record));
                let no = no_state.map(|state| self.expression(no, &state, record));
                match (yes, no) {
                    (Some(yes), Some(no)) => yes.join(no),
                    (Some(value), None) | (None, Some(value)) => value,
                    (None, None) => Interval::UNKNOWN,
                }
            }
            ExprKind::Compare(values, ops) => {
                self.expression(&values[0], state, record);
                let mut current = Some(state.clone());
                for (index, &op) in ops.iter().enumerate() {
                    let Some(state) = current else { break };
                    self.expression(&values[index + 1], &state, record);
                    current = self.comparison(&values[index], &values[index + 1], op, true, state);
                }
                Interval::bounded(0, 1)
            }
            ExprKind::Builtin(builtin, args) => {
                let values = args
                    .iter()
                    .map(|arg| self.expression(arg, state, record))
                    .collect::<Vec<_>>();
                match builtin {
                    Builtin::Len => match args[0].ty {
                        Type::ListInt
                        | Type::List(_)
                        | Type::Dict(_)
                        | Type::Set(_)
                        | Type::Tuple(_) => Interval::bounded(0, MAX_LIST_LEN),
                        Type::Str => Interval {
                            min: Some(0),
                            max: None,
                        },
                        _ => Interval::UNKNOWN,
                    },
                    Builtin::Int if args[0].ty == Type::Int => values[0],
                    Builtin::Int if args[0].ty == Type::Bool => Interval::bounded(0, 1),
                    _ => Interval::UNKNOWN,
                }
            }
            ExprKind::List(values) | ExprKind::Set { values, .. } | ExprKind::Tuple(values) => {
                for value in values {
                    self.expression(value, state, record);
                }
                Interval::UNKNOWN
            }
            ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
                for value in &args.values {
                    self.expression(value, state, record);
                }
                Interval::UNKNOWN
            }
            ExprKind::Dict { entries, .. } => {
                for (key, value) in entries {
                    self.expression(key, state, record);
                    self.expression(value, state, record);
                }
                Interval::UNKNOWN
            }
            ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Not(value)
            | ExprKind::Field(value, _) => {
                self.expression(value, state, record);
                Interval::UNKNOWN
            }
            ExprKind::Exception { message, .. } => {
                if let Some(message) = message {
                    self.expression(message, state, record);
                }
                Interval::UNKNOWN
            }
            ExprKind::Str(_) | ExprKind::None => Interval::UNKNOWN,
        };
        if record && expression.ty == Type::Int {
            self.intervals
                .entry(std::ptr::from_ref(expression))
                .and_modify(|old| *old = old.join(result))
                .or_insert(result);
        }
        result
    }

    fn assume(&mut self, test: &Expr, truth: bool, state: State) -> Option<State> {
        match &test.kind {
            ExprKind::Bool(value) => (*value == truth).then_some(state),
            ExprKind::Not(value) => self.assume(value, !truth, state),
            ExprKind::Boolean(and, values) => {
                if *and == truth {
                    values
                        .iter()
                        .try_fold(state, |state, value| self.assume(value, truth, state))
                } else {
                    let mut alternatives = None;
                    let mut current = Some(state);
                    for value in values {
                        let Some(state) = current else { break };
                        alternatives =
                            merge(alternatives, self.assume(value, truth, state.clone()));
                        current = self.assume(value, !truth, state);
                    }
                    alternatives
                }
            }
            ExprKind::Compare(values, ops) => {
                let mut alternatives = None;
                let mut current = Some(state);
                for (index, &op) in ops.iter().enumerate() {
                    let Some(state) = current else { break };
                    if !truth {
                        alternatives = merge(
                            alternatives,
                            self.comparison(
                                &values[index],
                                &values[index + 1],
                                op,
                                false,
                                state.clone(),
                            ),
                        );
                    }
                    current = self.comparison(&values[index], &values[index + 1], op, true, state);
                }
                if truth { current } else { alternatives }
            }
            _ => Some(state),
        }
    }

    /// Only affine expressions of immutable integer locals can refine storage.
    /// Heap reads and lengths may supply universal bounds, but are never treated
    /// as repeated reads of the same value across user calls or alias mutation.
    fn comparison(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: Compare,
        truth: bool,
        state: State,
    ) -> Option<State> {
        if left.ty != Type::Int || right.ty != Type::Int {
            return Some(state);
        }
        let op = if truth {
            op
        } else {
            match op {
                Compare::Equal => Compare::NotEqual,
                Compare::NotEqual => Compare::Equal,
                Compare::Less => Compare::GreaterEqual,
                Compare::LessEqual => Compare::Greater,
                Compare::Greater => Compare::LessEqual,
                Compare::GreaterEqual => Compare::Less,
            }
        };
        let (left, right, op) = match op {
            Compare::Greater => (right, left, Compare::Less),
            Compare::GreaterEqual => (right, left, Compare::LessEqual),
            _ => (left, right, op),
        };
        let a = self.expression(left, &state, false);
        let b = self.expression(right, &state, false);
        match op {
            Compare::Less | Compare::LessEqual => {
                let strict = i128::from(matches!(op, Compare::Less));
                let left_bound = Interval {
                    min: None,
                    max: b.max.and_then(|value| value.checked_sub(strict)),
                };
                let right_bound = Interval {
                    min: a.min.and_then(|value| value.checked_add(strict)),
                    max: None,
                };
                a.intersect(left_bound)?;
                b.intersect(right_bound)?;
                let state = refine(left, left_bound, state)?;
                refine(right, right_bound, state)
            }
            Compare::Equal => {
                let common = a.intersect(b)?;
                let state = refine(left, common, state)?;
                refine(right, common, state)
            }
            Compare::NotEqual => {
                if a.singleton()
                    .zip(b.singleton())
                    .is_some_and(|(a, b)| a == b)
                {
                    return None;
                }
                let state = exclude(left, a, b.singleton(), state)?;
                exclude(right, b, a.singleton(), state)
            }
            Compare::Greater | Compare::GreaterEqual => unreachable!(),
        }
    }
}

/// Bounds all yielded values, independently of reassignment of the visible loop
/// target. Unknown step signs retain the general path; zero-step failure occurs
/// in the existing runtime lowering before the body can run.
fn range_values(args: &[Interval]) -> Option<Interval> {
    let (start, stop, step) = range_arguments(args);
    let result = if step.min.is_some_and(|value| value > 0) {
        Interval {
            min: start.min,
            max: stop.max.and_then(|value| value.checked_sub(1)),
        }
    } else if step.max.is_some_and(|value| value < 0) {
        Interval {
            min: stop.min.and_then(|value| value.checked_add(1)),
            max: start.max,
        }
    } else {
        Interval::UNKNOWN
    };
    Interval::UNKNOWN.intersect(result)
}

fn range_arguments(args: &[Interval]) -> (Interval, Interval, Interval) {
    match args {
        [stop] => (Interval::exact(0), *stop, Interval::exact(1)),
        [start, stop] => (*start, *stop, Interval::exact(1)),
        [start, stop, step] => (*start, *stop, *step),
        _ => unreachable!("range arity was checked"),
    }
}

/// The last yielded cursor is strictly below/above stop, but its increment can
/// exceed the small range. Include that increment before allowing native loop
/// control; the visible target's interval alone is insufficient.
fn cursor_values(args: &[Interval]) -> Option<Interval> {
    let (start, stop, step) = range_arguments(args);
    let start = start.small()?;
    let stop = stop.small()?;
    let step = step.small()?;
    let result = if step.min > 0 {
        Interval::bounded(
            i128::from(start.min),
            i128::from(start.max).max(i128::from(stop.max) - 1 + i128::from(step.max)),
        )
    } else if step.max < 0 {
        Interval::bounded(
            i128::from(start.min).min(i128::from(stop.min) + 1 + i128::from(step.min)),
            i128::from(start.max),
        )
    } else {
        return None;
    };
    result.small()?;
    Some(result)
}

struct Affine {
    slot: Option<usize>,
    scale: i128,
    offset: i128,
}

/// Recognizes one-local affine forms without re-reading any heap expression.
/// Checked coefficients and offsets make unusually large literals conservative.
fn affine(expression: &Expr) -> Option<Affine> {
    if expression.ty != Type::Int {
        return None;
    }
    match &expression.kind {
        ExprKind::Local(slot) => Some(Affine {
            slot: Some(*slot),
            scale: 1,
            offset: 0,
        }),
        ExprKind::Int(value) => Some(Affine {
            slot: None,
            scale: 0,
            offset: value.parse().ok()?,
        }),
        ExprKind::Negate(value) => {
            let value = affine(value)?;
            Some(Affine {
                slot: value.slot,
                scale: value.scale.checked_neg()?,
                offset: value.offset.checked_neg()?,
            })
        }
        ExprKind::Binary(op, left, right) => {
            let left = affine(left)?;
            let right = affine(right)?;
            match op {
                Binary::Add | Binary::Subtract => {
                    if left.slot.is_some() && right.slot.is_some() && left.slot != right.slot {
                        return None;
                    }
                    let sign = if matches!(op, Binary::Add) { 1 } else { -1 };
                    Some(Affine {
                        slot: left.slot.or(right.slot),
                        scale: left.scale.checked_add(right.scale.checked_mul(sign)?)?,
                        offset: left.offset.checked_add(right.offset.checked_mul(sign)?)?,
                    })
                }
                Binary::Multiply => {
                    let (value, factor) = if right.scale == 0 {
                        (left, right.offset)
                    } else if left.scale == 0 {
                        (right, left.offset)
                    } else {
                        return None;
                    };
                    Some(Affine {
                        slot: value.slot,
                        scale: value.scale.checked_mul(factor)?,
                        offset: value.offset.checked_mul(factor)?,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn refine(expression: &Expr, bounds: Interval, mut state: State) -> Option<State> {
    let Some(form) = affine(expression) else {
        return Some(state);
    };
    if form.scale == 0 {
        Interval::exact(form.offset).intersect(bounds)?;
        return Some(state);
    }
    let slot = form.slot?;
    let lower = bounds.min.and_then(|value| value.checked_sub(form.offset));
    let upper = bounds.max.and_then(|value| value.checked_sub(form.offset));
    let (lower, upper) = if form.scale > 0 {
        (lower, upper)
    } else {
        (upper, lower)
    };
    let bounds = Interval {
        min: lower.and_then(|value| divide(value, form.scale, true)),
        max: upper.and_then(|value| divide(value, form.scale, false)),
    };
    state.0[slot] = state.0[slot].intersect(bounds)?;
    Some(state)
}

/// Intervals can exclude an endpoint, but not a hole in their interior.
fn exclude(
    expression: &Expr,
    interval: Interval,
    value: Option<i128>,
    state: State,
) -> Option<State> {
    let Some(value) = value else {
        return Some(state);
    };
    let bounds = Interval {
        min: if interval.min == Some(value) {
            value.checked_add(1)
        } else {
            None
        },
        max: if interval.max == Some(value) {
            value.checked_sub(1)
        } else {
            None
        },
    };
    refine(expression, bounds, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> Module {
        crate::frontend::lower(&crate::modules::SourceGraph::single(source).unwrap()).unwrap()
    }

    fn expressions(function: &Function) -> Vec<&Expr> {
        fn expression<'a>(value: &'a Expr, output: &mut Vec<&'a Expr>) {
            output.push(value);
            match &value.kind {
                ExprKind::List(values)
                | ExprKind::Set { values, .. }
                | ExprKind::Tuple(values)
                | ExprKind::Compare(values, _)
                | ExprKind::Boolean(_, values)
                | ExprKind::Builtin(_, values) => {
                    for value in values {
                        expression(value, output);
                    }
                }
                ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
                    for value in &args.values {
                        expression(value, output);
                    }
                }
                ExprKind::Dict { entries, .. } => {
                    for (key, value) in entries {
                        expression(key, output);
                        expression(value, output);
                    }
                }
                ExprKind::Present(value)
                | ExprKind::Absent(value)
                | ExprKind::Unwrap(value)
                | ExprKind::Not(value)
                | ExprKind::Negate(value)
                | ExprKind::Field(value, _) => expression(value, output),
                ExprKind::Binary(_, left, right) => {
                    expression(left, output);
                    expression(right, output);
                }
                ExprKind::Conditional(test, yes, no) => {
                    expression(test, output);
                    expression(yes, output);
                    expression(no, output);
                }
                ExprKind::Exception { message, .. } => {
                    if let Some(message) = message {
                        expression(message, output);
                    }
                }
                ExprKind::Int(_)
                | ExprKind::Bool(_)
                | ExprKind::Str(_)
                | ExprKind::None
                | ExprKind::Local(_) => {}
            }
        }
        fn block<'a>(body: &'a [Statement], output: &mut Vec<&'a Expr>) {
            for statement in body {
                match statement {
                    Statement::Assign(_, value)
                    | Statement::Expr(value)
                    | Statement::Return(value)
                    | Statement::Assert(value) => expression(value, output),
                    Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                        for value in [a, b, c] {
                            expression(value, output);
                        }
                    }
                    Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                        expression(a, output);
                        expression(b, output);
                    }
                    Statement::If(test, yes, no) => {
                        expression(test, output);
                        block(yes, output);
                        block(no, output);
                    }
                    Statement::While(test, body) => {
                        expression(test, output);
                        block(body, output);
                    }
                    Statement::Scope { body, .. } => block(body, output),
                    Statement::Try {
                        body,
                        handlers,
                        orelse,
                        finalbody,
                    } => {
                        for body in [body, orelse, finalbody] {
                            block(body, output);
                        }
                        for handler in handlers {
                            block(&handler.body, output);
                        }
                    }
                    Statement::Raise(value) => {
                        if let Some(value) = value {
                            expression(value, output);
                        }
                    }
                    Statement::For { args, body, .. } => {
                        for arg in args {
                            expression(arg, output);
                        }
                        block(body, output);
                    }
                    Statement::Move(_, _)
                    | Statement::Clear(_)
                    | Statement::IterNext { .. }
                    | Statement::Break
                    | Statement::Continue => {}
                }
            }
        }
        let mut output = Vec::new();
        block(&function.body, &mut output);
        output
    }

    #[test]
    fn conditional_results_join_only_the_selected_branch_ranges() {
        let module =
            module("def bounded(value: int) -> int:\n    return value if 0 <= value < 8 else 0\n");
        let facts = analyze(&module);
        let expression = expressions(&module.functions[0])
            .into_iter()
            .find(|expression| matches!(expression.kind, ExprKind::Conditional(..)))
            .unwrap();
        assert_eq!(facts.integer(expression), Some(IntRange { min: 0, max: 7 }));
        let ExprKind::Conditional(_, yes, _) = &expression.kind else {
            unreachable!();
        };
        assert_eq!(facts.integer(yes), Some(IntRange { min: 0, max: 7 }));
    }

    #[test]
    fn conditional_unknown_arm_prevents_small_integer_proof() {
        let module =
            module("def choose(flag: bool, value: int) -> int:\n    return 0 if flag else value\n");
        let facts = analyze(&module);
        let expression = expressions(&module.functions[0])
            .into_iter()
            .find(|expression| matches!(expression.kind, ExprKind::Conditional(..)))
            .unwrap();
        assert_eq!(facts.integer(expression), None);
    }

    #[test]
    fn heap_indices_and_arithmetic_survive_loop_widening() {
        let module = module(
            r#"
def pop_ready(ready: list[int]) -> int:
    result = ready[0]
    last = ready.pop()
    count = len(ready)
    if count > 0:
        position = 0
        while position * 2 + 1 < count:
            child = position * 2 + 1
            if child + 1 < count and ready[child + 1] < ready[child]:
                child += 1
            if ready[child] >= last:
                break
            ready[position] = ready[child]
            position = child
        ready[position] = last
    return result
"#,
        );
        let facts = analyze(&module);
        let mut accesses = 0;
        let mut arithmetic = 0;
        for expression in expressions(&module.functions[0]) {
            match &expression.kind {
                ExprKind::Builtin(Builtin::ListGet, args) => {
                    accesses += 1;
                    assert!(facts.integer(&args[1]).is_some_and(|range| range.min >= 0));
                    assert_eq!(
                        facts.integer(expression),
                        None,
                        "a bounded index does not bound its element"
                    );
                }
                ExprKind::Binary(_, _, _) => {
                    arithmetic += 1;
                    assert!(facts.integer(expression).is_some());
                }
                _ => {}
            }
        }
        assert_eq!(accesses, 5);
        assert!(arithmetic >= 7);
        // Heap elements remain arbitrary precision, while the count and both
        // loop indices have no possible owning BigInt value, even on cleanup.
        assert_eq!(
            facts.small_locals(&module.functions[0]),
            [false, false, false, true, true, true]
        );
    }

    #[test]
    fn guards_refine_successors_but_not_prior_operands() {
        let module = module(
            r#"
def guarded(value: int, values: list[int]) -> int:
    if 0 <= value and value < len(values):
        return values[value]
    return -1

def rejected(value: int, values: list[int]) -> int:
    if value < 0 or value >= len(values):
        return -1
    return values[value]

def chained(value: int, values: list[int]) -> int:
    if 0 <= value < len(values):
        return values[value]
    return -1
"#,
        );
        let facts = analyze(&module);
        for function in &module.functions {
            let mut accesses = 0;
            for expression in expressions(function) {
                match &expression.kind {
                    ExprKind::Builtin(Builtin::ListGet, args) => {
                        accesses += 1;
                        assert_eq!(
                            facts.integer(&args[1]),
                            Some(IntRange {
                                min: 0,
                                max: MAX_LIST_LEN as i64 - 1
                            })
                        );
                    }
                    ExprKind::Compare(values, _) => {
                        for value in values {
                            if matches!(value.kind, ExprKind::Local(_)) {
                                assert_eq!(
                                    facts.integer(value),
                                    None,
                                    "a guard cannot prove its own earlier load small"
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            assert_eq!(accesses, 1);
        }
    }

    #[test]
    fn loop_growth_breaks_and_continues_invalidate_small_facts() {
        let module = module(
            r#"
def growing(again: bool) -> int:
    value = 1
    while again:
        value *= 2
    return value

def breaking(again: bool, huge: int) -> int:
    value = 0
    while again:
        if again:
            value = huge
            break
        value += 1
    return value

def continuing(again: bool, huge: int) -> int:
    value = 0
    while again:
        if again:
            value = huge
            continue
        value = 0
    return value
"#,
        );
        let facts = analyze(&module);
        for function in &module.functions {
            let Some(Statement::Return(value)) = function.body.last() else {
                panic!("expected return")
            };
            assert_eq!(facts.integer(value), None);
            assert!(facts.small_locals(function).into_iter().all(|small| !small));
        }
        let multiply = expressions(&module.functions[0])
            .into_iter()
            .find(|expression| matches!(expression.kind, ExprKind::Binary(Binary::Multiply, _, _)))
            .unwrap();
        assert_eq!(facts.integer(multiply), None);
    }

    #[test]
    fn range_targets_reset_but_rebound_exit_values_remain_unknown() {
        let module = module(
            r#"
def target(values: list[int], huge: int) -> int:
    value = 0
    for value in range(len(values)):
        print(value)
        value = huge
    return value

def descending(values: list[int]) -> None:
    for value in range(len(values) - 1, -1, -1):
        print(value)
"#,
        );
        let facts = analyze(&module);
        for function in &module.functions {
            for expression in expressions(function) {
                if let ExprKind::Builtin(Builtin::Print, args) = &expression.kind {
                    assert_eq!(
                        facts.integer(&args[0]),
                        Some(IntRange {
                            min: 0,
                            max: MAX_LIST_LEN as i64 - 1
                        })
                    );
                }
            }
        }
        let Some(Statement::Return(value)) = module.functions[0].body.last() else {
            panic!("expected return")
        };
        assert_eq!(facts.integer(value), None);
        let function = &module.functions[0];
        let small = facts.small_locals(function);
        let Statement::Scope { body, .. } = &function.body[1] else {
            panic!("expected range scope")
        };
        let Statement::For {
            target,
            cursor,
            stop,
            step,
            ..
        } = &body[0]
        else {
            panic!("expected range loop")
        };
        assert!(!small[*target], "the body can leave a BigInt target owner");
        assert!([cursor, stop, step].into_iter().all(|slot| small[*slot]));
    }

    #[test]
    fn range_control_proofs_include_final_increment() {
        let module = module(
            r#"
def controls(values: list[int], step: int) -> None:
    for value in range(len(values)):
        print(value)
    for value in range(len(values) - 1, -1, -1):
        print(value)
    for value in range(4611686018427387902, 4611686018427387903, 2):
        print(value)
    for value in range(-4611686018427387903, -4611686018427387904, -2):
        print(value)
    for value in range(0, 1, step):
        print(value)
"#,
        );
        let facts = analyze(&module);
        let loops = module.functions[0]
            .body
            .iter()
            .flat_map(|statement| {
                if let Statement::Scope { body, .. } = statement {
                    body.as_slice()
                } else {
                    std::slice::from_ref(statement)
                }
            })
            .filter(|statement| matches!(statement, Statement::For { .. }))
            .collect::<Vec<_>>();
        assert_eq!(loops.len(), 5);
        assert_eq!(
            facts.range_cursor(loops[0]),
            Some(IntRange {
                min: 0,
                max: MAX_LIST_LEN as i64
            })
        );
        assert_eq!(
            facts.range_cursor(loops[1]),
            Some(IntRange {
                min: -1,
                max: MAX_LIST_LEN as i64 - 1
            })
        );
        for statement in &loops[2..] {
            assert_eq!(facts.range_cursor(statement), None);
        }
        let small = facts.small_locals(&module.functions[0]);
        for (index, statement) in loops.into_iter().enumerate() {
            let Statement::For {
                target,
                cursor,
                stop,
                step,
                ..
            } = statement
            else {
                unreachable!()
            };
            assert!(!small[*target], "all loops rebind the same target slot");
            for slot in [cursor, stop, step] {
                assert_eq!(small[*slot], index < 2);
            }
        }
    }

    #[test]
    fn small_locals_reject_parameters_and_any_unknown_replacement() {
        let module = module(
            r#"
def owners(values: list[int], unknown: int, again: bool) -> int:
    count = len(values)
    position = 0
    while position < count:
        position += 1
    replaced = unknown
    replaced = 0
    if again:
        count = unknown
    return position

def parameter(unknown: int) -> int:
    unknown = 0
    return unknown
"#,
        );
        let facts = analyze(&module);
        assert_eq!(
            facts.small_locals(&module.functions[0]),
            [false, false, false, false, true, false]
        );
        assert_eq!(facts.small_locals(&module.functions[1]), [false]);
    }

    #[test]
    fn small_locals_propagate_move_owners_until_stable() {
        let mut module = module("def moving(unknown: int) -> None:\n    pass\n");
        let function = &mut module.functions[0];
        function.locals.extend([Type::Int; 5]);
        function.body = vec![
            Statement::Assign(
                1,
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("1".into()),
                },
            ),
            Statement::Move(2, 1),
            Statement::Move(1, 2),
            // This reverse-order edge needs a second pass after slot 3 becomes
            // ineligible. Clearing it later cannot erase the old owner.
            Statement::Move(4, 3),
            Statement::Move(3, 0),
            Statement::Clear(3),
            Statement::Clear(5),
        ];
        let facts = analyze(&module);
        assert_eq!(
            facts.small_locals(&module.functions[0]),
            [false, true, true, false, false, true]
        );
    }

    #[test]
    fn small_locals_reject_unreachable_writes_without_facts() {
        let mut module = module("def dead() -> int:\n    value = 1\n    return value\n");
        let function = &mut module.functions[0];
        let Statement::Assign(slot, value) = &function.body[0] else {
            panic!("expected assignment")
        };
        function.body.push(Statement::Assign(*slot, value.clone()));
        let facts = analyze(&module);
        let function = &module.functions[0];
        let Some(Statement::Assign(_, value)) = function.body.last() else {
            panic!("expected unreachable assignment")
        };
        assert_eq!(facts.integer(value), None);
        assert_eq!(facts.small_locals(function), [false]);
    }

    #[test]
    fn mapping_iteration_invalidates_integer_outputs() {
        for items in [false, true] {
            let mut module = module("def output() -> None:\n    pass\n");
            let function = &mut module.functions[0];
            function.locals = vec![Type::Iterator, Type::Int, Type::Int, Type::Bool];
            function.body = vec![
                Statement::Assign(
                    1,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Int("1".into()),
                    },
                ),
                Statement::Assign(
                    2,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Int("2".into()),
                    },
                ),
                Statement::IterNext {
                    iterator: 0,
                    key: 1,
                    value: items.then_some(2),
                    ready: 3,
                },
                Statement::Expr(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Local(1),
                }),
                Statement::Expr(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Local(2),
                }),
            ];
            let facts = analyze(&module);
            let function = &module.functions[0];
            let Statement::Expr(key) = &function.body[3] else {
                unreachable!()
            };
            let Statement::Expr(value) = &function.body[4] else {
                unreachable!()
            };
            assert_eq!(facts.integer(key), None);
            assert_eq!(
                facts.integer(value),
                (!items).then_some(IntRange { min: 2, max: 2 })
            );
            assert_eq!(facts.small_locals(function), [false, false, !items, false]);
        }
    }

    #[test]
    fn heap_reads_stay_unknown_while_length_snapshots_survive_mutation() {
        let module = module(
            r#"
def shrink(values: list[int]) -> None:
    values.pop()

def snapshot(values: list[int]) -> int:
    count = len(values)
    shrink(values)
    print(count)
    if 0 <= values[0] and values[0] < 10:
        return values[0]
    return count
"#,
        );
        let facts = analyze(&module);
        let mut reads = 0;
        for expression in expressions(&module.functions[1]) {
            match &expression.kind {
                ExprKind::Builtin(Builtin::ListGet, _) => {
                    reads += 1;
                    assert_eq!(facts.integer(expression), None);
                }
                ExprKind::Builtin(Builtin::Print, args) => {
                    assert_eq!(
                        facts.integer(&args[0]),
                        Some(IntRange {
                            min: 0,
                            max: MAX_LIST_LEN as i64
                        })
                    );
                }
                _ => {}
            }
        }
        assert_eq!(reads, 3);
    }

    #[test]
    fn signed_rounding_and_analysis_overflow_are_conservative() {
        for a in -12_i128..=12 {
            for b in -5_i128..=5 {
                if b == 0 {
                    continue;
                }
                let floor = divide(a, b, false).unwrap();
                let ceiling = divide(a, b, true).unwrap();
                assert!(floor <= ceiling && ceiling <= floor + 1);
                let remainder = a - floor * b;
                assert!(remainder == 0 || (remainder > 0) == (b > 0));
                assert!(remainder.abs() < b.abs());
                assert_eq!(
                    Interval::exact(a).binary(Binary::FloorDivide, Interval::exact(b)),
                    Interval::exact(floor)
                );
                assert!(
                    Interval::exact(a)
                        .binary(Binary::Modulo, Interval::exact(b))
                        .intersect(Interval::exact(remainder))
                        .is_some()
                );
            }
        }
        assert_eq!(
            Interval::exact(i128::MAX).binary(Binary::Add, Interval::exact(1)),
            Interval::UNKNOWN
        );
        assert_eq!(
            Interval::exact(i128::MIN).binary(Binary::Multiply, Interval::exact(-1)),
            Interval::UNKNOWN
        );
        assert_eq!(divide(i128::MIN, -1, false), None);
    }
}
