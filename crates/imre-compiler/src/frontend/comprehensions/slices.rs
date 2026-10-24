//! Removes a private reverse slice when an integer reduction cannot mutate it.

use crate::ir::{Binary, Builtin, Compare, Expr, ExprKind, Statement, Type};

/// A helper's first iterable has no Python-visible identity. For a readonly
/// integer reduction, reading the original list in the same selected order
/// preserves the slice snapshot. Literal bounds cannot raise or call user code,
/// and all remaining captures are local reads. Keep every other slice intact.
pub(super) fn fuse(body: &mut [Statement], params: &[usize], args: &mut [Expr]) {
    let Some(&parameter) = params.first() else {
        return;
    };
    let Some(argument) = args.first() else {
        return;
    };
    if argument.ty != Type::ListInt
        || !args[1..]
            .iter()
            .all(|value| matches!(value.kind, ExprKind::Local(_)))
        || !crate::list_bounds::block_readonly(body)
    {
        return;
    }
    let ExprKind::Builtin(Builtin::Slice, parts) = &argument.kind else {
        return;
    };
    let [source, start, stop, step] = parts.as_slice() else {
        return;
    };
    if !omitted(stop) {
        return;
    }
    let start = if omitted(start) {
        -1
    } else if let Some(start) = negative_component(start) {
        start
    } else {
        return;
    };
    let Some(step) = negative_component(step) else {
        return;
    };
    let source = source.clone();
    let [
        Statement::Assign(_, _),
        Statement::Scope {
            body: iteration, ..
        },
        Statement::Return(_),
    ] = body
    else {
        return;
    };
    let [
        Statement::Assign(captured, input),
        Statement::Assign(cursor, initial),
        Statement::While(test, loop_body, orelse),
    ] = iteration.as_mut_slice()
    else {
        return;
    };
    if !matches!(input.kind, ExprKind::Local(slot) if slot == parameter)
        || !matches!(&initial.kind, ExprKind::Int(value) if value == "0")
        || !orelse.is_empty()
        || !forward_condition(test, *captured, *cursor)
    {
        return;
    }
    let Some(increment) = loop_body.iter_mut().find_map(|statement| {
        if let Statement::Assign(slot, value) = statement
            && slot == cursor
            && let ExprKind::Binary(Binary::Add, left, right) = &value.kind
            && matches!(left.kind, ExprKind::Local(slot) if slot == *cursor)
            && matches!(&right.kind, ExprKind::Int(value) if value == "1")
        {
            Some(value)
        } else {
            None
        }
    }) else {
        return;
    };
    // These are private iteration slots created by lower_iteration. The body
    // cannot name or rebind them; the only cursor write is its own advance.
    *increment = add(local(*cursor), integer(step));
    *initial = add(
        Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![list_local(*captured)]),
        },
        integer(start),
    );
    *test = Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(
            vec![local(*cursor), integer(0)],
            vec![Compare::GreaterEqual],
        ),
    };
    args[0] = source;
}

fn omitted(value: &Expr) -> bool {
    matches!(&value.kind, ExprKind::Absent(inner) if matches!(inner.kind, ExprKind::None))
}

/// Optional slice components wrap their integer value in Present.
fn negative_component(value: &Expr) -> Option<i64> {
    let value = match &value.kind {
        ExprKind::Present(value) => value.as_ref(),
        _ => return None,
    };
    let value = match &value.kind {
        ExprKind::Int(value) => value.parse::<i64>().ok()?,
        ExprKind::Negate(value) => {
            let ExprKind::Int(value) = &value.kind else {
                return None;
            };
            value.parse::<i64>().ok()?.checked_neg()?
        }
        _ => return None,
    };
    (-(1_i64 << 62)..0).contains(&value).then_some(value)
}

fn forward_condition(value: &Expr, captured: usize, cursor: usize) -> bool {
    let ExprKind::Compare(values, operations) = &value.kind else {
        return false;
    };
    let [left, right] = values.as_slice() else {
        return false;
    };
    matches!(operations.as_slice(), [Compare::Less])
        && matches!(left.kind, ExprKind::Local(slot) if slot == cursor)
        && matches!(&right.kind, ExprKind::Builtin(Builtin::Len, args)
            if matches!(args.as_slice(), [Expr { kind: ExprKind::Local(slot), .. }] if *slot == captured))
}

fn integer(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

fn local(slot: usize) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Local(slot),
    }
}

fn list_local(slot: usize) -> Expr {
    Expr {
        ty: Type::ListInt,
        kind: ExprKind::Local(slot),
    }
}

fn add(left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Binary(Binary::Add, Box::new(left), Box::new(right)),
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn readonly_reverse_integer_reductions_do_not_materialize_the_slice() {
        for slice in [
            "::-1",
            "-2::-2",
            "-3::-7",
            "None:None:-1",
            "-4611686018427387904::-1",
            "::-4611686018427387904",
        ] {
            let source = format!(
                "def reduce(values: list[int]) -> int:\n    return sum([value * 3 for value in values[{slice}] if value != 2])\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert!(!llvm.contains("call ptr @imre_list_slice("), "{slice}");
        }
    }

    #[test]
    fn effects_dynamic_bounds_and_other_slice_shapes_keep_the_snapshot() {
        for source in [
            "def mutate(values: list[int], value: int) -> int:\n    values[0] = 99\n    return value\n\ndef reduce(values: list[int]) -> int:\n    return sum([mutate(values, value) for value in values[::-2]])\n",
            "def reduce(values: list[int], step: int) -> int:\n    return sum([value for value in values[::step]])\n",
            "def reduce(values: list[int]) -> int:\n    return sum([value for value in values[::2]])\n",
            "def reduce(values: list[int]) -> int:\n    return sum([value for value in values[:-1:-2]])\n",
            "def reduce(values: list[int]) -> int:\n    return sum([value for value in values[1::-2]])\n",
            "def reduce(values: list[int]) -> int:\n    return sum([value for value in values[::-0]])\n",
            "def reduce(values: list[int]) -> int:\n    return sum([value for value in values[::-18446744073709551616]])\n",
            "def reduce(values: list[int]) -> int:\n    last = 0\n    return sum([(last := value) for value in values[::-1]])\n",
        ] {
            let llvm = compile(source, &CompileOptions::default()).unwrap();
            assert!(llvm.contains("call ptr @imre_list_slice("), "{source}");
        }
    }
}
