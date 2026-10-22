//! Borrow a selected list field while the caller still owns its record.

use crate::ir::{Builtin, Compare, Expr, ExprKind, Function, Statement, Type};

pub(crate) struct Getter {
    pub root: usize,
    pub test: Expr,
    pub yes: usize,
    pub no: usize,
}

/// A single return choosing two concrete fields of one incoming record cannot
/// allocate a list or transfer its ownership. Only plain local arguments and
/// unboxed constants can be substituted without changing argument effects.
pub(crate) fn selected(call: &Expr, functions: &[Function]) -> Option<Getter> {
    let ExprKind::Call(id, args) = &call.kind else {
        return None;
    };
    if !matches!(call.ty, Type::ListInt | Type::List(_)) || !args.values.iter().all(atom) {
        return None;
    }
    let function = &functions[*id];
    let [Statement::Return(value)] = function.body.as_slice() else {
        return None;
    };
    let ExprKind::Conditional(test, yes, no) = &value.kind else {
        return None;
    };
    let (ExprKind::Field(a, yes), ExprKind::Field(b, no)) = (&yes.kind, &no.kind) else {
        return None;
    };
    let (ExprKind::Local(a), ExprKind::Local(b)) = (&a.kind, &b.kind) else {
        return None;
    };
    if a != b {
        return None;
    }
    let argument = |slot| {
        let pos = function.params.iter().position(|&p| p == slot)?;
        args.values.get(*args.order.get(pos)?)
    };
    let ExprKind::Local(root) = &argument(*a)?.kind else {
        return None;
    };
    let apply = |v: &Expr| {
        if let ExprKind::Local(slot) = v.kind {
            argument(slot).cloned()
        } else {
            atom(v).then(|| v.clone())
        }
    };
    let test = match &test.kind {
        ExprKind::Local(_) if test.ty == Type::Bool => apply(test)?,
        ExprKind::Compare(values, ops) => {
            let ([a, b], [Compare::Equal | Compare::NotEqual]) =
                (values.as_slice(), ops.as_slice())
            else {
                return None;
            };
            if a.ty != b.ty || !matches!(a.ty, Type::Bool | Type::StrEnum(_)) {
                return None;
            }
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Compare(vec![apply(a)?, apply(b)?], ops.clone()),
            }
        }
        _ => return None,
    };
    Some(Getter {
        root: *root,
        test,
        yes: *yes,
        no: *no,
    })
}

fn atom(value: &Expr) -> bool {
    matches!(
        value.kind,
        ExprKind::Local(_)
            | ExprKind::Bool(_)
            | ExprKind::Builtin(Builtin::StrEnumMember { .. }, _)
    )
}
