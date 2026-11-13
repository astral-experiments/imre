//! Bounded native accumulators with an explicit loop-entry budget.
//!
//! The original loop remains the fallback. A fast loop needs a zero initial
//! subtotal, stable input length, a monotone cursor, and at most one bounded
//! addition per iteration. Its accumulator identity carries the resulting
//! induction proof; ordinary interval widening is never treated as that proof.

use crate::ir::{Binary, Builtin, Expr, ExprKind, Module, Statement, Type, walk_expressions};
use crate::list_bounds::block_readonly;
use crate::ranges::{IntRange, IntegerFacts};
use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;

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

/// Rewrites eligible statement loops using the integer bounds of their original addends.
/// Expression-scoped loops do not have statement-level entry facts.
pub(crate) fn specialize(module: &mut Module) {
    let ranges = crate::ranges::analyze(module).into_original_integers();
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        if function.has_expression_bindings() {
            continue;
        }
        rewrite(
            &mut function.body,
            &mut vec![Initial::Unknown; function.locals.len()],
            &ranges,
        );
    }
}

/// Tracks incoming local values and visits each original loop before its children.
fn rewrite(body: &mut [Statement], initial: &mut [Initial], ranges: &IntegerFacts) {
    for statement in body {
        match statement {
            Statement::Assign(slot, value) => {
                let slot = *slot;
                let value = initial_value(value, initial);
                invalidate(std::slice::from_ref(statement), initial);
                initial[slot] = value;
            }
            Statement::Move(target, source) => {
                let target = *target;
                let value = initial[*source];
                invalidate(std::slice::from_ref(statement), initial);
                initial[target] = value;
            }
            Statement::Scope { body, clear } => {
                rewrite(body, initial, ranges);
                for &slot in clear.iter() {
                    invalidate_slot(slot, initial);
                }
            }
            Statement::If(test, yes, no) => {
                invalidate(&[Statement::Expr(test.clone())], initial);
                let mut other = initial.to_vec();
                rewrite(yes, initial, ranges);
                rewrite(no, &mut other, ranges);
                for (left, right) in initial.iter_mut().zip(other) {
                    if *left != right {
                        *left = Initial::Unknown;
                    }
                }
            }
            Statement::While(..) | Statement::For { .. } | Statement::Try { .. } => {
                let replacement = version(statement, initial, ranges);
                if matches!(statement, Statement::Try { .. }) {
                    initial.fill(Initial::Unknown);
                } else {
                    invalidate(std::slice::from_ref(statement), initial);
                }
                if let Some((guard, fast)) = replacement {
                    let fallback = std::mem::replace(statement, Statement::Break);
                    *statement = Statement::If(guard, vec![fast], vec![fallback]);
                } else {
                    crate::ir::statement_children_mut(statement, |child| {
                        if let crate::ir::ChildMut::Suite(body) = child {
                            rewrite(body, &mut vec![Initial::Unknown; initial.len()], ranges);
                        }
                    });
                }
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
    ranges: &IntegerFacts,
) -> Option<(Expr, Statement)> {
    let Statement::While(test, body, _) = statement else {
        return None;
    };
    let (cursor, source, reverse) = loop_source(test, initial)?;
    if !block_readonly(body) || crate::ir::contains_loop(body) {
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
    prove_prefix_reads(&mut body[..advance], source, cursor);
    bound_accumulator(body, *accumulator, bounds);
    Some((
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(
                vec![length(source), Expr::integer(budget)],
                vec![CmpOp::LtE],
            ),
        },
        fast,
    ))
}

/// Carries the loop's relational index proof only up to its unique advance.
/// Forward cursors start at zero and enter below the stable source length;
/// reverse cursors start below that length and enter at a nonnegative value.
/// The checked write/callback analysis preserves both locals and the rooted
/// list throughout this prefix. Reads after the advance, in the else suite,
/// and in the original fallback retain their bounds and exception checks.
fn prove_prefix_reads(body: &mut [Statement], source: usize, cursor: usize) {
    fn expression(value: &mut Expr, source: usize, cursor: usize) {
        if let ExprKind::Builtin(operation, args) = &mut value.kind
            && let Builtin::ListGetBounded { min, max, .. } = *operation
            && let [receiver, index] = args.as_slice()
            && matches!(receiver.kind, ExprKind::Local(slot) if slot == source)
            && matches!(index.kind, ExprKind::Local(slot) if slot == cursor)
        {
            *operation = Builtin::ListGetBoundedInBounds { min, max };
            return;
        }
        match &mut value.kind {
            ExprKind::Scope { body, value, .. } => {
                prove_prefix_reads(body, source, cursor);
                expression(value, source, cursor);
            }
            // Bound-method captures are outside the index proof.
            ExprKind::Callable { .. } => {}
            _ => crate::ir::children_mut(value, |child| expression(child, source, cursor)),
        }
    }
    for statement in body {
        crate::ir::statement_children_mut(statement, |child| match child {
            crate::ir::ChildMut::Expression(value) => expression(value, source, cursor),
            crate::ir::ChildMut::Suite(body) => prove_prefix_reads(body, source, cursor),
        });
    }
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
        [CmpOp::Lt] if initial[cursor] == Initial::Zero => {
            Some((cursor, list_length(right)?, false))
        }
        [CmpOp::GtE] if literal(right) == Some(0) => {
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
    ranges: &IntegerFacts,
    output: &mut Vec<(usize, IntRange)>,
) {
    for statement in body {
        match statement {
            Statement::Assign(slot, expression)
                if *slot != cursor && initial[*slot] == Initial::Zero && writes[*slot] == 1 =>
            {
                if let ExprKind::Binary(Binary::Add, left, right) = &expression.kind
                    && matches!(left.kind, ExprKind::Local(source) if source == *slot)
                    && let Some(term) = ranges.get(right)
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
pub(crate) fn count_writes(body: &[Statement], writes: &mut [usize]) {
    crate::ir::walk_writes(body, |slot, _| writes[slot] += 1);
}

pub(crate) fn contains_continue(body: &[Statement]) -> bool {
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

pub(super) fn literal(value: &Expr) -> Option<i64> {
    let value = match &value.kind {
        ExprKind::Int(value) => value.parse().ok()?,
        ExprKind::Unary(UnaryOp::USub, value) => literal(value)?.checked_neg()?,
        _ => return None,
    };
    (MIN_SMALL..=MAX_SMALL)
        .contains(&i128::from(value))
        .then_some(value)
}

fn length(source: usize) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Builtin(Builtin::Len, vec![Expr::local(source, Type::ListInt)]),
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

    fn list_reads(body: &[Statement]) -> (usize, usize) {
        let mut checked = 0;
        let mut proven = 0;
        walk_expressions(body, &mut |value| match value.kind {
            ExprKind::Builtin(Builtin::ListGetBounded { .. }, _) => checked += 1,
            ExprKind::Builtin(Builtin::ListGetBoundedInBounds { .. }, _) => proven += 1,
            _ => {}
        });
        (checked, proven)
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
        let llvm = crate::llvm::emit(&module, &ranges, false).unwrap();
        assert!(llvm.contains("call void @llvm.assume"));
        assert!(llvm.contains("call i64 @int_add"));
        assert!(llvm.contains("call i64 @imre_list_borrow_index(ptr"));
        let (_, read) = llvm
            .split_once("call i64 @imre_list_borrow_index_proven(")
            .unwrap();
        let (before_tag, _) = read.split_once("call void @llvm.assume").unwrap();
        assert!(before_tag.contains("and i64"));
        assert!(
            !before_tag
                .lines()
                .any(|line| line.contains("icmp eq i64") && line.ends_with(", 0"))
        );
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
            assert_eq!(list_reads(&module.functions[0].body), (1, 1));
        }
    }

    #[test]
    fn only_the_exact_pre_advance_read_loses_its_bounds_check() {
        let mut module = lower(
            r#"
def total(text: str, other_text: str) -> int:
    digits = list(map(int, text))
    other = list(map(int, other_text))
    result = 0
    cursor = 0
    while cursor < len(digits):
        digit = digits[cursor]
        other_digit = other[cursor]
        next_digit = digits[cursor + 1]
        cursor += 1
        after_advance = digits[cursor]
        result += digit
    else:
        after_loop = digits[cursor]
    return result
"#,
        );
        specialize(&mut module);
        let Statement::If(_, fast, fallback) = module.functions[0]
            .body
            .iter()
            .find(|statement| matches!(statement, Statement::If(..)))
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(list_reads(fast), (4, 1));
        assert_eq!(list_reads(fallback), (5, 0));
    }

    #[test]
    fn prefix_scopes_and_branches_keep_the_same_cursor_and_source_proof() {
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
        let Statement::While(_, body, _) = module.functions[0]
            .body
            .iter_mut()
            .find(|statement| matches!(statement, Statement::While(..)))
            .unwrap()
        else {
            unreachable!()
        };
        let read = body.remove(0);
        body.insert(
            0,
            Statement::Expr(Expr {
                ty: Type::Int,
                kind: ExprKind::Scope {
                    body: vec![Statement::If(
                        Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Bool(true),
                        },
                        vec![read.clone()],
                        vec![read],
                    )],
                    value: Box::new(Expr::integer(0)),
                    clear: vec![],
                },
            }),
        );
        specialize(&mut module);
        assert_eq!(list_reads(&module.functions[0].body), (2, 2));
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
            for function in &module.functions {
                assert_eq!(list_reads(&function.body).1, 0);
            }
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
                value: Box::new(Expr::integer(0)),
                clear: vec![cursor],
            },
        }));
        specialize(&mut module);
        assert!(bounds(&module).is_empty());
        assert_eq!(list_reads(&module.functions[0].body).1, 0);
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
