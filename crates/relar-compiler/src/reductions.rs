//! Bounded native accumulators with an explicit loop-entry budget.
//!
//! The original loop remains the fallback. A fast loop needs a zero initial
//! subtotal, stable input length, a monotone cursor, and at most one bounded
//! addition per iteration. Its accumulator identity carries the resulting
//! induction proof; ordinary interval widening is never treated as that proof.

use std::collections::HashMap;

use crate::ir::{
    Binary, Builtin, Compare, Expr, ExprKind, Module, Statement, Type, walk_expressions,
};
use crate::list_bounds::block_readonly;
use crate::ranges::{IntRange, RangeFacts};

const MIN_SMALL: i128 = -(1_i128 << 62);
const MAX_SMALL: i128 = (1_i128 << 62) - 1;
const MAX_LIST_LEN: i128 = (1_i128 << 60) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Initial {
    Unknown,
    Zero,
    Reverse { source: usize, offset: i64 },
}

impl Initial {
    fn source(self) -> Option<usize> {
        match self {
            Self::Reverse { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Plans against one immutable range snapshot, then installs checked versions.
/// Expression-scoped loops remain unchanged; private comprehension helpers use
/// ordinary statement regions and need no broader expression-flow inference.
pub(crate) fn specialize(module: &mut Module) {
    let ranges = crate::ranges::analyze(module);
    let mut plans = HashMap::new();
    for function in module.functions.iter().chain([&module.entry]) {
        if function.expression_rebound().into_iter().any(|value| value) {
            continue;
        }
        collect(
            &function.body,
            &mut vec![Initial::Unknown; function.locals.len()],
            &ranges,
            &mut plans,
        );
    }
    drop(ranges);
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        install(&mut function.body, &mut plans);
    }
}

fn collect(
    body: &[Statement],
    initial: &mut [Initial],
    ranges: &RangeFacts<'_>,
    plans: &mut HashMap<*const Statement, Statement>,
) {
    for statement in body {
        match statement {
            Statement::Assign(slot, value) => {
                let value = initial_value(value, initial);
                invalidate(std::slice::from_ref(statement), initial);
                initial[*slot] = value;
            }
            Statement::Move(target, source) => {
                let value = initial[*source];
                invalidate(std::slice::from_ref(statement), initial);
                initial[*target] = value;
            }
            Statement::Scope { body, clear } => {
                collect(body, initial, ranges, plans);
                for &slot in clear {
                    invalidate_slot(slot, initial);
                }
            }
            Statement::If(test, yes, no) => {
                invalidate(&[Statement::Expr(test.clone())], initial);
                let mut other = initial.to_vec();
                collect(yes, initial, ranges, plans);
                collect(no, &mut other, ranges, plans);
                for (left, right) in initial.iter_mut().zip(other) {
                    if *left != right {
                        *left = Initial::Unknown;
                    }
                }
            }
            Statement::While(_, body, orelse) => {
                if let Some(version) = version(statement, initial, ranges) {
                    plans.insert(std::ptr::from_ref(statement), version);
                }
                collect(
                    body,
                    &mut vec![Initial::Unknown; initial.len()],
                    ranges,
                    plans,
                );
                collect(
                    orelse,
                    &mut vec![Initial::Unknown; initial.len()],
                    ranges,
                    plans,
                );
                invalidate(std::slice::from_ref(statement), initial);
            }
            Statement::For { body, orelse, .. } => {
                for body in [body, orelse] {
                    collect(
                        body,
                        &mut vec![Initial::Unknown; initial.len()],
                        ranges,
                        plans,
                    );
                }
                invalidate(std::slice::from_ref(statement), initial);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody]
                    .into_iter()
                    .chain(handlers.iter().map(|handler| &handler.body))
                {
                    collect(
                        body,
                        &mut vec![Initial::Unknown; initial.len()],
                        ranges,
                        plans,
                    );
                }
                initial.fill(Initial::Unknown);
            }
            _ => invalidate(std::slice::from_ref(statement), initial),
        }
    }
}

fn invalidate(body: &[Statement], initial: &mut [Initial]) {
    let mut writes = vec![0; initial.len()];
    count_writes(body, &mut writes);
    for (slot, count) in writes.into_iter().enumerate() {
        if count != 0 {
            invalidate_slot(slot, initial);
        }
    }
    if !block_readonly(body) {
        for value in initial {
            if matches!(value, Initial::Reverse { .. }) {
                *value = Initial::Unknown;
            }
        }
    }
}

fn invalidate_slot(slot: usize, initial: &mut [Initial]) {
    initial[slot] = Initial::Unknown;
    for value in initial {
        if value.source() == Some(slot) {
            *value = Initial::Unknown;
        }
    }
}

fn initial_value(value: &Expr, initial: &[Initial]) -> Initial {
    if literal(value) == Some(0) {
        return Initial::Zero;
    }
    if let ExprKind::Local(slot) = value.kind {
        return initial[slot];
    }
    if let ExprKind::Binary(Binary::Add, left, right) = &value.kind
        && let Some(source) = list_length(left)
        && let Some(offset) = literal(right)
        && offset < 0
    {
        return Initial::Reverse { source, offset };
    }
    Initial::Unknown
}

fn version(
    statement: &Statement,
    initial: &[Initial],
    ranges: &RangeFacts<'_>,
) -> Option<Statement> {
    let Statement::While(test, body, _) = statement else {
        return None;
    };
    let (cursor, source, reverse) = loop_source(test, initial)?;
    if !block_readonly(body) || crate::list_specialization::contains_loop(body) {
        return None;
    }
    let mut writes = vec![0; initial.len()];
    count_writes(body, &mut writes);
    if writes[source] != 0 || writes[cursor] != 1 {
        return None;
    }
    let advance = body.iter().position(|statement| {
        let Statement::Assign(slot, value) = statement else {
            return false;
        };
        let ExprKind::Binary(Binary::Add, left, right) = &value.kind else {
            return false;
        };
        *slot == cursor
            && matches!(left.kind, ExprKind::Local(slot) if slot == cursor)
            && literal(right).is_some_and(|step| if reverse { step < 0 } else { step == 1 })
    })?;
    // Exceptions, return, and break do not repeat. Continue does, so the cursor
    // advance must dominate every such path, including expression-owned scopes.
    if contains_continue(&body[..advance]) {
        return None;
    }
    let mut candidates = Vec::new();
    find_accumulators(body, initial, &writes, cursor, ranges, &mut candidates);
    let [(accumulator, term)] = candidates.as_slice() else {
        return None;
    };
    let (budget, bounds) = budget(*term)?;
    let mut fast = statement.clone();
    let Statement::While(_, body, _) = &mut fast else {
        unreachable!()
    };
    bound_accumulator(body, *accumulator, bounds);
    Some(Statement::If(
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(
                vec![length(source), integer(budget)],
                vec![Compare::LessEqual],
            ),
        },
        vec![fast],
        vec![statement.clone()],
    ))
}

fn loop_source(test: &Expr, initial: &[Initial]) -> Option<(usize, usize, bool)> {
    let ExprKind::Compare(values, operations) = &test.kind else {
        return None;
    };
    let [left, right] = values.as_slice() else {
        return None;
    };
    let ExprKind::Local(cursor) = left.kind else {
        return None;
    };
    match operations.as_slice() {
        [Compare::Less] if initial[cursor] == Initial::Zero => {
            Some((cursor, list_length(right)?, false))
        }
        [Compare::GreaterEqual] if literal(right) == Some(0) => {
            let Initial::Reverse { source, .. } = initial[cursor] else {
                return None;
            };
            Some((cursor, source, true))
        }
        _ => None,
    }
}

/// Keeps one extra term of margin beyond every possible loop subtotal. This
/// lets ordinary interval addition prove the next result small without relying
/// on a relational cursor fact at each individual update.
fn budget(term: IntRange) -> Option<(i64, IntRange)> {
    let positive = i128::from(term.max).max(0);
    let negative = (-i128::from(term.min)).max(0);
    if positive == 0 && negative == 0 {
        return None;
    }
    let mut count = MAX_LIST_LEN;
    if positive != 0 {
        count = count.min((MAX_SMALL - positive) / positive);
    }
    if negative != 0 {
        count = count.min((-MIN_SMALL - negative) / negative);
    }
    if count < 1 {
        return None;
    }
    Some((
        count as i64,
        IntRange {
            min: (-count * negative) as i64,
            max: (count * positive) as i64,
        },
    ))
}

fn find_accumulators(
    body: &[Statement],
    initial: &[Initial],
    writes: &[usize],
    cursor: usize,
    ranges: &RangeFacts<'_>,
    output: &mut Vec<(usize, IntRange)>,
) {
    for statement in body {
        match statement {
            Statement::Assign(slot, expression)
                if *slot != cursor && initial[*slot] == Initial::Zero && writes[*slot] == 1 =>
            {
                if let ExprKind::Binary(Binary::Add, left, right) = &expression.kind
                    && matches!(left.kind, ExprKind::Local(source) if source == *slot)
                    && let Some(term) = ranges.integer(right)
                {
                    output.push((*slot, term));
                }
            }
            Statement::Scope { body, .. } => {
                find_accumulators(body, initial, writes, cursor, ranges, output);
            }
            Statement::If(_, yes, no) => {
                for body in [yes, no] {
                    find_accumulators(body, initial, writes, cursor, ranges, output);
                }
            }
            _ => {}
        }
    }
}

fn bound_accumulator(body: &mut [Statement], accumulator: usize, bounds: IntRange) {
    for statement in body {
        match statement {
            Statement::Assign(slot, expression) if *slot == accumulator => {
                let ExprKind::Binary(Binary::Add, left, _) = &mut expression.kind else {
                    unreachable!("the unique accumulator write was checked");
                };
                let value = left.as_ref().clone();
                **left = Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(
                        Builtin::IntBounded {
                            min: bounds.min,
                            max: bounds.max,
                        },
                        vec![value],
                    ),
                };
            }
            Statement::Scope { body, .. } => bound_accumulator(body, accumulator, bounds),
            Statement::If(_, yes, no) => {
                bound_accumulator(yes, accumulator, bounds);
                bound_accumulator(no, accumulator, bounds);
            }
            _ => {}
        }
    }
}

