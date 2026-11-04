//! Forward values across equal counted loops when their intermediate lists are private.

mod nullable;
mod periodic;
mod records;

use std::collections::{HashMap, HashSet};

use crate::ir::{
    self, Binary, Builtin, Compare, Expr, ExprKind, Function, Module, Statement, Type,
};

pub(crate) fn specialize(module: &mut Module) {
    let constructors = records::constructors(module);
    for function in &mut module.functions {
        for first in (0..function.body.len()).rev() {
            if let Some((last, replacement, locals)) = fused(function, first)
                .or_else(|| records::fuse(function, first, &constructors))
                .or_else(|| nullable::fuse(function, first, &constructors))
            {
                function.locals.extend(locals);
                function.body.splice(first..=last, replacement);
            }
        }
    }
    periodic::specialize(module);
}

/// Both loops enumerate the ordinal used to append and read each element.
fn counted(statement: &Statement) -> Option<(&Expr, usize, &[Statement])> {
    let (bound, target, body) = ordinal_loop(statement)?;
    matches!(bound.kind, ExprKind::Local(_) | ExprKind::Int(_)).then_some((bound, target, body))
}

/// Recognize a range from zero with unit stride and an arbitrary upper bound.
fn ordinal_loop(statement: &Statement) -> Option<(&Expr, usize, &[Statement])> {
    let Statement::Scope { body, clear } = statement else {
        return None;
    };
    let [
        Statement::For {
            target,
            cursor,
            stop,
            step,
            args,
            body,
            orelse,
        },
    ] = body.as_slice()
    else {
        return None;
    };
    if !orelse.is_empty()
        || clear
            .iter()
            .any(|slot| ![cursor, stop, step].contains(&slot))
    {
        return None;
    }
    let bound = match args.as_slice() {
        [bound] => bound,
        [zero, bound] if integer(zero, "0") => bound,
        [zero, bound, one] if integer(zero, "0") && integer(one, "1") => bound,
        _ => return None,
    };
    Some((bound, *target, body))
}

fn integer(expr: &Expr, value: &str) -> bool {
    matches!(&expr.kind, ExprKind::Int(n) if n == value)
}

fn same_bound(left: &Expr, right: &Expr) -> bool {
    match (&left.kind, &right.kind) {
        (ExprKind::Local(a), ExprKind::Local(b)) => a == b,
        (ExprKind::Int(a), ExprKind::Int(b)) => a == b,
        _ => false,
    }
}

/// Restrict scheduled computations to immutable values and operations that cannot
/// dispatch to Python or raise arithmetic exceptions.
fn producer_value(expr: &Expr, inputs: &mut HashSet<usize>) -> bool {
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_) => true,
        ExprKind::Local(slot)
            if matches!(
                expr.ty,
                Type::Int | Type::Float | Type::Bool | Type::Str | Type::Bytes
            ) =>
        {
            inputs.insert(*slot);
            true
        }
        ExprKind::Tuple(values) => values.iter().all(|value| producer_value(value, inputs)),
        ExprKind::Compare(values, _) => values.iter().all(|value| {
            matches!(value.ty, Type::Int | Type::Bool) && producer_value(value, inputs)
        }),
        ExprKind::Binary(op, a, b) if a.ty == Type::Int && b.ty == Type::Int => {
            let total = match op {
                Binary::Add
                | Binary::Subtract
                | Binary::BitAnd
                | Binary::BitOr
                | Binary::BitXor => true,
                Binary::Modulo => {
                    matches!(&b.kind, ExprKind::Int(n) if n.parse::<i64>().is_ok_and(|n| n > 0))
                }
                _ => false,
            };
            total && producer_value(a, inputs) && producer_value(b, inputs)
        }
        _ => false,
    }
}

/// Check that the consumer neither replaces captured inputs nor hides assignments
/// inside expressions.
fn preserves(body: &[Statement], inputs: &HashSet<usize>) -> bool {
    body.iter().all(|statement| match statement {
        Statement::Assign(slot, _) => !inputs.contains(slot),
        Statement::If(_, yes, no) => preserves(yes, inputs) && preserves(no, inputs),
        Statement::Expr(_) => true,
        _ => false,
    })
}

struct Column {
    value: Expr,
    reads: usize,
    temp: Option<usize>,
}

/// Finds a read of exactly the element produced for this ordinal.
fn row_read(expr: &Expr, index: usize) -> Option<usize> {
    let ExprKind::Builtin(Builtin::ListGet, args) = &expr.kind else {
        return None;
    };
    let [collection, offset] = args.as_slice() else {
        return None;
    };
    let ExprKind::Local(slot) = collection.kind else {
        return None;
    };
    matches!(offset.kind, ExprKind::Local(i) if i == index).then_some(slot)
}

/// The producer may name its ordinal differently from the consumer.
fn remap_index(expr: &mut Expr, from: usize, to: usize) {
    if matches!(expr.kind, ExprKind::Local(slot) if slot == from) {
        expr.kind = ExprKind::Local(to);
    } else {
        ir::children_mut(expr, |child| remap_index(child, from, to));
    }
}

