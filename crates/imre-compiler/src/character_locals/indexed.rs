//! Strength-reduce indexed ordinal reads in a complete forward traversal of an immutable string.

use super::{Builtin, Expr, ExprKind, IterNextKind, Statement, Type, ir};

/// Return whether this expression reads the ordinal at the current loop index.
fn indexed_ordinal(value: &Expr, text: usize, target: usize) -> bool {
    let ExprKind::Builtin(Builtin::Ord, args) = &value.kind else {
        return false;
    };
    let [
        Expr {
            kind: ExprKind::Builtin(Builtin::StringGet, get),
            ..
        },
    ] = args.as_slice()
    else {
        return false;
    };
    let [source, index] = get.as_slice() else {
        return false;
    };
    source.ty == Type::Str
        && matches!(source.kind, ExprKind::Local(slot) if slot == text)
        && matches!(index.kind, ExprKind::Local(slot) if slot == target)
}

/// Look through eager arithmetic only. Sparse or short-circuited reads need no cursor.
fn eager_ordinal(value: &Expr, text: usize, target: usize) -> bool {
    match &value.kind {
        ExprKind::Binary(_, left, right) => {
            eager_ordinal(left, text, target) || eager_ordinal(right, text, target)
        }
        _ => indexed_ordinal(value, text, target),
    }
}

/// The original range still assigns its visible target and handles break, continue and else.
pub(super) fn specialize(statement: &mut Statement, locals: &mut Vec<Type>) {
    let Statement::For {
        target, args, body, ..
    } = statement
    else {
        return;
    };
    let bound = match args.as_slice() {
        [bound] => bound,
        [start, bound] if start.tagged_integer_literal() == Some(0) => bound,
        [start, bound, step]
            if start.tagged_integer_literal() == Some(0)
                && step.tagged_integer_literal() == Some(1) =>
        {
            bound
        }
        _ => return,
    };
    let ExprKind::Builtin(Builtin::Len, items) = &bound.kind else {
        return;
    };
    let [
        Expr {
            ty: Type::Str,
            kind: ExprKind::Local(text),
        },
    ] = items.as_slice()
    else {
        return;
    };
    let (text, target) = (*text, *target);
    let eager = body
        .iter()
        .map_while(|stmt| match stmt {
            Statement::Assign(_, value) | Statement::Expr(value) => Some(value),
            _ => None,
        })
        .any(|value| eager_ordinal(value, text, target));
    if !eager {
        return;
    }
    let mut rebound = false;
    ir::walk_writes(body, |slot, _| rebound |= slot == text || slot == target);
    if rebound {
        return;
    }
    let iterator = locals.len();
    let code = iterator + 1;
    let ready = iterator + 2;
    locals.extend([Type::Iterator, Type::Int, Type::Bool]);
    ir::walk_expressions_mut(body, &mut |value| {
        if indexed_ordinal(value, text, target) {
            *value = Expr::local(code, Type::Int);
        }
    });
    // The unmodified string supplies exactly as many code points as the range has indices.
    body.insert(
        0,
        Statement::IterNext {
            kind: IterNextKind::CodePoint,
            iterator,
            key: code,
            value: None,
            ready,
        },
    );
    let range = std::mem::replace(statement, Statement::Clear(iterator));
    *statement = Statement::Scope {
        body: vec![
            Statement::Assign(
                iterator,
                Expr {
                    ty: Type::Iterator,
                    kind: ExprKind::Builtin(
                        Builtin::StringIter,
                        vec![Expr::local(text, Type::Str)],
                    ),
                },
            ),
            range,
        ],
        clear: vec![iterator, code, ready],
    };
}
