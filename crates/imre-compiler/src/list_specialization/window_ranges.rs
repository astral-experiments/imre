use super::{builtin, list_local};
use crate::ir::{self, Builtin, Expr, ExprKind, Function, Statement, Type};
use crate::ranges::{self, IntegerFacts};
use ruff_python_ast::CmpOp;

const LOW: i64 = i32::MIN as i64;
const HIGH: i64 = i32::MAX as i64;

/// Checks the list-tail interval on entry and after every append.
pub(super) fn specialize(function: &mut Function) {
    let mut proposed = false;
    ir::rewrite_statements(&mut function.body, &mut |statement| {
        proposed |= propose(statement)
    });
    if !proposed {
        return;
    }
    // A tail can expand beyond the scanned i32 inputs. Retry after widening
    // or discarding any assumption, including facts used by a later loop.
    for attempt in 0.. {
        let facts = ranges::integer_bounds(function);
        let mut changed = false;
        ir::rewrite_statements(&mut function.body, &mut |statement| {
            changed |= certify(statement, &facts, attempt >= 3);
        });
        if !changed {
            return;
        }
    }
}

/// Only equivalent compiler-created branches may acquire an extra runtime guard.
pub(crate) fn is_window_range_guard(test: &Expr) -> bool {
    let ExprKind::Boolean(true, guards) = &test.kind else {
        return false;
    };
    let Some(Expr {
        kind: ExprKind::Compare(operands, _),
        ..
    }) = guards.first()
    else {
        return false;
    };
    matches!(
        operands.first(),
        Some(Expr {
            kind: ExprKind::Builtin(Builtin::CursorScanBudget(_), _),
            ..
        })
    )
}

fn propose(statement: &mut Statement) -> bool {
    let Statement::Try {
        body, finalbody, ..
    } = statement
    else {
        return false;
    };
    let [
        Statement::Expr(Expr {
            kind: ExprKind::Builtin(Builtin::FinishListAppends(output), _),
            ..
        }),
    ] = finalbody.as_slice()
    else {
        return false;
    };
    let [Statement::While(_, _, _)] = body.as_slice() else {
        return false;
    };
    let mut cursor = None;
    ir::walk_statements(body, &mut |statement| {
        if let Statement::IterNext {
            iterator,
            kind: ir::IterNextKind::CodePoint,
            ..
        } = statement
        {
            cursor = Some(*iterator);
        }
    });
    let Some(cursor) = cursor else { return false };
    let mut narrow = body.clone();
    let counters = bound_cursor_counters(&mut narrow, cursor);
    let mut inputs = std::collections::BTreeSet::from([*output]);
    let mut eligible = true;
    ir::walk_expressions_mut(&mut narrow, &mut |value| {
        if let ExprKind::Builtin(operation, args) = &mut value.kind {
            match operation {
                Builtin::ListGetStable(origin) => {
                    if let ExprKind::Local(slot) = args[0].kind {
                        inputs.insert(slot);
                        *operation = Builtin::ListGetI32(origin.take());
                    } else {
                        eligible = false;
                    }
                }
                Builtin::GetInAppendWindow(slot, origin) if *slot == *output => {
                    *operation = Builtin::GetBoundedInAppendWindow {
                        output: *output,
                        origin: origin.take(),
                        min: LOW,
                        max: HIGH,
                    };
                }
                _ => {}
            }
        }
    });
    if !eligible {
        return false;
    }
    let budget = Expr {
        ty: Type::Int,
        kind: ExprKind::Builtin(Builtin::CursorScanBudget(cursor), Vec::new()),
    };
    let scans = inputs
        .into_iter()
        .map(|slot| {
            builtin(
                Builtin::ListAllI32,
                vec![
                    list_local(slot),
                    Expr::integer(0),
                    budget.clone(),
                    Expr::integer(1),
                ],
            )
        })
        .collect::<Vec<_>>();
    let mut guards = vec![Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(vec![budget, Expr::integer(8)], vec![CmpOp::GtE]),
    }];
    guards.extend(counters.into_iter().map(|slot| {
        builtin(
            Builtin::IntFitsI32,
            vec![Expr {
                ty: Type::Int,
                kind: ExprKind::Local(slot),
            }],
        )
    }));
    guards.extend(scans);
    let original = std::mem::take(body);
    body.push(Statement::If(
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Boolean(true, guards),
        },
        narrow,
        original,
    ));
    true
}