fn greater(left: &Expr, right: i128) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(
            vec![left.clone(), Expr::integer(right)],
            vec![Compare::Greater],
        ),
    }
}

/// Find the original top-level list initialization before its producer.
fn seed(function: &Function, first: usize, slot: usize) -> Option<&[Expr]> {
    function.body[..first]
        .iter()
        .find_map(|statement| match statement {
            Statement::Assign(
                to,
                Expr {
                    kind: ExprKind::List(items),
                    ..
                },
            ) if *to == slot => Some(items.as_slice()),
            _ => None,
        })
}

fn absent(value: &Expr) -> bool {
    matches!(&value.kind, ExprKind::Absent(inner) if matches!(inner.kind,ExprKind::None))
}

/// Only the original append receivers and the forwarded reads may see these
/// freshly initialized lists. Count slot transfers separately from expressions.
fn private_lists(
    function: &Function,
    first: usize,
    consumer_reads: &HashMap<usize, usize>,
    inputs: &HashSet<usize>,
    prefix: usize,
) -> bool {
    let mut reads = HashMap::new();
    let mut writes = HashMap::new();
    let mut hidden = false;
    let protected = |slot: &usize| consumer_reads.contains_key(slot) || inputs.contains(slot);
    ir::walk_expressions(&function.body, &mut |expr| match &expr.kind {
        ExprKind::Local(slot) if consumer_reads.contains_key(slot) => {
            *reads.entry(*slot).or_insert(0_usize) += 1
        }
        ExprKind::Scope { clear, .. } => hidden |= clear.iter().any(&protected),
        _ => {}
    });
    ir::walk_statements(&function.body, &mut |s| match s {
        Statement::Assign(slot, _) if consumer_reads.contains_key(slot) => {
            *writes.entry(*slot).or_insert(0) += 1
        }
        Statement::Move(a, b) => hidden |= protected(a) || protected(b),
        Statement::For {
            target,
            cursor,
            stop,
            step,
            ..
        } => {
            hidden |= [target, cursor, stop, step]
                .iter()
                .any(|slot| consumer_reads.contains_key(*slot));
        }
        Statement::IterNext {
            iterator,
            key,
            value,
            ready,
            ..
        } => {
            hidden |= [Some(iterator), Some(key), value.as_ref(), Some(ready)]
                .into_iter()
                .flatten()
                .any(|slot| consumer_reads.contains_key(slot));
        }
        Statement::Clear(slot) => hidden |= protected(slot),
        Statement::Try { handlers, .. } => {
            hidden |= handlers
                .iter()
                .filter_map(|handler| handler.name.as_ref())
                .any(&protected)
        }
        Statement::Scope { clear, .. } => hidden |= clear.iter().any(&protected),
        _ => {}
    });
    !hidden
        && consumer_reads.iter().all(|(slot, expected)| {
            let initialized = seed(function, first, *slot)
                .is_some_and(|items| items.len() == prefix && items.iter().all(absent));
            initialized
                && *expected > 0
                && reads.get(slot) == Some(&(1 + expected))
                && writes.get(slot) == Some(&1)
        })
}

/// Only constant local initialization may separate the two loops.
fn next_loop(function: &Function, first: usize) -> Option<usize> {
    let offset = function.body[first + 1..].iter().position(|s| {
        !matches!(
            s,
            Statement::Assign(
                _,
                Expr {
                    kind: ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Float(_),
                    ..
                }
            )
        )
    })?;
    Some(first + 1 + offset)
}

/// Preserve oversized append errors, the last producer ordinal, and the gap.
fn loop_prefix(
    function: &Function,
    first: usize,
    last: usize,
    bound: &Expr,
    source_index: usize,
    index: usize,
) -> Vec<Statement> {
    // An oversized producer must encounter the original append error before
    // any consumer effect. Keep that producer intact on its cold path.
    let mut replacement = vec![Statement::If(
        greater(bound, crate::ranges::MAX_LIST_LEN),
        vec![function.body[first].clone()],
        vec![],
    )];
    if source_index != index {
        let final_index = Expr {
            ty: Type::Int,
            kind: ExprKind::Binary(
                Binary::Subtract,
                Box::new(bound.clone()),
                Box::new(Expr::integer(1)),
            ),
        };
        replacement.push(Statement::If(
            greater(bound, 0),
            vec![Statement::Assign(source_index, final_index)],
            vec![],
        ));
    }
    replacement.extend_from_slice(&function.body[first + 1..last]);
    replacement
}

