//! Bounded pure integer callees whose comparison result can stay unboxed.

use crate::ir::{Binary, Compare, Expr, ExprKind, Function, Module, Statement, Type};

use super::{Emitter, Value, conditionals};

const MIN_SMALL: i128 = -(1_i128 << 62);
const MAX_SMALL: i128 = (1_i128 << 62) - 1;
const MAX_NODES: usize = 32;
const MAX_PARAMETERS: usize = 8;
const MAX_STATEMENTS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Interval {
    min: i128,
    max: i128,
}

#[derive(Clone, Copy)]
enum Operation {
    Parameter(usize),
    Constant(i64),
    Add(usize, usize),
    Subtract(usize, usize),
    Negate(usize),
    Absolute(usize),
}

#[derive(Clone)]
struct Node {
    operation: Operation,
    interval: Interval,
}

/// Every intermediate fits signed i64 after all incoming integer tags pass.
/// These values never escape as Python integers: only the final Bool is exposed.
/// A difference of small inputs can use the full i64 range except MIN, allowing
/// its absolute value without allocating the otherwise necessary heap integer.
#[derive(Clone)]
pub(super) struct Summary {
    nodes: Vec<Node>,
    result: usize,
    parameters: usize,
}

pub(super) fn analyze(module: &Module) -> Vec<Option<Summary>> {
    module
        .functions
        .iter()
        .enumerate()
        .map(|(id, function)| {
            // Inlining this arithmetic would skip the observable Python frame.
            if module.source_frames.get(id).is_some_and(Option::is_some) {
                None
            } else {
                summarize(function)
            }
        })
        .collect()
}

/// Summarizes final IR only. No calls, heap reads, scopes, handlers, parameter
/// writes or observable effects are admitted, including in unused assignments.
fn summarize(function: &Function) -> Option<Summary> {
    if function.returns != Type::Int
        || function.locals.iter().any(|ty| *ty != Type::Int)
        || function.params.len() > MAX_PARAMETERS
        || function.body.len() > MAX_STATEMENTS
    {
        return None;
    }
    let (Statement::Return(result), assignments) = function.body.split_last()? else {
        return None;
    };
    let mut summary = Summary {
        nodes: Vec::new(),
        result: 0,
        parameters: function.params.len(),
    };
    let mut locals = vec![None; function.locals.len()];
    for (index, &slot) in function.params.iter().enumerate() {
        locals[slot] = Some(summary.push(
            Operation::Parameter(index),
            Interval {
                min: MIN_SMALL,
                max: MAX_SMALL,
            },
        )?);
    }
    for assignment in assignments {
        let Statement::Assign(slot, value) = assignment else {
            return None;
        };
        if function.params.contains(slot) {
            return None;
        }
        let value = summary.expression(value, &locals)?;
        locals[*slot] = Some(value);
    }
    summary.result = summary.expression(result, &locals)?;
    Some(summary)
}

impl Summary {
    fn push(&mut self, operation: Operation, interval: Interval) -> Option<usize> {
        if self.nodes.len() == MAX_NODES
            || interval.min < i128::from(i64::MIN)
            || interval.max > i128::from(i64::MAX)
        {
            return None;
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            operation,
            interval,
        });
        Some(index)
    }

    /// Interval arithmetic is wider than the emitted operations. Rejection,
    /// rather than a speculative overflow exit, keeps evaluation non-replaying.
    fn expression(&mut self, value: &Expr, locals: &[Option<usize>]) -> Option<usize> {
        if value.ty != Type::Int {
            return None;
        }
        match &value.kind {
            ExprKind::Local(slot) => *locals.get(*slot)?,
            ExprKind::Int(text) => {
                let value = small_literal(text)?;
                self.push(
                    Operation::Constant(value),
                    Interval {
                        min: i128::from(value),
                        max: i128::from(value),
                    },
                )
            }
            ExprKind::Binary(operation @ (Binary::Add | Binary::Subtract), left, right) => {
                let left = self.expression(left, locals)?;
                let right = self.expression(right, locals)?;
                let a = self.nodes[left].interval;
                let b = self.nodes[right].interval;
                let (operation, interval) = match operation {
                    Binary::Add => (
                        Operation::Add(left, right),
                        Interval {
                            min: a.min.checked_add(b.min)?,
                            max: a.max.checked_add(b.max)?,
                        },
                    ),
                    Binary::Subtract => (
                        Operation::Subtract(left, right),
                        Interval {
                            min: a.min.checked_sub(b.max)?,
                            max: a.max.checked_sub(b.min)?,
                        },
                    ),
                    _ => unreachable!(),
                };
                self.push(operation, interval)
            }
            ExprKind::Negate(value) => {
                let input = self.expression(value, locals)?;
                let interval = self.nodes[input].interval;
                self.push(
                    Operation::Negate(input),
                    Interval {
                        min: interval.max.checked_neg()?,
                        max: interval.min.checked_neg()?,
                    },
                )
            }
            ExprKind::Conditional(test, yes, no) => {
                let input =
                    self.expression(conditionals::absolute_operand(test, yes, no)?, locals)?;
                let interval = self.nodes[input].interval;
                let min = if interval.min >= 0 {
                    interval.min
                } else if interval.max <= 0 {
                    interval.max.checked_neg()?
                } else {
                    0
                };
                self.push(
                    Operation::Absolute(input),
                    Interval {
                        min,
                        max: interval.min.checked_neg()?.max(interval.max),
                    },
                )
            }
            _ => None,
        }
    }
}