fn certify(statement: &mut Statement, facts: &IntegerFacts, last: bool) -> bool {
    let Statement::If(test, yes, no) = statement else {
        return false;
    };
    if !is_window_range_guard(test) {
        return false;
    }
    let [Statement::While(_, body, _)] = yes.as_mut_slice() else {
        return false;
    };
    if !matches!(no.as_slice(), [Statement::While(_, _, _)]) {
        return false;
    }
    let mut assumed = None;
    let (mut low, mut high) = (LOW, HIGH);
    let mut unknown = false;
    ir::walk_expressions(body, &mut |value| {
        if let ExprKind::Builtin(operation, args) = &value.kind {
            match operation {
                Builtin::GetBoundedInAppendWindow { min, max, .. } => assumed = Some((*min, *max)),
                Builtin::AppendInWindow(_) => match facts.get(&args[1]) {
                    Some(interval) => {
                        low = low.min(interval.min);
                        high = high.max(interval.max);
                    }
                    None => unknown = true,
                },
                _ => {}
            }
        }
    });
    let Some((old_low, old_high)) = assumed else {
        return false;
    };
    if !unknown && old_low <= low && old_high >= high {
        return false;
    }
    if unknown || last {
        *statement = no.pop().unwrap();
    } else {
        ir::walk_expressions_mut(body, &mut |value| {
            if let ExprKind::Builtin(Builtin::GetBoundedInAppendWindow { min, max, .. }, _) =
                &mut value.kind
            {
                *min = old_low.min(low);
                *max = old_high.max(high);
            }
        });
    }
    true
}

/// A successful scalar pull advances at least one byte, including on multibyte
/// strings. A unique update gated by its ready flag cannot outnumber those bytes.
fn bound_cursor_counters(narrow: &mut [Statement], cursor: usize) -> Vec<usize> {
    let [Statement::While(_, body, _)] = narrow else {
        return vec![];
    };
    let Some(ready) = checked_pull_flag(body, cursor) else {
        return vec![];
    };
    let mut writes = std::collections::BTreeMap::new();
    ir::walk_writes(body, |slot, _| *writes.entry(slot).or_insert(0usize) += 1);
    let mut bounds = std::collections::BTreeMap::new();
    visit_successful_pulls(body, ready, |statements| {
        ir::walk_writes(statements, |slot, rhs| {
            if writes[&slot] == 1
                && let Some(Expr {
                    kind: ExprKind::Binary(op, left, right),
                    ty: Type::Int,
                }) = rhs
                && matches!(left.kind,ExprKind::Local(id) if id == slot)
                && let ExprKind::Int(literal) = &right.kind
                && let Ok(magnitude) = literal.parse::<i32>()
            {
                let step = match op {
                    ir::Binary::Add => i64::from(magnitude),
                    ir::Binary::Subtract => -i64::from(magnitude),
                    _ => return,
                };
                let change = i128::from(HIGH) * i128::from(step);
                let min = i128::from(LOW) + change.min(0);
                let max = i128::from(HIGH) + change.max(0);
                if min >= -(1_i128 << 62) && max < 1_i128 << 62 {
                    bounds.insert(slot, (min as i64, max as i64));
                }
            }
        });
    });
    fn rewrite(body: &mut [Statement], bounds: &std::collections::BTreeMap<usize, (i64, i64)>) {
        fn expr(value: &mut Expr, bounds: &std::collections::BTreeMap<usize, (i64, i64)>) {
            if let ExprKind::Local(slot) = value.kind
                && let Some(&(min, max)) = bounds.get(&slot)
            {
                let local = value.clone();
                value.kind =
                    ExprKind::Builtin(Builtin::BoundedCursorValue { min, max }, vec![local]);
            } else if let ExprKind::Scope { body, value, .. } = &mut value.kind {
                rewrite(body, bounds);
                expr(value, bounds);
            } else {
                ir::children_mut(value, |v| expr(v, bounds));
            }
        }
        for stmt in body {
            ir::statement_children_mut(stmt, |child| match child {
                ir::ChildMut::Expression(value) => expr(value, bounds),
                ir::ChildMut::Suite(body) => rewrite(body, bounds),
            });
        }
    }
    if !bounds.is_empty() {
        rewrite(body, &bounds);
    }
    bounds.into_keys().collect()
}

