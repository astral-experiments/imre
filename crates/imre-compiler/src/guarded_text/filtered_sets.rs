//! Avoid building an empty set when its only observation would be a truth test.

use crate::ir::{
    self, Builtin, ChildMut, Expr, ExprKind, Statement, Type, walk_expressions,
    walk_expressions_mut,
};
use crate::types::Types;

/// Changes a private set local to an optional set. Every read after the guard
/// unwraps an actual owned set, so a nonempty result can still escape or mutate.
pub(super) fn specialize(
    body: &mut [Statement],
    locals: &mut [Type],
    types: &Types,
    reads: &[usize],
    writes: &[usize],
) {
    for statement in &mut *body {
        ir::statement_children_mut(statement, |child| {
            if let ChildMut::Suite(body) = child {
                specialize(body, locals, types, reads, writes);
            }
        });
    }
    for index in 1..body.len() {
        let [Statement::Assign(slot, filter), Statement::If(test, yes, _)] =
            &mut body[index - 1..=index]
        else {
            continue;
        };
        let slot = *slot;
        let ExprKind::Builtin(Builtin::Bool, arguments) = &test.kind else {
            continue;
        };
        if writes[slot] != 1
            || !matches!(arguments.as_slice(), [Expr { kind: ExprKind::Local(guard), .. }] if *guard == slot)
            || !matches!(filter.ty, Type::Set(_))
        {
            continue;
        }
        let mut guarded_reads = 0;
        walk_expressions(yes, &mut |expr| {
            guarded_reads +=
                usize::from(matches!(expr.kind, ExprKind::Local(read) if read == slot));
        });
        if reads[slot] != guarded_reads + 1 {
            continue;
        }
        let set = filter.ty;
        let optional = types.optional(set);
        if !omit_empty(filter, optional) {
            continue;
        }
        locals[slot] = optional;
        let absent = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::IsNone, vec![Expr::local(slot, optional)]),
        };
        test.kind = ExprKind::Not(Box::new(absent));
        walk_expressions_mut(yes, &mut |expr| {
            if expr.ty == set && matches!(expr.kind, ExprKind::Local(read) if read == slot) {
                *expr = Expr {
                    ty: set,
                    kind: ExprKind::Unwrap(Box::new(Expr::local(slot, optional))),
                };
            }
        });
    }
}

/// Preserves the empty-input branch, where Python skips the membership operand.
fn omit_empty(filter: &mut Expr, optional: Type) -> bool {
    let ExprKind::Scope { value, .. } = &mut filter.kind else {
        return false;
    };
    let ExprKind::Conditional(_, selected, empty) = &mut value.kind else {
        return false;
    };
    if !matches!(
        selected.kind,
        ExprKind::Builtin(Builtin::FilterCharacterSet(_), _)
    ) || !matches!(&empty.kind, ExprKind::Set { values, .. } if values.is_empty())
    {
        return false;
    }
    filter.ty = optional;
    value.ty = optional;
    selected.ty = optional;
    **empty = Expr::absent(optional);
    true
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn only_omit_an_empty_set_if_the_guard_owns_every_read() {
        for (tail, omit) in [
            (
                "if found:\n        found.add('x')\n        return len(found)\n    return 0",
                true,
            ),
            ("if found:\n        return 1\n    return len(found)", false),
            (
                "if found:\n        return 1\n    else:\n        found.add('x')\n    return 0",
                false,
            ),
            (
                "alias = found\n    if found:\n        return 1\n    return len(alias)",
                false,
            ),
            (
                "if found:\n        found = {'y'}\n        return len(found)\n    return 0",
                false,
            ),
        ] {
            let source = format!(
                "def select(text: str, keys: set[str]) -> int:\n    found = {{c for c in text if c in keys}}\n    {tail}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @imre_filter_nonempty_character_set("),
                omit,
                "{tail}"
            );
            assert_eq!(
                llvm.contains("call ptr @imre_filter_character_set("),
                !omit,
                "{tail}"
            );
        }
    }
}