fn small_literal(text: &str) -> Option<i64> {
    let value = text.parse::<i64>().ok()?;
    (MIN_SMALL..=MAX_SMALL)
        .contains(&i128::from(value))
        .then_some(value)
}

impl Emitter<'_, '_> {
    /// Captures effectful arguments once using ordinary call ownership. Only a
    /// pure local/literal RHS can move before the effect-free callee calculation.
    /// The slow branch reuses the same captures and retains the ordinary owned
    /// result; the fast branch exposes only a Bool, never an untagged Int owner.
    pub(super) fn integer_region_comparison(
        &mut self,
        left: &Expr,
        right: &Expr,
        operation: Compare,
    ) -> Option<Value> {
        let ExprKind::Call(function, args) = &left.kind else {
            return None;
        };
        let summary = self.integer_regions.get(*function)?.as_ref()?.clone();
        let pure_right = match &right.kind {
            ExprKind::Local(_) => true,
            ExprKind::Int(text) => small_literal(text).is_some(),
            _ => false,
        };
        if left.ty != Type::Int
            || right.ty != Type::Int
            || !pure_right
            || args.values.len() != summary.parameters
            || args.order.len() != summary.parameters
            || args.values.iter().any(|value| value.ty != Type::Int)
            || args.order.iter().any(|&index| index >= args.values.len())
        {
            return None;
        }

        let arguments = self.call_operands(args);
        let right = self.operand(right);
        let temporaries = self.temporaries.clone();
        let mut tags = right.value.operand.clone();
        for argument in &arguments {
            tags = self.instruction(format!("and i64 {tags}, {}", argument.value.operand));
        }
        let tags = self.instruction(format!("and i64 {tags}, 1"));
        let small = self.instruction(format!("icmp eq i64 {tags}, 1"));
        let (fast, slow, end) = (self.label(), self.label(), self.label());
        self.line(format!(
            "br i1 {small}, label %{fast}, label %{slow}, !prof !1"
        ));
        self.terminated = true;

        self.start(&fast);
        let mut values: Vec<String> = Vec::with_capacity(summary.nodes.len());
        for node in &summary.nodes {
            let value = match node.operation {
                Operation::Parameter(index) => self.instruction(format!(
                    "ashr i64 {}, 1",
                    arguments[args.order[index]].value.operand
                )),
                Operation::Constant(value) => value.to_string(),
                Operation::Add(left, right) => {
                    self.instruction(format!("add nsw i64 {}, {}", values[left], values[right]))
                }
                Operation::Subtract(left, right) => {
                    self.instruction(format!("sub nsw i64 {}, {}", values[left], values[right]))
                }
                Operation::Negate(value) => {
                    self.instruction(format!("sub nsw i64 0, {}", values[value]))
                }
                Operation::Absolute(value) => {
                    let negative = self.instruction(format!("icmp slt i64 {}, 0", values[value]));
                    let negated = self.instruction(format!("sub nsw i64 0, {}", values[value]));
                    self.instruction(format!(
                        "select i1 {negative}, i64 {negated}, i64 {}",
                        values[value]
                    ))
                }
            };
            values.push(value);
        }
        let right_value = self.instruction(format!("ashr i64 {}, 1", right.value.operand));
        let predicate = match operation {
            Compare::Equal => "eq",
            Compare::NotEqual => "ne",
            Compare::Less => "slt",
            Compare::LessEqual => "sle",
            Compare::Greater => "sgt",
            Compare::GreaterEqual => "sge",
        };
        let fast_value = self.instruction(format!(
            "icmp {predicate} i64 {}, {right_value}",
            values[summary.result]
        ));
        let fast_end = self.current.clone();
        self.jump(&end);

        self.start(&slow);
        self.temporaries = temporaries.clone();
        let left = self.user_call(
            *function,
            Type::Int,
            &args
                .order
                .iter()
                .map(|&index| &arguments[index].value)
                .collect::<Vec<_>>(),
            false,
        );
        let slow_value = self.compare(operation, &left, &right.value);
        self.release(&left);
        let slow_end = self.current.clone();
        self.jump(&end);

        self.start(&end);
        self.temporaries = temporaries;
        let result = Value {
            ty: Type::Bool,
            operand: self.instruction(format!(
                "phi i1 [ {fast_value}, %{fast_end} ], [ {}, %{slow_end} ]",
                slow_value.operand
            )),
            small_integer: false,
        };
        for argument in &arguments {
            self.release_operand(argument);
        }
        self.release_operand(&right);
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileOptions, compile};

    const DISTANCE: &str = "def distance(a: int, b: int) -> int:\n    delta = a - b\n    return -delta if delta < 0 else delta\n";

    fn summary(source: &str) -> Option<Summary> {
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        summarize(&module.functions[0])
    }

