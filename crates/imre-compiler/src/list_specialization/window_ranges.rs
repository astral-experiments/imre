use super::{builtin, list_local};
use crate::ir::{self, Builtin, Expr, ExprKind, Function, Statement, Type};
use crate::ranges::{self, IntegerFacts};

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

/// Recognizes the compiler-created alternatives before attaching extra guards or
/// removing an uncertified version. A user conditional can also have two loop arms.
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
    let mut inputs = std::collections::BTreeSet::from([*output]);
    let mut eligible = true;
    ir::walk_expressions_mut(&mut narrow, &mut |value| {
        if let ExprKind::Builtin(operation, args) = &mut value.kind {
            match operation {
                Builtin::ListGetStable => {
                    if let ExprKind::Local(slot) = args[0].kind {
                        inputs.insert(slot);
                        *operation = Builtin::ListGetI32;
                    } else {
                        eligible = false;
                    }
                }
                Builtin::GetInAppendWindow(slot) if *slot == *output => {
                    *operation = Builtin::GetBoundedInAppendWindow {
                        output: *output,
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
        kind: ExprKind::Compare(
            vec![budget, Expr::integer(8)],
            vec![ir::Compare::GreaterEqual],
        ),
    }];
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
