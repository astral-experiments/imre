//! Keep one period of a scalar-struct producer whose ordinal is masked or reduced modulo a power of two.

use super::counted;
use crate::ir::{Binary, Builtin, Expr, ExprKind, Module, Statement, Type};
use crate::reductions::literal;
use crate::types::Types;
use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;
use std::collections::{HashMap, HashSet};

/// Replace a periodic producer by an ordinary prefix and a continuation. Running the
/// final iteration normally preserves every source local, including the induction variable.
pub(super) fn specialize(module: &mut Module) {
    if !module.source_frames.is_empty() {
        return;
    }
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        specialize_block(&mut function.body, &function.locals, &module.types);
    }
}

fn specialize_block(statements: &mut [Statement], locals: &[Type], types: &Types) {
    for statement in statements {
        let Some((limit, receiver, period)) = producer(statement, locals, types) else {
            crate::ir::statement_children_mut(statement, |child| {
                if let crate::ir::ChildMut::Suite(suite) = child {
                    specialize_block(suite, locals, types);
                }
            });
            continue;
        };
        let Statement::Scope { body, .. } = statement else {
            unreachable!()
        };
        let mut tail = body[0].clone();
        let Statement::For { args, .. } = &mut body[0] else {
            unreachable!()
        };
        let within = Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(vec![limit.clone(), Expr::integer(period)], vec![CmpOp::Lt]),
        };
        *args = vec![Expr {
            ty: Type::Int,
            kind: ExprKind::Conditional(
                Box::new(within),
                Box::new(limit.clone()),
                Box::new(Expr::integer(period)),
            ),
        }];
        let Statement::For { args, .. } = &mut tail else {
            unreachable!()
        };
        let resume = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(
                Builtin::StructCyclePrefix,
                vec![receiver, Expr::integer(period), limit.clone()],
            ),
        };
        *args = vec![resume, limit];
        body.push(tail);
    }
}

/// Prove that each assignment and every emitted field repeats after the same number of iterations.
fn producer(statement: &Statement, locals: &[Type], types: &Types) -> Option<(Expr, Expr, i64)> {
    let (limit, ordinal, body) = counted(statement)?;
    let (last, assignments) = body.split_last()?;
    let Statement::Expr(Expr {
        kind: ExprKind::Builtin(Builtin::Append, args),
        ..
    }) = last
    else {
        return None;
    };
    let [receiver, item] = args.as_slice() else {
        return None;
    };
    let ExprKind::Local(list) = receiver.kind else {
        return None;
    };
    let ExprKind::Construct { args, .. } = &item.kind else {
        return None;
    };
    if !types.is_value_struct(item.ty) || matches!(limit.kind, ExprKind::Local(s) if s == ordinal) {
        return None;
    }
    let mut written = HashSet::new();
    for assignment in assignments {
        let Statement::Assign(slot, value) = assignment else {
            return None;
        };
        if *slot == ordinal
            || *slot == list
            || locals[*slot] != value.ty
            || !scalar(value.ty)
            || matches!(limit.kind, ExprKind::Local(s) if s == *slot)
        {
            return None;
        }
        written.insert(*slot);
    }
    let mut known = HashMap::new();
    let mut period = 1;
    for assignment in assignments {
        let Statement::Assign(slot, value) = assignment else {
            unreachable!()
        };
        let p = expression_period(value, ordinal, &written, &known)?;
        known.insert(*slot, p);
        period = period.max(p);
    }
    for value in &args.values {
        period = period.max(expression_period(value, ordinal, &written, &known)?);
    }
    (period > 1).then(|| (limit.clone(), receiver.clone(), period))
}

fn scalar(ty: Type) -> bool {
    matches!(ty, Type::Int | Type::Float | Type::Bool)
}

/// Invariant operands have period one. All admitted periods are powers of two, so
/// taking their maximum preserves the complete sequence of scalar operand values.
fn expression_period(
    value: &Expr,
    ordinal: usize,
    written: &HashSet<usize>,
    known: &HashMap<usize, i64>,
) -> Option<i64> {
    if !scalar(value.ty) {
        return None;
    }
    match &value.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => Some(1),
        ExprKind::Local(slot) if *slot != ordinal => {
            if written.contains(slot) {
                known.get(slot).copied()
            } else {
                Some(1)
            }
        }
        ExprKind::Binary(op @ (Binary::BitAnd | Binary::Modulo), left, right) if matches!(left.kind, ExprKind::Local(s) if s == ordinal) =>
        {
            let n = u64::try_from(literal(right)?).ok()?;
            let period = match op {
                Binary::BitAnd => n.checked_add(1)?.checked_next_power_of_two()?,
                Binary::Modulo if n.is_power_of_two() => n,
                _ => return None,
            };
            i64::try_from(period).ok().filter(|&p| p <= i64::MAX / 2)
        }
        ExprKind::Binary(
            Binary::Add
            | Binary::Subtract
            | Binary::Multiply
            | Binary::BitAnd
            | Binary::BitOr
            | Binary::BitXor,
            a,
            b,
        ) => Some(
            expression_period(a, ordinal, written, known)?
                .max(expression_period(b, ordinal, written, known)?),
        ),
        ExprKind::Unary(UnaryOp::USub, x) => expression_period(x, ordinal, written, known),
        ExprKind::Builtin(Builtin::Float(_), args) if args.len() == 1 => {
            expression_period(&args[0], ordinal, written, known)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn proves_each_scalar_repeats_without_loop_carried_dependencies() {
        let source = r#"
from imre import struct
@struct
class Pair:
    x: float
    y: float
def build(n: int) -> list[Pair]:
    rows: list[Pair] = []
    x = 0.0
    if n > 0:
        for i in range(n):
            x = float(i & 5)
            rows.append(Pair(x, float(i % 16)))
    return rows
print(len(build(int(input()))))
"#;
        let emits_cycle = |source: &str| {
            compile(source, &CompileOptions::default())
                .unwrap()
                .contains("call i64 @imre_struct_cycle_prefix(")
        };
        assert!(emits_cycle(source));
        assert!(!emits_cycle(&source.replace("float(i % 16)", "float(i)")));
        assert!(!emits_cycle(
            &source.replace("x = float(i & 5)", "x = x + float(i & 5)")
        ));
        assert!(!emits_cycle(
            &source.replace("float(i % 16)", "float(i % 7)")
        ));
        assert!(!emits_cycle(&source.replace(
            "x = float(i & 5)",
            "print(i)\n            x = float(i & 5)"
        )));
    }
}
