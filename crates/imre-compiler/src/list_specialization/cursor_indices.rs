//! Bound stable list indices by the remaining pulls of a UTF-8 cursor.
use super::window_ranges::{checked_pull_flag, visit_single_pull_writes};
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use std::collections::BTreeMap;

/// Every recorded index lies in this inclusive band relative to its counter at
/// iteration entry. A successful pull leaves at most byte_budget - 1 prior pulls.
pub(crate) struct Span {
    pub(crate) list: usize,
    pub(crate) counter: usize,
    pub(crate) low: i32,
    pub(crate) high: i32,
    /// Addresses of the index operands in this suite, valid while emitting the same IR.
    pub(crate) indices: Vec<usize>,
}

type Origins = BTreeMap<usize, (usize, i32)>;

/// Only copies and constant offsets preserve the identity of an ordinal.
fn origin(value: &Expr, locals: &Origins) -> Option<(usize, i32)> {
    if value.ty != Type::Int {
        return None;
    }
    match &value.kind {
        ExprKind::Local(slot) => locals.get(slot).copied(),
        ExprKind::Builtin(Builtin::BoundedCursorValue { .. }, args) => origin(&args[0], locals),
        ExprKind::Scope { body, value, .. } if body.is_empty() => origin(value, locals),
        ExprKind::Binary(op, left, right) => {
            let (root, offset) = origin(left, locals)?;
            let ExprKind::Int(n) = &right.kind else {
                return None;
            };
            let n = n.parse::<i32>().ok()?;
            let offset = match op {
                Binary::Add => offset.checked_add(n)?,
                Binary::Subtract => offset.checked_sub(n)?,
                _ => return None,
            };
            Some((root, offset))
        }
        _ => None,
    }
}

/// Reject all definitions inside a suite whose control flow is unknown.
fn forget_writes(stmt: &Statement, locals: &mut Origins) {
    ir::walk_writes(std::slice::from_ref(stmt), |slot, _| {
        locals.remove(&slot);
    });
}

/// Record reads before applying the writes of a straight-line statement.
fn accept(stmt: &Statement, locals: &mut Origins, spans: &mut BTreeMap<(usize, usize), Span>) {
    if let Statement::Scope { body, clear } = stmt {
        for nested in body {
            accept(nested, locals, spans);
        }
        for slot in clear {
            locals.remove(slot);
        }
        return;
    }
    let assignment = match stmt {
        Statement::Assign(slot, rhs) => Some((*slot, origin(rhs, locals))),
        Statement::Move(dst, src) => Some((*dst, locals.get(src).copied())),
        Statement::Expr(_) => None,
        _ => {
            forget_writes(stmt, locals);
            return;
        }
    };
    // Expressions with eager writes may change an ordinal while evaluating an operand.
    let mut effect = false;
    let mut reads = Vec::new();
    ir::walk_expressions(std::slice::from_ref(stmt), &mut |e| match &e.kind {
        ExprKind::Scope { .. } | ExprKind::Bind(..) => effect = true,
        ExprKind::Builtin(Builtin::ListGetI32, args) => {
            if let ExprKind::Local(list) = args[0].kind
                && let Some((counter, offset)) = origin(&args[1], locals)
            {
                reads.push((list, counter, offset, std::ptr::from_ref(&args[1]) as usize));
            }
        }
        _ => {}
    });
    if !effect {
        for (list, counter, offset, index) in reads {
            let span = spans.entry((list, counter)).or_insert_with(|| Span {
                list,
                counter,
                low: offset,
                high: offset,
                indices: Vec::new(),
            });
            span.low = span.low.min(offset);
            span.high = span.high.max(offset);
            span.indices.push(index);
        }
    }
    forget_writes(stmt, locals);
    if let Some((dst, Some(value))) = assignment {
        locals.insert(dst, value);
    }
}

/// Follow values only on paths dominated by a successful pull. A guarded
/// counter advances by one at most once per pull. Rebinding, nested control
/// flow or an unrecognized expression invalidates the corresponding copy.
pub(crate) fn span(suite: &[Statement], cursor: usize) -> Option<Span> {
    let [Statement::While(_, body, _)] = suite else {
        return None;
    };
    let ready = checked_pull_flag(body, cursor)?;
    let mut roots = Origins::new();
    visit_single_pull_writes(body, ready, |slot, rhs| {
        let initial = Origins::from([(slot, (slot, 0))]);
        if origin(rhs, &initial) == Some((slot, 1)) {
            roots.insert(slot, (slot, 0));
        }
    });
    if roots.is_empty() {
        return None;
    }
    let mut spans = BTreeMap::new();
    let mut pulled = false;
    let mut after_success = false;
    for stmt in body {
        if !pulled {
            ir::walk_statements(std::slice::from_ref(stmt), &mut |s| {
                if matches!(s,Statement::IterNext{iterator,..} if *iterator==cursor) {
                    pulled = true;
                }
            });
            forget_writes(stmt, &mut roots);
        } else if after_success {
            accept(stmt, &mut roots, &mut spans);
        } else if let Statement::If(test, yes, no) = stmt {
            match &test.kind {
                ExprKind::Local(flag) if *flag == ready => {
                    for s in yes {
                        accept(s, &mut roots, &mut spans);
                    }
                }
                ExprKind::Not(x)
                    if matches!(x.kind,ExprKind::Local(flag) if flag==ready)
                        && matches!(
                            yes.as_slice(),
                            [Statement::Exhaust | Statement::Break | Statement::Continue]
                        )
                        && no.is_empty() =>
                {
                    after_success = true;
                }
                _ => forget_writes(stmt, &mut roots),
            }
        } else {
            forget_writes(stmt, &mut roots);
        }
    }
    spans
        .into_values()
        .filter(|s| s.indices.len() >= 2)
        .max_by_key(|s| s.indices.len())
}
