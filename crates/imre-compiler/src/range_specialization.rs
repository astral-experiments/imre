//! Guarded native control for zero-based ranges with an arbitrary-precision stop.
//!
//! The stop is evaluated once before checking its bounds. Separate hidden control
//! slots keep the fast loop's ownership proof independent of the fallback, while
//! both versions preserve the original visible target and Python control flow.

use ruff_python_ast::CmpOp;
use std::collections::HashSet;

use crate::ir::{self, Expr, ExprKind, Module, Statement, Type};
use crate::ranges;

const MAX_SMALL: i64 = (1_i64 << 62) - 1;

/// Adds a checked fast branch only when existing analysis cannot prove the loop
/// small. Nested body/else loops are excluded to keep cloning linear in code size.
pub(crate) fn specialize(module: &mut Module) {
    let small_cursors = ranges::analyze(module).into_small_cursors();
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        // Range analysis skips functions with expression rebindings.
        if function.has_expression_bindings() {
            continue;
        }
        ir::rewrite_statements(&mut function.body, &mut |statement| {
            rewrite_loop(statement, &mut function.locals, &small_cursors);
        });
    }
}

fn literal(expression: &Expr) -> Option<i64> {
    if let ExprKind::Int(value) = &expression.kind {
        value.parse().ok()
    } else {
        None
    }
}

/// A stop at most MAX_SMALL-step keeps even the final cursor increment small.
/// The captured stop cannot be rebound by user code. Its scope releases it on
/// every exit, while branch-private control owners preserve early else cleanup.
fn rewrite_loop(
    statement: &mut Statement,
    locals: &mut Vec<Type>,
    small_cursors: &HashSet<*const Statement>,
) {
    let id = std::ptr::from_ref(statement);
    let Statement::For {
        args, body, orelse, ..
    } = statement
    else {
        return;
    };
    // A rewritten child keeps its original loop in the fallback.
    if small_cursors.contains(&id) || ir::contains_loop(body) || ir::contains_loop(orelse) {
        return;
    }
    let (stop_argument, stride) = match args.as_slice() {
        [_] => (0, 1),
        [start, _] if literal(start) == Some(0) => (1, 1),
        [start, _, step] if literal(start) == Some(0) => {
            let Some(step) = literal(step).filter(|step| (1..=MAX_SMALL).contains(step)) else {
                return;
            };
            (1, step)
        }
        _ => return,
    };
    let captured = locals.len();
    locals.push(Type::Int);
    // Transfer the captured owner into range initialization before the body or
    // else suite runs. In particular, a BigInt stop must not stay rooted past the
    // original loop's early control-slot cleanup on normal exhaustion.
    let captured_stop = Expr {
        ty: Type::Int,
        kind: ExprKind::Scope {
            body: Vec::new(),
            value: Box::new(local(captured)),
            clear: vec![captured],
        },
    };
    let stop_value = std::mem::replace(&mut args[stop_argument], captured_stop);
    let mut fast = statement.clone();
    let Statement::For {
        cursor,
        stop,
        step,
        orelse,
        ..
    } = &mut fast
    else {
        unreachable!()
    };
    let previous = [*cursor, *stop, *step];
    let controls = [locals.len(), locals.len() + 1, locals.len() + 2];
    locals.extend([Type::Int; 3]);
    [*cursor, *stop, *step] = controls;
    // Frontend lowering releases the range's hidden owners before entering else.
    // Those straight-line clears must refer to this branch's private controls.
    for statement in orelse {
        if let Statement::Clear(slot) = statement
            && let Some(index) = previous.iter().position(|old| old == slot)
        {
            *slot = controls[index];
        }
    }
    let fallback = std::mem::replace(statement, Statement::Clear(captured));
    let guard = Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(
            vec![
                Expr::integer(0),
                local(captured),
                Expr::integer(MAX_SMALL - stride),
            ],
            vec![CmpOp::LtE, CmpOp::LtE],
        ),
    };
    *statement = Statement::Scope {
        body: vec![
            Statement::Assign(captured, stop_value),
            Statement::If(
                guard,
                vec![Statement::Scope {
                    body: vec![fast],
                    clear: controls.to_vec(),
                }],
                vec![fallback],
            ),
        ],
        clear: vec![captured],
    };
}

