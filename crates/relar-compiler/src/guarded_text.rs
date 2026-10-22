//! Delay a total string operation past loop guards while eagerly owning its input.

use crate::ir::{
    Builtin, Expr, ExprKind, Module, Statement, TextOperation, Type, walk_expressions,
};
use crate::reductions::{contains_continue, count_writes};

pub(crate) fn specialize(module: &mut Module) {
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        let mut reads = vec![0; function.locals.len()];
        let mut uses_unquote = false;
        walk_expressions(&function.body, &mut |expr| match expr.kind {
            ExprKind::Local(slot) => reads[slot] += 1,
            ExprKind::Builtin(Builtin::Text(TextOperation::Unquote), _) => uses_unquote = true,
            _ => {}
        });
        if !uses_unquote {
            continue;
        }
        let mut writes = vec![0; reads.len()];
        count_writes(&function.body, &mut writes);
        for &slot in &function.params {
            writes[slot] += 1;
        }
        let private: Vec<_> = reads
            .iter()
            .zip(&writes)
            .map(|(&r, &w)| r == 1 && w == 1)
            .collect();
        block(&mut function.body, &private, false);
    }
}

fn block(body: &mut Vec<Statement>, private: &[bool], in_loop: bool) {
    for statement in &mut *body {
        match statement {
            Statement::Scope { body, .. } => block(body, private, in_loop),
            Statement::If(_, yes, no) => {
                block(yes, private, in_loop);
                block(no, private, in_loop);
            }
            Statement::For { body, orelse, .. } | Statement::While(_, body, orelse) => {
                block(body, private, true);
                block(orelse, private, in_loop);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                block(body, private, in_loop);
                for handler in handlers {
                    block(&mut handler.body, private, in_loop);
                }
                block(orelse, private, in_loop);
                block(finalbody, private, in_loop);
            }
            _ => {}
        }
    }
    if !in_loop || !contains_continue(body) {
        return;
    }
    // Replacing a region leaves the indices of earlier statements unchanged.
    for index in (0..body.len()).rev() {
        let start = index + 1;
        let Statement::Assign(slot, expression) = &mut body[index] else {
            continue;
        };
        let slot = *slot;
        if !private[slot] || source(expression).is_none() {
            continue;
        }
        let Some(distance) = body[start..].iter().position(|statement| {
            let mut found = false;
            walk_expressions(std::slice::from_ref(statement), &mut |value| {
                found |= matches!(value.kind, ExprKind::Local(id) if id == slot);
            });
            found
        }) else {
            continue;
        };
        let use_at = start + distance;
        if !contains_continue(&body[start..use_at]) {
            continue;
        }
        // The input can be a fallible call or a mutable field read. Snapshot it
        // at its original evaluation point. No other read, write, move, or
        // walrus can observe our private slot between this assignment and use.
        let mut guarded: Vec<_> = body.drain(index..=use_at).collect();
        let Statement::Assign(_, expression) = &mut guarded[0] else {
            unreachable!()
        };
        let input = std::mem::replace(
            source(expression).unwrap(),
            Expr {
                ty: Type::Str,
                kind: ExprKind::Local(slot),
            },
        );
        let projection = std::mem::replace(expression, input);
        guarded.insert(guarded.len() - 1, Statement::Assign(slot, projection));
        // Drop the snapshot at early exits and the owned result after its sole
        // use, including when a use retains it in an escaping record or list.
        body.insert(
            index,
            Statement::Scope {
                body: guarded,
                clear: vec![slot],
            },
        );
    }
}

/// Default UTF-8 percent decoding is total on a string. A fused split can join
/// it past the guard only if the separator is a nonempty literal.
fn source(expression: &mut Expr) -> Option<&mut Expr> {
    let ExprKind::Builtin(Builtin::Text(TextOperation::Unquote), values) = &mut expression.kind
    else {
        return None;
    };
    let [argument] = values.as_mut_slice() else {
        return None;
    };
    let values = match argument {
        Expr {
            kind: ExprKind::Builtin(Builtin::ListGet, values),
            ..
        } => values,
        input => return Some(input),
    };
    let [split, index] = values.as_mut_slice() else {
        return None;
    };
    let args = split.last_rsplit_args(index)?;
    if !matches!(&args[1].kind, ExprKind::Present(s) if matches!(&s.kind, ExprKind::Str(v) if !v.is_empty()))
    {
        return None;
    }
    let ExprKind::Builtin(_, args) = &mut split.kind else {
        unreachable!()
    };
    Some(&mut args[0])
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn only_total_single_use_projections_cross_a_guard() {
        for (separator, skipped, delayed) in [
            ("'/'", "continue", true),
            ("sep", "continue", false),
            ("''", "continue", false),
            ("'/'", "print(name)\n            continue", false),
            ("'/'", "name = 'rebound'\n            continue", false),
        ] {
            let python = format!(
                "from urllib.parse import unquote\ndef names(items: list[str], sep: str) -> None:\n    for item in items:\n        name = unquote(item.rsplit({separator}, 1)[-1])\n        print('guard')\n        if not item:\n            {skipped}\n        print(name)\n"
            );
            let llvm = compile(&python, &CompileOptions::default()).unwrap();
            let decode = llvm
                .find("call ptr @relar_str_unquote_rsplit_last(")
                .unwrap();
            let guard = llvm.find("call void @relar_print_str(").unwrap();
            assert_eq!(decode > guard, delayed, "{python}");
        }
    }
}
