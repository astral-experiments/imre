//! Elide unobserved split lists when their only consumers parse fixed fields.

use crate::ir::{
    Builtin, ChildMut, Expr, ExprKind, Module, Statement, Type, statement_children_mut,
    walk_expressions, walk_expressions_mut,
};
use crate::reductions::count_writes;

/// Keep the exact input string in a private local and parse each projection at
/// its original use. A skipped or reordered read must still see the right error.
pub(crate) fn specialize(module: &mut Module) {
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        let mut splits = false;
        let mut reads = vec![0; function.locals.len()];
        walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Local(slot) = value.kind {
                reads[slot] += 1;
            }
            splits |= split(value);
        });
        if !splits {
            continue;
        }
        let mut writes = vec![0; reads.len()];
        count_writes(&function.body, &mut writes);
        for &slot in &function.params {
            writes[slot] += 1;
        }
        let mut masks = vec![0u8; reads.len()];
        capture(&mut function.body, &mut |slot, following| {
            if writes[slot] != 1 || reads[slot] == 0 {
                return None;
            }
            let (count, mask) = nearby_fields(following, slot);
            if count != reads[slot] {
                return None;
            }
            masks[slot] = mask;
            function.locals[slot] = Type::Str;
            Some(mask)
        });
        walk_expressions_mut(&mut function.body, &mut |value| {
            let Some((source, index)) = field(value) else {
                return;
            };
            let mut op = Builtin::SplitIntField(index);
            let input = match &source.kind {
                ExprKind::Local(slot) if masks[*slot] != 0 => {
                    if masks[*slot].count_ones() >= 2 {
                        op = Builtin::SplitIntCached(index);
                    }
                    Expr::local(*slot, Type::Str)
                }
                ExprKind::Builtin(Builtin::Split, args) if split(source) => args[0].clone(),
                _ => return,
            };
            value.kind = ExprKind::Builtin(op, vec![input]);
        });
    }
}

fn split(value: &Expr) -> bool {
    matches!(&value.kind, ExprKind::Builtin(Builtin::Split, inputs)
        if matches!(inputs.as_slice(), [input] if input.ty == Type::Str))
}

fn field(value: &Expr) -> Option<(&Expr, u32)> {
    let ExprKind::Builtin(Builtin::Int, args) = &value.kind else {
        return None;
    };
    let [arg] = args.as_slice() else { return None };
    let ExprKind::Builtin(Builtin::ListGet, args) = &arg.kind else {
        return None;
    };
    let [source, index] = args.as_slice() else {
        return None;
    };
    let ExprKind::Int(literal) = &index.kind else {
        return None;
    };
    let index = literal.parse().ok()?;
    (arg.ty == Type::Str).then_some((source, index))
}

/// A retained split list can be cheaper than rescanning a string across a loop.
fn nearby_fields(body: &[Statement], slot: usize) -> (usize, u8) {
    let mut count = 0;
    let mut mask = 0;
    for statement in body.iter().take(8) {
        if !matches!(
            statement,
            Statement::Assign(..) | Statement::Expr(_) | Statement::Return(_)
        ) {
            break;
        }
        let mut found = 0;
        let mut fields = 0;
        let mut scoped = false;
        walk_expressions(std::slice::from_ref(statement), &mut |value| {
            scoped |= matches!(value.kind, ExprKind::Scope { .. });
            if let Some((source, index)) = field(value)
                && index < 8
                && matches!(source.kind, ExprKind::Local(read) if read == slot)
            {
                found += 1;
                fields |= 1 << index;
            }
        });
        if scoped {
            break;
        }
        count += found;
        mask |= fields;
    }
    (count, mask)
}

fn capture(body: &mut [Statement], private: &mut impl FnMut(usize, &[Statement]) -> Option<u8>) {
    let mut remaining = body;
    while let Some((statement, rest)) = remaining.split_first_mut() {
        remaining = rest;
        if let Statement::Assign(slot, value) = statement
            && split(value)
            && let Some(mask) = private(*slot, remaining)
            && let ExprKind::Builtin(_, inputs) = &mut value.kind
        {
            let source = inputs.pop().unwrap();
            *value = if mask.count_ones() >= 2 {
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::SplitIntSource(mask), vec![source]),
                }
            } else {
                source
            };
        } else {
            statement_children_mut(statement, |child| {
                if let ChildMut::Suite(body) = child {
                    capture(body, private);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count actual typed operations rather than their always-declared ABI symbols.
    fn operations(body: &str) -> (usize, usize, usize) {
        let graph = crate::modules::SourceGraph::single(body).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        specialize(&mut module);
        let mut counts = (0, 0, 0);
        for function in module.functions.iter().chain([&module.entry]) {
            walk_expressions(&function.body, &mut |value| match value.kind {
                ExprKind::Builtin(Builtin::Split, _) => counts.0 += 1,
                ExprKind::Builtin(Builtin::SplitIntField(_), _) => counts.1 += 1,
                ExprKind::Builtin(Builtin::SplitIntCached(_), _) => counts.2 += 1,
                _ => {}
            });
        }
        counts
    }

    #[test]
    fn replaces_only_the_private_straight_line_projections() {
        assert_eq!(
            operations(
                "def fields(s: str) -> int:\n    f = s.split()\n    return int(f[2]) - int(f[1])\n"
            ),
            (0, 0, 2)
        );
        assert_eq!(
            operations("def one(s: str) -> int:\n    return int(s.split()[3])\n"),
            (0, 1, 0)
        );
        for body in [
            "f = s.split()\n    a = f\n    a[0] = '9'\n    return int(f[0])",
            "f = s.split()\n    f = s.split(',')\n    return int(f[0])",
            "f = s.split()\n    n = 0\n    for i in range(3):\n        n += int(f[0])\n    return n",
            "f = s.split()\n    return int(f[100])",
            "f = s.split()\n    return int(f[-1])",
        ] {
            let (splits, direct, cached) =
                operations(&format!("def f1(s: str) -> int:\n    {body}\n"));
            assert!(splits > 0, "{body}");
            assert_eq!((direct, cached), (0, 0), "{body}");
        }
    }
}
