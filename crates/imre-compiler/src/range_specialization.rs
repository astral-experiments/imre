//! Guarded native control for zero-based ranges with an arbitrary-precision stop.
//!
//! The stop is evaluated once before checking its bounds. Separate hidden control
//! slots keep the fast loop's ownership proof independent of the fallback, while
//! both versions preserve the original visible target and Python control flow.

use std::collections::HashMap;

use crate::ir::{Compare, Expr, ExprKind, Module, Statement, Type, walk_expressions};
use crate::ranges;

const MAX_SMALL: i64 = (1_i64 << 62) - 1;

struct Plan {
    stop_argument: usize,
    step: i64,
}

/// Adds a checked fast branch only when existing analysis cannot prove the loop
/// small. Nested body/else loops are excluded to keep cloning linear in code size.
pub(crate) fn specialize(module: &mut Module) {
    let plans = {
        let facts = ranges::analyze(module);
        module
            .functions
            .iter()
            .chain([&module.entry])
            .map(|function| {
                let mut plans = HashMap::new();
                // Range analysis deliberately skips functions with expression
                // rebindings, so adding a guard there would not enable a proof.
                if function.expression_rebound().into_iter().any(|value| value) {
                    return plans;
                }
                visit_statements(&function.body, &mut |statement| {
                    let Statement::For {
                        args, body, orelse, ..
                    } = statement
                    else {
                        return;
                    };
                    if facts.range_cursor(statement).is_some()
                        || contains_loop(body)
                        || contains_loop(orelse)
                    {
                        return;
                    }
                    let (stop_argument, step) = match args.as_slice() {
                        [_] => (0, 1),
                        [start, _] if literal(start) == Some(0) => (1, 1),
                        [start, _, step] if literal(start) == Some(0) => {
                            let Some(step) =
                                literal(step).filter(|step| (1..=MAX_SMALL).contains(step))
                            else {
                                return;
                            };
                            (1, step)
                        }
                        _ => return,
                    };
                    plans.insert(
                        std::ptr::from_ref(statement),
                        Plan {
                            stop_argument,
                            step,
                        },
                    );
                });
                plans
            })
            .collect::<Vec<_>>()
    };
    for (function, mut plans) in module
        .functions
        .iter_mut()
        .chain([&mut module.entry])
        .zip(plans)
    {
        if !plans.is_empty() {
            rewrite_block(&mut function.body, &mut function.locals, &mut plans);
            debug_assert!(plans.is_empty());
        }
    }
}

fn literal(expression: &Expr) -> Option<i64> {
    if let ExprKind::Int(value) = &expression.kind {
        value.parse().ok()
    } else {
        None
    }
}

/// Visits ordinary statement blocks and, separately, every eager expression's
/// statement roots. The shared expression walker already visits nested scopes.
fn visit_statements(body: &[Statement], visit: &mut impl FnMut(&Statement)) {
    visit_block(body, visit);
    walk_expressions(body, &mut |expression| {
        if let ExprKind::Scope { body, .. } = &expression.kind {
            visit_block(body, visit);
        }
    });
}

fn visit_block(body: &[Statement], visit: &mut impl FnMut(&Statement)) {
    for statement in body {
        visit(statement);
        match statement {
            Statement::Scope { body, .. } => visit_block(body, visit),
            Statement::If(_, yes, no) | Statement::While(_, yes, no) => {
                visit_block(yes, visit);
                visit_block(no, visit);
            }
            Statement::For { body, orelse, .. } => {
                visit_block(body, visit);
                visit_block(orelse, visit);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                visit_block(body, visit);
                for handler in handlers {
                    visit_block(&handler.body, visit);
                }
                visit_block(orelse, visit);
                visit_block(finalbody, visit);
            }
            _ => {}
        }
    }
}

fn contains_loop(body: &[Statement]) -> bool {
    let mut found = false;
    visit_statements(body, &mut |statement| {
        found |= matches!(statement, Statement::For { .. } | Statement::While(..));
    });
    found
}

