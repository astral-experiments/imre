//! Container displays in registered Python frames.
use super::permanent;
use crate::ir::{Builtin, Expr, ExprKind};

pub(in crate::llvm) fn permanent_handle(value: &Expr) -> Option<u16> {
    let handle = match &value.without_location().kind {
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