/// Counts every replacement and cleanup, including expression-owned regions.
fn count_writes(body: &[Statement], writes: &mut [usize]) {
    statement_writes(body, writes);
    walk_expressions(body, &mut |value| match &value.kind {
        ExprKind::Scope { body, clear, .. } => {
            statement_writes(body, writes);
            for &slot in clear {
                writes[slot] += 1;
            }
        }
        ExprKind::Bind(slot, _) => writes[*slot] += 1,
        _ => {}
    });
}

fn statement_writes(body: &[Statement], writes: &mut [usize]) {
    for statement in body {
        match statement {
            Statement::Assign(slot, _) | Statement::Clear(slot) => writes[*slot] += 1,
            Statement::Move(target, source) => {
                writes[*target] += 1;
                writes[*source] += 1;
            }
            Statement::Scope { body, clear } => {
                statement_writes(body, writes);
                for &slot in clear {
                    writes[slot] += 1;
                }
            }
            Statement::If(_, yes, no) | Statement::While(_, yes, no) => {
                statement_writes(yes, writes);
                statement_writes(no, writes);
            }
            Statement::For {
                target,
                cursor,
                stop,
                step,
                body,
                orelse,
                ..
            } => {
                for &slot in [target, cursor, stop, step] {
                    writes[slot] += 1;
                }
                statement_writes(body, writes);
                statement_writes(orelse, writes);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody] {
                    statement_writes(body, writes);
                }
                for handler in handlers {
                    if let Some(slot) = handler.name {
                        writes[slot] += 1;
                    }
                    statement_writes(&handler.body, writes);
                }
            }
            Statement::IterNext {
                key, value, ready, ..
            } => {
                for slot in [Some(*key), *value, Some(*ready)].into_iter().flatten() {
                    writes[slot] += 1;
                }
            }
            _ => {}
        }
    }
}

