//! Fuse the producer of a private decimal digit list with two alternating reductions.
//!
//! ```python
//! digits = list(map(int, text))
//! units = sum(digits[-1::-2])
//! weighted = sum([3 * d for d in digits[-2::-2]])
//! ```

use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Function, Module, Statement, Type};
use crate::reductions::{count_writes, literal};
use ruff_python_ast::CmpOp;

mod terms;

#[derive(Clone, Copy)]
struct Lane {
    /// The last digit belongs to lane zero, regardless of the string's length.
    phase: usize,
    table: [u8; 10],
}

/// Work on checked integer operations after fixed tuples have been scalarized.
/// Source frame inspection can observe the original comprehension call.
pub(crate) fn specialize(module: &mut Module) {
    if !module.source_frames.is_empty() {
        return;
    }
    let helpers: Vec<_> = module.functions.iter().map(mapped_helper).collect();
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        let mut reads = vec![0usize; function.locals.len()];
        let mut producer = false;
        ir::walk_expressions(&function.body, &mut |e| match e.kind {
            ExprKind::Local(id) => reads[id] += 1,
            ExprKind::Builtin(Builtin::StringIntList(_), _) => producer = true,
            _ => {}
        });
        if !producer {
            continue;
        }
        let mut writes = vec![0; reads.len()];
        count_writes(&function.body, &mut writes);
        for &param in &function.params {
            writes[param] += 1;
        }
        let private = |id: usize| reads[id] == 2 && writes[id] == 1;
        fuse(&mut function.body, &mut function.locals, &helpers, &private);
    }
}

/// Keep the original statements as the fallback. No Python-visible assignment
/// takes place until all bytes and both subtotals have passed the runtime guard.
fn fuse(
    body: &mut Vec<Statement>,
    locals: &mut Vec<Type>,
    helpers: &[Option<Lane>],
    private: &impl Fn(usize) -> bool,
) {
    for statement in &mut *body {
        ir::statement_children_mut(statement, |child| {
            if let ir::ChildMut::Suite(body) = child {
                fuse(body, locals, helpers, private);
            }
        });
    }
    for i in (0..body.len().saturating_sub(2)).rev() {
        let [
            Statement::Assign(list, producer),
            Statement::Assign(a, first),
            Statement::Assign(b, second),
            ..,
        ] = &body[i..]
        else {
            continue;
        };
        let ExprKind::Builtin(Builtin::StringIntList(_), args) = &producer.kind else {
            continue;
        };
        let [text] = args.as_slice() else { continue };
        if !private(*list) || a == b || !matches!(text.kind, ExprKind::Local(_)) {
            continue;
        }
        let (Some(x), Some(y)) = (lane(first, *list, helpers), lane(second, *list, helpers)) else {
            continue;
        };
        if x.phase == y.phase {
            continue;
        }
        let mut tables = [[0; 10]; 2];
        tables[x.phase] = x.table;
        tables[y.phase] = y.table;
        let pack = locals.len();
        locals.push(Type::Int);
        let attempt = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::TryDigitSums(tables), vec![text.clone()]),
        };
        let success = Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(vec![local(pack), Expr::integer(0)], vec![CmpOp::GtE]),
        };
        let unpack = |phase| {
            let (op, bits) = if phase == 0 {
                (Binary::BitAnd, i64::from(i32::MAX))
            } else {
                (Binary::RightShift, 31)
            };
            Expr {
                ty: Type::Int,
                kind: ExprKind::Binary(op, Box::new(local(pack)), Box::new(Expr::integer(bits))),
            }
        };
        let outputs = vec![
            Statement::Assign(*a, unpack(x.phase)),
            Statement::Assign(*b, unpack(y.phase)),
        ];
        // The list has no uses outside these two reductions. Only the fallback
        // materializes it, so release its owner in that same scope.
        let clear = vec![*list];
        let fallback = Statement::Scope {
            body: body.drain(i..i + 3).collect(),
            clear,
        };
        body.insert(
            i,
            Statement::Scope {
                body: vec![
                    Statement::Assign(pack, attempt),
                    Statement::If(success, outputs, vec![fallback]),
                ],
                clear: vec![pack],
            },
        );
    }
}

fn lane(value: &Expr, list: usize, helpers: &[Option<Lane>]) -> Option<Lane> {
    match &value.kind {
        ExprKind::Builtin(Builtin::ListSliceSum, args) => {
            let (source, start, step) = ir::reverse_slice(args)?;
            if !is_local(source, list) || step != -2 {
                return None;
            }
            Some(Lane {
                phase: phase(start)?,
                table: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            })
        }
        ExprKind::Call(id, args)
            if matches!(args.values.as_slice(), [arg] if is_local(arg, list))
                && args.order == [0] =>
        {
            helpers[*id]
        }
        _ => None,
    }
}

