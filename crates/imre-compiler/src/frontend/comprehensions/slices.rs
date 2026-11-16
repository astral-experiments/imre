//! Removes a private reverse slice when an integer reduction cannot mutate it.

use crate::ir::{Binary, Builtin, Expr, ExprKind, Statement, Type};
use ruff_python_ast::CmpOp;

/// Read a private reverse slice in place when the reduction cannot mutate its source.
pub(super) fn fuse(body: &mut [Statement]) {
    let [
        Statement::Assign(_, _),
        Statement::Scope {
            body: iteration, ..
        },
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
    let ExprKind::Builtin(Builtin::Slice, parts) = &mut input.kind else {
        return;
    };
    let Some((_, start, step)) = crate::ir::reverse_slice(parts) else {
        return;
    };
    if input.ty != Type::ListInt
        || !matches!(&initial.kind, ExprKind::Int(value) if value == "0")
        || !orelse.is_empty()
        || !test.tests_index_before_len(*captured, *cursor)
        || !crate::list_bounds::block_readonly(loop_body)
    {
        return;
    }
    let Some(increment) = loop_body.iter_mut().find_map(|statement| {
        if let Statement::Assign(slot, value) = statement
            && slot == cursor
            && value.local_integer_offset() == Some((*cursor, 1))
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
    *increment = add(local(*cursor), Expr::integer(step));
    *initial = add(
        Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![list_local(*captured)]),
        },
        Expr::integer(start),
    );
    *test = Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(vec![local(*cursor), Expr::integer(0)], vec![CmpOp::GtE]),
    };
    *input = parts.swap_remove(0);
}

fn local(slot: usize) -> Expr {
    Expr::local(slot, Type::Int)
}

fn list_local(slot: usize) -> Expr {
    Expr::local(slot, Type::ListInt)
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