fn contains_continue(body: &[Statement]) -> bool {
    fn statements(body: &[Statement]) -> bool {
        body.iter().any(|statement| match statement {
            Statement::Continue => true,
            Statement::Scope { body, .. } => statements(body),
            Statement::If(_, yes, no) => statements(yes) || statements(no),
            _ => false,
        })
    }
    let mut found = statements(body);
    walk_expressions(body, &mut |value| {
        if let ExprKind::Scope { body, .. } = &value.kind {
            found |= statements(body);
        }
    });
    found
}

fn install(body: &mut [Statement], plans: &mut HashMap<*const Statement, Statement>) {
    for statement in body {
        if let Some(version) = plans.remove(&std::ptr::from_ref(statement)) {
            *statement = version;
            continue;
        }
        match statement {
            Statement::Scope { body, .. } => install(body, plans),
            Statement::If(_, yes, no) | Statement::While(_, yes, no) => {
                install(yes, plans);
                install(no, plans);
            }
            Statement::For { body, orelse, .. } => {
                install(body, plans);
                install(orelse, plans);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody] {
                    install(body, plans);
                }
                for handler in handlers {
                    install(&mut handler.body, plans);
                }
            }
            _ => {}
        }
    }
}

fn list_length(value: &Expr) -> Option<usize> {
    let ExprKind::Builtin(Builtin::Len, args) = &value.kind else {
        return None;
    };
    match args.as_slice() {
        [
            Expr {
                ty: Type::ListInt,
                kind: ExprKind::Local(slot),
            },
        ] => Some(*slot),
        _ => None,
    }
}