    fn caller_body(source: &str, function: usize) -> String {
        let output = compile(source, &CompileOptions::default()).unwrap();
        output
            .split_once(&format!("define internal i1 @fn_{function}("))
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0
            .to_owned()
    }

    #[test]
    fn intervals_admit_full_width_differences_but_reject_overflowing_nodes() {
        let difference =
            summary("def difference(a: int, b: int) -> int:\n    return a - b\n").unwrap();
        assert_eq!(
            difference.nodes[difference.result].interval,
            Interval {
                min: i128::from(i64::MIN) + 1,
                max: i128::from(i64::MAX),
            }
        );
        let magnitude = summary(DISTANCE).unwrap();
        assert_eq!(
            magnitude.nodes[magnitude.result].interval,
            Interval {
                min: 0,
                max: i128::from(i64::MAX),
            }
        );
        let addition = summary("def total(a: int, b: int) -> int:\n    return a + b\n").unwrap();
        assert_eq!(
            addition.nodes[addition.result].interval.min,
            i128::from(i64::MIN)
        );
        for expression in ["-(a + b)", "a + b + a", "a - b - a", "a * b"] {
            assert!(
                summary(&format!(
                    "def rejected(a: int, b: int) -> int:\n    return {expression}\n"
                ))
                .is_none(),
                "{expression}"
            );
        }
        assert!(summary(
            "def rejected(a: int, b: int) -> int:\n    total = a + b\n    return -total if total < 0 else total\n"
        ).is_none());
    }

    #[test]
    fn summaries_keep_old_local_definitions_and_exclude_effects_and_large_dags() {
        let value = summary(
            "def versions(a: int, b: int) -> int:\n    x = a - b\n    old = x\n    x = 0\n    return old\n",
        ).unwrap();
        assert!(matches!(
            value.nodes[value.result].operation,
            Operation::Subtract(..)
        ));
        for body in [
            "    a = a\n    return a\n",
            "    print(a)\n    return a\n",
            "    return (local := a)\n",
            "    return sum(value for value in [a])\n",
            "    if a > 0:\n        return a\n    return b\n",
            "    try:\n        return a\n    finally:\n        print(b)\n",
        ] {
            assert!(summary(&format!("def rejected(a: int, b: int) -> int:\n{body}")).is_none());
        }
        let expression = format!("a{}", " + 0".repeat(MAX_NODES));
        assert!(
            summary(&format!(
                "def rejected(a: int) -> int:\n    return {expression}\n"
            ))
            .is_none()
        );
    }

    #[test]
    fn guarded_comparison_exposes_only_bool_and_retains_the_original_call() {
        let source = format!(
            "{DISTANCE}\ndef probe(a: int, b: int, limit: int) -> bool:\n    return distance(a, b) > limit\n"
        );
        let body = caller_body(&source, 1);
        let decoded = body.find("ashr i64").unwrap();
        assert!(body[..decoded].contains("!prof !1"));
        let fallback = body.find("call i64 @fn_0(").unwrap();
        let fast = &body[decoded..fallback];
        assert!(fast.contains("sub nsw i64"));
        assert!(fast.contains("select i1"));
        assert!(fast.contains("icmp sgt i64"));
        assert!(!fast.contains("call "));
        assert!(body[fallback..].contains("call i32 @compare_int("));
        assert!(body[fallback..].contains("call void @release_int_nonzero("));
        assert!(body[fallback..].contains("phi i1"));
        assert!(!body.contains("phi i64"));

        for expression in [
            "distance(a, b) > distance(a, b)",
            "distance(a, b) > (limit := a)",
            "0 < distance(a, b) < limit",
            "distance(a, b) > 1000000000000000000000000000000",
        ] {
            let body = caller_body(
                &format!(
                    "{DISTANCE}\ndef probe(a: int, b: int, limit: int) -> bool:\n    return {expression}\n"
                ),
                1,
            );
            assert!(!body.contains("ashr i64"), "{expression}");
        }
    }

    #[test]
    fn effectful_arguments_are_captured_once_before_the_guarded_paths() {
        let source = format!(
            "{DISTANCE}\ndef source(value: int) -> int:\n    print(value)\n    return value\n\ndef probe(limit: int) -> bool:\n    return distance(b=source(2), a=source(1)) > limit\n"
        );
        let body = caller_body(&source, 2);
        let decoded = body.find("ashr i64").unwrap();
        assert_eq!(body[..decoded].matches("call i64 @fn_1(").count(), 2);
        assert_eq!(body.matches("call i64 @fn_1(").count(), 2);
        assert_eq!(body.matches("call i64 @fn_0(").count(), 1);
        let calls = body
            .lines()
            .filter(|line| line.contains("call i64 @fn_1("))
            .collect::<Vec<_>>();
        assert!(calls[0].ends_with("i64 5)"));
        assert!(calls[1].ends_with("i64 3)"));
        let first = calls[0].split_once(" = ").unwrap().0.trim();
        let second = calls[1].split_once(" = ").unwrap().0.trim();
        assert!(body.contains(&format!("call i64 @fn_0(i64 {second}, i64 {first})")));
    }
}