fn local(slot: usize) -> Expr {
    Expr::local(slot, Type::Int)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Binary, walk_expressions};
    use crate::ranges::IntRange;

    fn module(source: &str) -> Module {
        let mut module =
            crate::frontend::lower(&crate::modules::SourceGraph::single(source).unwrap()).unwrap();
        crate::tuples::scalarize(&mut module);
        module
    }

    #[test]
    fn guard_proves_final_increment_and_index_without_erasing_target_ownership() {
        let mut module = module(
            r#"
def scan(data: bytes) -> None:
    for index in range(0, len(data) - 1, 2):
        print(index + 1)
        index = 1267650600228229401496703205376
    else:
        print("done")
"#,
        );
        specialize(&mut module);
        let facts = ranges::analyze(&module);
        let function = &module.functions[0];
        let small = facts.small_locals(function);
        let mut fast = 0;
        let mut fallback = 0;
        ir::walk_statements(&function.body, &mut |statement| {
            let Statement::For {
                target,
                cursor,
                stop,
                step,
                body,
                orelse,
                ..
            } = statement
            else {
                return;
            };
            assert!(
                !small[*target],
                "the visible target can own a rebound BigInt"
            );
            let controls = [*cursor, *stop, *step];
            if let Some(proof) = facts.range_cursor(statement) {
                fast += 1;
                assert_eq!(
                    proof,
                    IntRange {
                        min: 0,
                        max: MAX_SMALL - 1
                    }
                );
                assert!(controls.iter().all(|&slot| small[slot]));
                for (clear, slot) in orelse.iter().zip(controls) {
                    assert!(matches!(clear, Statement::Clear(value) if *value == slot));
                }
                let mut additions = 0;
                walk_expressions(body, &mut |value| {
                    if matches!(value.kind, ExprKind::Binary(Binary::Add, ..)) {
                        additions += 1;
                        assert_eq!(
                            facts.integer(value),
                            Some(IntRange {
                                min: 1,
                                max: MAX_SMALL - 2
                            })
                        );
                    }
                });
                assert_eq!(additions, 1);
            } else {
                fallback += 1;
                assert!(controls.iter().all(|&slot| !small[slot]));
            }
        });
        assert_eq!((fast, fallback), (1, 1));
    }

    #[test]
    fn effectful_scoped_stop_is_captured_once_after_its_own_loop_rewrite() {
        let mut module = module(
            r#"
def bound(limit: int) -> int:
    print(limit)
    return limit

def scan(limit: int) -> None:
    for value in range([*range(bound(limit))][0]):
        print(value)
"#,
        );
        specialize(&mut module);
        let facts = ranges::analyze(&module);
        let body = &module.functions[1].body;
        let mut calls = 0;
        walk_expressions(body, &mut |value| {
            calls += usize::from(matches!(value.kind, ExprKind::Call(0, _)));
        });
        let mut fast = 0;
        let mut fallback = 0;
        ir::walk_statements(body, &mut |statement| {
            if matches!(statement, Statement::For { .. }) {
                if facts.range_cursor(statement).is_some() {
                    fast += 1;
                } else {
                    fallback += 1;
                }
            }
        });
        assert_eq!(
            calls, 1,
            "the nested stop must not be duplicated into either loop version"
        );
        assert_eq!((fast, fallback), (2, 2));
    }

    #[test]
    fn nested_body_and_else_loops_are_not_cloned_recursively() {
        let mut module = module(
            r#"
def nested_body(outer: int, inner: int) -> None:
    for first in range(outer):
        for second in range(inner):
            print(second)

def nested_else(outer: int, inner: int) -> None:
    for first in range(outer):
        print(first)
    else:
        for second in range(inner):
            print(second)
"#,
        );
        specialize(&mut module);
        let facts = ranges::analyze(&module);
        for function in &module.functions {
            let mut loops = 0;
            let mut fast = 0;
            ir::walk_statements(&function.body, &mut |statement| {
                if matches!(statement, Statement::For { .. }) {
                    loops += 1;
                    fast += usize::from(facts.range_cursor(statement).is_some());
                }
            });
            assert_eq!((loops, fast), (3, 1));
        }
    }

    #[test]
    fn existing_proofs_and_unsupported_controls_remain_unchanged() {
        let mut module = module(
            r#"
def controls(limit: int, step: int) -> None:
    for value in range(1, limit):
        print(value)
    for value in range(0, limit, step):
        print(value)
    for value in range(0, limit, -1):
        print(value)
    for value in range(0, limit, 4611686018427387904):
        print(value)
    for value in range(6):
        print(value)

def bindings(limit: int) -> None:
    stored = 0
    for value in range(limit):
        print(stored := value)
"#,
        );
        let lengths = module
            .functions
            .iter()
            .map(|function| function.locals.len())
            .collect::<Vec<_>>();
        assert!(module.functions[1].has_expression_bindings());
        specialize(&mut module);
        assert_eq!(
            module
                .functions
                .iter()
                .map(|function| function.locals.len())
                .collect::<Vec<_>>(),
            lengths
        );
    }
}