fn literal(value: &Expr) -> Option<i64> {
    let value = match &value.kind {
        ExprKind::Int(value) => value.parse().ok()?,
        ExprKind::Negate(value) => literal(value)?.checked_neg()?,
        _ => return None,
    };
    (MIN_SMALL..=MAX_SMALL)
        .contains(&i128::from(value))
        .then_some(value)
}

fn integer(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

fn length(source: usize) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Builtin(
            Builtin::Len,
            vec![Expr {
                ty: Type::ListInt,
                kind: ExprKind::Local(source),
            }],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(source: &str) -> Module {
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        crate::tuples::scalarize(&mut module);
        crate::list_bounds::specialize(&mut module);
        module
    }

    fn bounds(module: &Module) -> Vec<IntRange> {
        let mut result = Vec::new();
        for function in module.functions.iter().chain([&module.entry]) {
            walk_expressions(&function.body, &mut |value| {
                if let ExprKind::Builtin(Builtin::IntBounded { min, max }, _) = value.kind {
                    result.push(IntRange { min, max });
                }
            });
        }
        result
    }

    #[test]
    fn canonical_comprehension_reduction_keeps_a_checked_fallback() {
        let mut module = lower(
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    return sum([sum(divmod(2 * digit, 10)) for digit in digits])
"#,
        );
        specialize(&mut module);
        let proof = bounds(&module);
        assert_eq!(proof.len(), 1);
        assert_eq!(proof[0].min, 0);
        assert!(i128::from(proof[0].max) + 10 <= MAX_SMALL);
        let ranges = crate::ranges::analyze(&module);
        let mut native_adds = 0;
        let mut generic_adds = 0;
        for function in &module.functions {
            walk_expressions(&function.body, &mut |value| {
                if let ExprKind::Binary(Binary::Add, left, _) = &value.kind {
                    if matches!(left.kind, ExprKind::Builtin(Builtin::IntBounded { .. }, _)) {
                        assert!(ranges.integer(value).is_some());
                        native_adds += 1;
                    } else if ranges.integer(value).is_none() {
                        generic_adds += 1;
                    }
                }
            });
        }
        assert_eq!(native_adds, 1);
        assert!(
            generic_adds != 0,
            "the original accumulator remains the fallback"
        );
        let llvm = crate::llvm::emit(&module, &ranges, false);
        assert!(llvm.contains("call void @llvm.assume"));
        assert!(llvm.contains("call i64 @int_add"));
    }

    #[test]
    fn reverse_iteration_and_negative_terms_use_the_same_trip_bound() {
        for term in ["digit", "-digit"] {
            let mut module = lower(&format!(
                r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = len(digits) + -2
    while cursor >= 0:
        digit = digits[cursor]
        cursor += -2
        result += {term}
    return result
"#
            ));
            specialize(&mut module);
            let proof = bounds(&module);
            assert_eq!(proof.len(), 1);
            assert!(i128::from(proof[0].min) - 9 >= MIN_SMALL);
            assert!(i128::from(proof[0].max) + 9 <= MAX_SMALL);
        }
    }

    #[test]
    fn continue_and_break_after_cursor_advance_only_reduce_the_budget() {
        let mut module = lower(
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        if digit == 0:
            continue
        result += digit
        if digit == 9:
            break
    return result
"#,
        );
        specialize(&mut module);
        assert_eq!(bounds(&module).len(), 1);
    }

    #[test]
    fn rejects_unknown_terms_nonzero_start_and_unstable_induction() {
        for source in [
            r#"
def total(values: list[int]) -> int:
    result = 0
    for value in values:
        result += value
    return result
"#,
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 1
    for digit in digits:
        result += digit
    return result
"#,
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        if digit == 0:
            continue
        cursor += 1
        result += digit
    return result
"#,
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        result += digit
        result = 0
    return result
"#,
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        result += digit
        if digit == 0:
            cursor = 0
    return result
"#,
            r#"
def mutate(values: list[int]) -> None:
    values.append(1000)

def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    for digit in digits:
        result += digit
        mutate(digits)
    return result
"#,
        ] {
            let mut module = lower(source);
            specialize(&mut module);
            assert!(bounds(&module).is_empty());
        }
    }

    #[test]
    fn expression_hidden_cleanup_invalidates_cursor_identity() {
        let mut module = lower(
            r#"
def total(text: str) -> int:
    digits = list(map(int, text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        cursor += 1
        result += digit
    return result
"#,
        );
        let Statement::While(test, body, _) = module.functions[0]
            .body
            .iter_mut()
            .find(|statement| matches!(statement, Statement::While(..)))
            .unwrap()
        else {
            unreachable!()
        };
        let ExprKind::Compare(values, _) = &test.kind else {
            unreachable!()
        };
        let ExprKind::Local(cursor) = values[0].kind else {
            unreachable!()
        };
        body.push(Statement::Expr(Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body: Vec::new(),
                value: Box::new(integer(0)),
                clear: vec![cursor],
            },
        }));
        specialize(&mut module);
        assert!(bounds(&module).is_empty());
    }

    #[test]
    fn budget_has_one_term_of_headroom_and_rejects_unbounded_growth() {
        for term in [
            IntRange { min: 0, max: 9 },
            IntRange { min: -9, max: 0 },
            IntRange {
                min: -2_147_483_648,
                max: 2_147_483_647,
            },
        ] {
            let (count, bound) = budget(term).unwrap();
            assert!(count >= 1);
            assert!(i128::from(count) <= MAX_LIST_LEN);
            assert!(i128::from(bound.min) + i128::from(term.min) >= MIN_SMALL);
            assert!(i128::from(bound.max) + i128::from(term.max) <= MAX_SMALL);
        }
        for term in [
            IntRange { min: 0, max: 0 },
            IntRange {
                min: 0,
                max: MAX_SMALL as i64,
            },
            IntRange {
                min: MIN_SMALL as i64,
                max: 0,
            },
        ] {
            assert!(budget(term).is_none());
        }
    }
}
