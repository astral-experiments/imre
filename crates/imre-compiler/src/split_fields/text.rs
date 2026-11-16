//! Keep the source and byte spans when a temporary split list is only counted and indexed.
use super::*;
use crate::ir::{Function, rewrite_statements};

fn projection(value: &Expr) -> Option<(usize, Option<u8>)> {
    let ExprKind::Builtin(op, args) = &value.kind else {
        return None;
    };
    let (source, index) = match (op, args.as_slice()) {
        (Builtin::Len, [source]) => (source, None),
        (
            Builtin::ListGet(_),
            [
                source,
                Expr {
                    kind: ExprKind::Int(n),
                    ..
                },
            ],
        ) if value.ty == Type::Str => {
            let index = n.parse::<u8>().ok()?;
            if index >= 8 {
                return None;
            }
            (source, Some(index))
        }
        _ => return None,
    };
    let ExprKind::Local(slot) = source.kind else {
        return None;
    };
    Some((slot, index))
}

/// A second read of a field could observe its identity. A loop could repeat a single read.
fn uses(following: &[Statement], slot: usize) -> (usize, u8) {
    let mut seen = 0_u8;
    let mut count = 0;
    for stmt in following {
        match stmt {
            Statement::Assign(target, _) if *target != slot => {}
            Statement::Return(_) | Statement::Expr(_) => {}
            Statement::If(_, yes, no)
                if matches!(yes.as_slice(), [Statement::Raise(_)]) && no.is_empty() => {}
            _ => break,
        }
        let mut safe = true;
        let mut found = 0;
        walk_expressions(std::slice::from_ref(stmt), &mut |value| {
            safe &= !matches!(value.kind, ExprKind::Scope { .. });
            if let Some((read, index)) = projection(value)
                && read == slot
            {
                if let Some(index) = index {
                    let bit = 1 << index;
                    safe &= seen & bit == 0;
                    seen |= bit;
                }
                found += 1;
            }
        });
        if !safe {
            break;
        }
        count += found;
    }
    (count, seen)
}

/// Require every write to produce a split and every use to follow its own producer.
pub(super) fn specialize(f: &mut Function, reads: &[usize], writes: &[usize]) {
    #[derive(Clone, Default)]
    struct Uses {
        reads: usize,
        writes: usize,
        mask: u8,
    }
    let mut uses_by_slot = vec![Uses::default(); reads.len()];
    super::capture(&mut f.body, &mut |slot, following| {
        let (count, mask) = uses(following, slot);
        let entry = &mut uses_by_slot[slot];
        entry.writes += 1;
        entry.reads += count;
        entry.mask |= mask;
        None
    });
    for (slot, entry) in uses_by_slot.iter_mut().enumerate() {
        if entry.writes != writes[slot] || entry.reads != reads[slot] {
            entry.mask = 0;
        }
    }
    if !uses_by_slot.iter().any(|entry| entry.mask != 0) {
        return;
    }
    rewrite_statements(&mut f.body, &mut |stmt| {
        if let Statement::Assign(slot, value) = stmt
            && uses_by_slot[*slot].mask != 0
            && split(value)
            && let ExprKind::Builtin(_, args) = &mut value.kind
        {
            value.ty = Type::Str;
            value.kind = ExprKind::Builtin(
                Builtin::SplitTextSource(8 - uses_by_slot[*slot].mask.leading_zeros() as u8),
                std::mem::take(args),
            );
            f.locals[*slot] = Type::Str;
        }
    });
    walk_expressions_mut(&mut f.body, &mut |value| {
        if let Some((slot, index)) = projection(value)
            && uses_by_slot[slot].mask != 0
        {
            let op = match (index, &mut value.kind) {
                (None, _) => Builtin::SplitTextLen,
                (Some(index), ExprKind::Builtin(Builtin::ListGet(origin), _)) => {
                    Builtin::SplitTextField(index, origin.take())
                }
                _ => unreachable!("indexed projections use a checked list read"),
            };
            value.kind = ExprKind::Builtin(op, vec![Expr::local(slot, Type::Str)]);
        }
    });
}
