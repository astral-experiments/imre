//! Container displays in registered Python frames.
use super::permanent;
use crate::ir::{Builtin, Expr, ExprKind};

fn literal(value: &Expr) -> &ExprKind {
    match &value.kind {
        ExprKind::Builtin(Builtin::RegisteredLocation(..), args) => literal(&args[0]),
        kind => kind,
    }
}

pub(super) fn spread(item: &Expr) -> Option<&Expr> {
    match &item.kind {
        ExprKind::Builtin(Builtin::RegisteredSpread, operands) => Some(&operands[0]),
        _ => None,
    }
}

pub(super) fn permanent_handle(value: &Expr) -> Option<u16> {
    let handle = match literal(value) {
        ExprKind::None => permanent::NONE,
        ExprKind::Bool(true) => permanent::TRUE,
        ExprKind::Bool(false) => permanent::FALSE,
        ExprKind::Int(n) => permanent::small_integer_id(n.parse().ok()?)?,
        ExprKind::Builtin(Builtin::RegisteredEllipsis, _) => permanent::ELLIPSIS,
        ExprKind::Tuple(values) if values.is_empty() => permanent::EMPTY_TUPLE,
        _ => return None,
    };
    u16::try_from(handle).ok()
}