/// Prove the traversal independently of the term: one input, exact reverse
/// cursor, one load and one addition per iteration, and no other effects.
fn mapped_helper(function: &Function) -> Option<Lane> {
    let [param] = function.params.as_slice() else {
        return None;
    };
    if function.locals[*param] != Type::ListInt || function.returns != Type::Int {
        return None;
    }
    let [
        Statement::Assign(sum, zero),
        Statement::Scope { body, clear },
        Statement::Return(result),
    ] = function.body.as_slice()
    else {
        return None;
    };
    let [
        Statement::Assign(list, input),
        Statement::Assign(cursor, start),
        Statement::While(test, next, orelse),
    ] = body.as_slice()
    else {
        return None;
    };
    let [
        Statement::Assign(item, load),
        Statement::Assign(advance, increment),
        Statement::Assign(target, add),
    ] = next.as_slice()
    else {
        return None;
    };
    let ExprKind::Compare(test, ops) = &test.kind else {
        return None;
    };
    let ExprKind::Builtin(Builtin::ListGet(_), args) = &load.kind else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, length, offset) = &start.kind else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, prev, step) = &increment.kind else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, subtotal, term) = &add.kind else {
        return None;
    };
    let slots = [*param, *sum, *list, *cursor, *item];
    if slots
        .iter()
        .enumerate()
        .any(|(i, s)| slots[..i].contains(s))
        || literal(zero) != Some(0)
        || !is_local(result, *sum)
        || clear.contains(sum)
        || !is_local(input, *param)
        || !orelse.is_empty()
        || !matches!(ops.as_slice(), [CmpOp::GtE])
        || !matches!(test.as_slice(),[c,z] if is_local(c,*cursor) && literal(z)==Some(0))
        || !matches!(&length.kind, ExprKind::Builtin(Builtin::Len,args) if matches!(args.as_slice(), [s] if is_local(s,*list)))
        || !matches!(args.as_slice(), [s,c] if is_local(s,*list) && is_local(c,*cursor))
        || advance != cursor
        || !is_local(prev, *cursor)
        || literal(step) != Some(-2)
        || target != sum
        || !is_local(subtotal, *sum)
    {
        return None;
    }
    Some(Lane {
        phase: phase(literal(offset)?)?,
        table: terms::table(term, *item, &slots, function.locals.len())?,
    })
}

fn phase(start: i64) -> Option<usize> {
    match start {
        -1 => Some(0),
        -2 => Some(1),
        _ => None,
    }
}

fn is_local(value: &Expr, id: usize) -> bool {
    matches!(value.kind, ExprKind::Local(s) if s==id)
}

fn local(id: usize) -> Expr {
    Expr::local(id, Type::Int)
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    fn fused(source: &str) -> bool {
        compile(source, &CompileOptions::default())
            .unwrap()
            .contains("call i64 @imre_str_try_digit_sums(")
    }

    #[test]
    fn independent_subtotals_use_the_checked_digit_maps() {
        for (first, second) in [
            (
                "sum(digits[-1::-2])",
                "sum([sum(divmod(2 * d, 10)) for d in digits[-2::-2]])",
            ),
            ("sum([d * 3 for d in digits[-2::-2]])", "sum(digits[::-2])"),
            (
                "sum([254 - d for d in digits[-1::-2]])",
                "sum([d % 7 for d in digits[-2::-2]])",
            ),
        ] {
            let s = format!(
                "def pair(text: str) -> tuple[int, int]:\n    digits = list(map(int, text))\n    x = {first}\n    y = {second}\n    return y, x\n"
            );
            assert!(fused(&s), "{s}");
        }
    }

    #[test]
    fn extra_uses_effects_and_partial_terms_keep_the_original_lists() {
        for (term, tail) in [
            ("d * 3", " + len(digits)"),
            ("report(d)", ""),
            ("17 // d", ""),
            ("d * 40", ""),
        ] {
            let s = format!(
                "def report(d: int) -> int:\n    print(d)\n    return d\n\ndef total(text: str) -> int:\n    digits = list(map(int, text))\n    a = sum(digits[-1::-2])\n    b = sum([{term} for d in digits[-2::-2]])\n    return a + b{tail}\n"
            );
            assert!(!fused(&s), "{s}");
        }
        assert!(!fused(
            "def total(text: str) -> int:\n    digits = list(map(int, text))\n    a = sum(digits[-1::-2])\n    digits[0] = 7\n    b = sum(digits[-2::-2])\n    return a + b\n"
        ));
    }
}