/// Statement addresses identify the immutable analysis plan; they are never
/// dereferenced. Replacing entries in place preserves unvisited sibling addresses.
/// Children are rewritten first, so an effectful stop scope is moved intact.
fn rewrite_block(
    body: &mut [Statement],
    locals: &mut Vec<Type>,
    plans: &mut HashMap<*const Statement, Plan>,
) {
    for statement in body {
        match statement {
            Statement::Assign(_, value)
            | Statement::Expr(value)
            | Statement::Return(value)
            | Statement::Assert(value) => rewrite_expression(value, locals, plans),
            Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    rewrite_expression(value, locals, plans);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                rewrite_expression(a, locals, plans);
                rewrite_expression(b, locals, plans);
            }
            Statement::Scope { body, .. } => rewrite_block(body, locals, plans),
            Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
                rewrite_expression(test, locals, plans);
                rewrite_block(yes, locals, plans);
                rewrite_block(no, locals, plans);
            }
            Statement::For {
                args, body, orelse, ..
            } => {
                for arg in args {
                    rewrite_expression(arg, locals, plans);
                }
                rewrite_block(body, locals, plans);
                rewrite_block(orelse, locals, plans);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                rewrite_block(body, locals, plans);
                for handler in handlers {
                    rewrite_block(&mut handler.body, locals, plans);
                }
                rewrite_block(orelse, locals, plans);
                rewrite_block(finalbody, locals, plans);
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    rewrite_expression(value, locals, plans);
                }
            }
            Statement::Move(..)
            | Statement::Clear(_)
            | Statement::IterNext { .. }
            | Statement::Break
            | Statement::Continue
            | Statement::Exhaust => {}
        }
        if let Some(plan) = plans.remove(&std::ptr::from_ref(statement)) {
            rewrite_loop(statement, locals, plan);
        }
    }
}

fn rewrite_expression(
    expression: &mut Expr,
    locals: &mut Vec<Type>,
    plans: &mut HashMap<*const Statement, Plan>,
) {
    match &mut expression.kind {
        ExprKind::Scope { body, value, .. } => {
            rewrite_block(body, locals, plans);
            rewrite_expression(value, locals, plans);
        }
        ExprKind::List(values)
        | ExprKind::Tuple(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Builtin(_, values) => {
            for value in values {
                rewrite_expression(value, locals, plans);
            }
        }
        ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
            for value in &mut args.values {
                rewrite_expression(value, locals, plans);
            }
        }
        ExprKind::Callable { receiver, .. } => {
            if let Some(receiver) = receiver {
                rewrite_expression(receiver, locals, plans);
            }
        }
        ExprKind::Invoke { callable, args } => {
            rewrite_expression(callable, locals, plans);
            for value in &mut args.values {
                rewrite_expression(value, locals, plans);
            }
        }
        ExprKind::Dict { entries, .. } => {
            for (key, value) in entries {
                rewrite_expression(key, locals, plans);
                rewrite_expression(value, locals, plans);
            }
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => rewrite_expression(value, locals, plans),
        ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
            rewrite_expression(left, locals, plans);
            rewrite_expression(right, locals, plans);
        }
        ExprKind::Conditional(test, yes, no) => {
            rewrite_expression(test, locals, plans);
            rewrite_expression(yes, locals, plans);
            rewrite_expression(no, locals, plans);
        }
        ExprKind::Exception { message, .. } => {
            if let Some(message) = message {
                rewrite_expression(message, locals, plans);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::None
        | ExprKind::Local(_) => {}
    }
}

/// A stop at most MAX_SMALL-step keeps even the final cursor increment small.
/// The captured stop cannot be rebound by user code. Its scope releases it on
/// every exit, while branch-private control owners preserve early else cleanup.
fn rewrite_loop(statement: &mut Statement, locals: &mut Vec<Type>, plan: Plan) {
    let captured = locals.len();
    locals.push(Type::Int);
    let Statement::For { args, .. } = statement else {
        unreachable!()
    };
    // Transfer the captured owner into range initialization before the body or
    // else suite runs. In particular, a BigInt stop must not stay rooted past the
    // original loop's early control-slot cleanup on normal exhaustion.
    let stop_argument = Expr {
        ty: Type::Int,
        kind: ExprKind::Scope {
            body: Vec::new(),
            value: Box::new(local(captured)),
            clear: vec![captured],
        },
    };
    let stop_value = std::mem::replace(&mut args[plan.stop_argument], stop_argument);
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
            vec![integer(0), local(captured), integer(MAX_SMALL - plan.step)],
            vec![Compare::LessEqual, Compare::LessEqual],
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
    Expr {
        ty: Type::Int,
        kind: ExprKind::Local(slot),
    }
}

fn integer(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Binary;
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
        visit_statements(&function.body, &mut |statement| {
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
        visit_statements(body, &mut |statement| {
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
            visit_statements(&function.body, &mut |statement| {
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
        assert!(
            module.functions[1]
                .expression_rebound()
                .into_iter()
                .any(|value| value)
        );
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