/// A callback could observe intermediate ownership while a producer is rescheduled.
fn observable_consumer(expr: &Expr) -> bool {
    !matches!(
        &expr.kind,
        ExprKind::Int(_)
            | ExprKind::Bool(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::Local(_)
            | ExprKind::Tuple(_)
            | ExprKind::Binary(..)
            | ExprKind::Compare(..)
            | ExprKind::Not(_)
            | ExprKind::Negate(_)
            | ExprKind::Boolean(..)
            | ExprKind::Conditional(..)
            | ExprKind::Builtin(Builtin::ListGet | Builtin::TupleGet | Builtin::Len, _)
    ) && !matches!(&expr.kind, ExprKind::Field(base,_) if matches!(base.ty,Type::Tuple(_)))
}

fn fused(function: &Function, first: usize) -> Option<(usize, Vec<Statement>, Vec<Type>)> {
    let (bound, source_index, producer) = counted(&function.body[first])?;
    let last = next_loop(function, first)?;
    let (other, index, consumer) = counted(&function.body[last])?;
    if bound.ty != Type::Int
        || function.locals[source_index] != Type::Int
        || function.locals[index] != Type::Int
        || !same_bound(bound, other)
        || matches!(bound.kind, ExprKind::Local(slot) if slot == source_index || slot == index)
    {
        return None;
    }
    let mut columns = HashMap::new();
    let mut order = Vec::new();
    let mut inputs = HashSet::new();
    for statement in producer {
        let Statement::Expr(Expr {
            kind: ExprKind::Builtin(Builtin::Append, args),
            ..
        }) = statement
        else {
            return None;
        };
        let [collection, value] = args.as_slice() else {
            return None;
        };
        let ExprKind::Local(slot) = collection.kind else {
            return None;
        };
        if !producer_value(value, &mut inputs)
            || columns
                .insert(
                    slot,
                    Column {
                        value: value.clone(),
                        reads: 0,
                        temp: None,
                    },
                )
                .is_some()
        {
            return None;
        }
        order.push(slot);
    }
    if columns.is_empty() || (source_index != index && inputs.contains(&index)) {
        return None;
    }
    if let ExprKind::Local(slot) = bound.kind {
        inputs.insert(slot);
    }
    inputs.insert(source_index);
    inputs.insert(index);
    let rebound = function.expression_rebound();
    if inputs
        .iter()
        .chain(columns.keys())
        .any(|&slot| rebound[slot])
        || !preserves(&function.body[first + 1..last], &inputs)
        || !preserves(consumer, &inputs)
    {
        return None;
    }
    for &slot in &inputs {
        if slot != index
            && slot != source_index
            && !function.params.contains(&slot)
            && !function.body[..first]
                .iter()
                .any(|s| matches!(s, Statement::Assign(to, _) if *to == slot))
        {
            return None;
        }
    }
    let mut observable = false;
    ir::walk_expressions(consumer, &mut |expr| {
        observable |= observable_consumer(expr);
        if let Some(slot) = row_read(expr, index)
            && let Some(column) = columns.get_mut(&slot)
        {
            column.reads += 1;
        }
    });
    let reads = columns
        .iter()
        .map(|(&slot, column)| (slot, column.reads))
        .collect();
    if observable || !private_lists(function, first, &reads, &inputs, 0) {
        return None;
    }

    let mut replacement = loop_prefix(function, first, last, bound, source_index, index);
    let mut combined = function.body[last].clone();
    let Statement::Scope { body, .. } = &mut combined else {
        unreachable!()
    };
    let Statement::For { body, .. } = &mut body[0] else {
        unreachable!()
    };
    let mut locals = Vec::new();
    let mut staged = Vec::new();
    for slot in order {
        let column = columns.get_mut(&slot)?;
        if source_index != index {
            remap_index(&mut column.value, source_index, index);
        }
        if column.reads > 1 {
            let temp = function.locals.len() + locals.len();
            locals.push(column.value.ty);
            staged.push(Statement::Assign(temp, column.value.clone()));
            column.temp = Some(temp);
        }
    }
    ir::walk_expressions_mut(body, &mut |expr| {
        if let Some(slot) = row_read(expr, index)
            && let Some(column) = columns.get(&slot)
        {
            if let Some(temp) = column.temp {
                expr.kind = ExprKind::Local(temp);
            } else {
                *expr = column.value.clone();
            }
        }
    });
    body.splice(..0, staged);
    replacement.push(combined);
    Some((last, replacement, locals))
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn forwards_private_rows_and_preserves_observable_lists() {
        let source = r#"
def lengths(count: int, name: str) -> int:
    names: list[str] = []
    for i in range(count):
        names.append(name)
    total = 0
    for j in range(count):
        total += len(names[j])
    return total
print(lengths(int(input()), "row"))
"#;
        let reads_list = |source: &str| {
            let llvm = compile(source, &CompileOptions::default()).unwrap();
            llvm.lines()
                .any(|line| line.contains("call ") && line.contains("@imre_seq_get"))
        };
        assert!(!reads_list(source));
        assert!(reads_list(
            &source.replace("return total", "return total + len(names)")
        ));
        assert!(reads_list(
            &source.replace("names[j]", "names[count - j - 1]")
        ));
        assert!(reads_list(&source.replace(
            "total += len(names[j])",
            "name = \"changed\"\n        total += len(names[j])"
        )));
        assert!(reads_list(
            &source.replace("for i in range(count)", "for count in range(count)")
        ));
    }
}