/// Returns the success flag of a single cursor pull, reset on every iteration.
fn checked_pull_flag(body: &[Statement], cursor: usize) -> Option<usize> {
    let mut ready = None;
    let mut pulls = 0;
    let mut clears = 0;
    let mut inner_loop = false;
    ir::walk_statements(body, &mut |stmt| match stmt {
        Statement::IterNext {
            iterator,
            ready: flag,
            kind: ir::IterNextKind::CodePoint,
            ..
        } if *iterator == cursor => {
            ready = Some(*flag);
            pulls += 1;
        }
        Statement::Clear(slot) if *slot == cursor => clears += 1,
        Statement::While(..) | Statement::For { .. } => inner_loop = true,
        _ => {}
    });
    let ready = ready?;
    if pulls != 1
        || inner_loop
        || !matches!(body.iter().find(|s| !matches!(s,Statement::Clear(_))),
        Some(Statement::Assign(slot, Expr {kind:ExprKind::Bool(false), ..})) if *slot == ready)
    {
        return None;
    }
    let mut writes = std::collections::BTreeMap::new();
    ir::walk_writes(body, |slot, _| *writes.entry(slot).or_insert(0usize) += 1);
    if writes.get(&ready) != Some(&2) || writes.get(&cursor).copied().unwrap_or(0) != clears {
        return None;
    }
    Some(ready)
}

/// Visits statements that execute only after the iterator produced a scalar.
fn visit_successful_pulls(body: &[Statement], ready: usize, mut visit: impl FnMut(&[Statement])) {
    let mut after_success = false;
    for statement in body {
        if after_success {
            visit(std::slice::from_ref(statement));
        } else if let Some((produced, empty)) = statement.branches_for_flag(ready) {
            visit(produced);
            after_success = produced.is_empty()
                && matches!(
                    empty,
                    [Statement::Exhaust | Statement::Break(_) | Statement::Continue(_)]
                );
        }
    }
}

/// Proves that every write to a certified integer append window follows one
/// successful pull from the same monotonically advancing scalar cursor.
pub(crate) fn reserved_append_window(suite: &[Statement]) -> Option<(usize, usize)> {
    let [Statement::While(_, body, otherwise)] = suite else {
        return None;
    };
    if !otherwise.is_empty() {
        return None;
    }
    let mut cursor = None;
    ir::walk_statements(body, &mut |stmt| {
        if let Statement::IterNext {
            iterator,
            kind: ir::IterNextKind::CodePoint,
            ..
        } = stmt
        {
            cursor = Some(*iterator)
        }
    });
    let cursor = cursor?;
    let ready = checked_pull_flag(body, cursor)?;
    let mut appends = 0;
    let mut target = None;
    let mut tail = None;
    ir::walk_expressions(body, &mut |v| match v.kind {
        ExprKind::Builtin(Builtin::AppendInWindow(out), _) => {
            appends += 1;
            target = Some(out)
        }
        ExprKind::Builtin(Builtin::GetBoundedInAppendWindow { output, .. }, _) => {
            tail = Some(output)
        }
        _ => {}
    });
    let out = target?;
    if appends != 1 || tail != Some(out) {
        return None;
    }
    fn writes(suite: &[Statement], out: usize) -> usize {
        let mut count = 0;
        ir::walk_expressions(suite, &mut |value| {
            if matches!(value.kind,ExprKind::Builtin(Builtin::AppendInWindow(slot),_) if slot==out)
            {
                count += 1
            }
        });
        count
    }
    // Only the tail has been checked. An ordinary read of the output could
    // observe an arbitrary-precision integer in the existing prefix.
    let mut references = 0;
    let mut window_receivers = 0;
    ir::walk_expressions(body, &mut |value| match &value.kind {
        ExprKind::Local(slot) if *slot == out => references += 1,
        ExprKind::Builtin(operation, args)
            if matches!(operation,
                Builtin::AppendInWindow(slot) | Builtin::GetBoundedInAppendWindow { output: slot, .. }
                    if *slot == out)
                && matches!(args.first().map(|arg| &arg.kind), Some(ExprKind::Local(slot)) if *slot == out) =>
        {
            window_receivers += 1;
        }
        _ => {}
    });
    if references != window_receivers {
        return None;
    }
    let mut gated = 0;
    visit_successful_pulls(body, ready, |statements| gated += writes(statements, out));
    (gated == 1).then_some((out, cursor))
}
