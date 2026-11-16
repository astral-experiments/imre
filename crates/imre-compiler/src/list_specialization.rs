//! Checked loop versions for stable input lists. A captured range can justify
//! scanning for bounded integers. Without a range, each fast read checks its
//! own tagged word. Runtime identity guards exclude aliases of append targets.

mod window_ranges;
pub(crate) use window_ranges::{is_window_range_guard, reserved_append_window};

use ruff_python_ast::CmpOp;
use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{Binary, Builtin, Expr, ExprKind, Module, Statement, Type, contains_loop};

/// Versions leaf loops after tuple scalarization.
/// Unknown calls and mutations retain the original arbitrary-precision path.
pub(crate) fn specialize(module: &mut Module) {
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        if function.has_expression_bindings() {
            continue;
        }
        let private_cursors = crate::ir::private_string_cursors(function);
        crate::ir::rewrite_statements(&mut function.body, &mut |statement| {
            rewrite_loop(statement, &mut function.locals, &private_cursors);
        });
        window_ranges::specialize(function);
    }
}

fn rewrite_loop(statement: &mut Statement, locals: &mut Vec<Type>, private_cursors: &[usize]) {
    let induction = match statement {
        Statement::For { target, .. } => Some(*target),
        _ => None,
    };
    let (arguments, body, orelse, while_source) = match statement {
        Statement::For {
            args, body, orelse, ..
        } => {
            let Some(args) = range_arguments(args) else {
                return;
            };
            (Some(args), body, orelse, None)
        }
        Statement::While(test, body, orelse) => match while_arguments(test) {
            Some((args, source)) => (Some(args), body, orelse, Some(source)),
            None if matches!(test.kind, ExprKind::Bool(true)) => (None, body, orelse, None),
            None => return,
        },
        _ => return,
    };
    if contains_loop(body) || contains_loop(orelse) {
        return;
    }
    let mut written = vec![false; locals.len()];
    crate::ir::walk_writes(body, |slot, _| written[slot] = true);
    if while_source.is_some_and(|source| written[source]) {
        return;
    }
    let mut aliases = locals
        .iter()
        .enumerate()
        .map(|(slot, ty)| (*ty == Type::ListInt && !written[slot]).then_some(slot))
        .collect::<Vec<_>>();
    let mut fast = statement.clone();
    let body = match &mut fast {
        Statement::For { body, .. } | Statement::While(_, body, _) => body,
        _ => unreachable!(),
    };
    let mut plan = Plan {
        locals,
        scan_inputs: arguments.is_some(),
        inputs: BTreeMap::new(),
        outputs: BTreeSet::new(),
        appends: 0,
        nontrivial_arithmetic: false,
    };
    if !plan.block(body, &mut aliases) {
        return;
    }
    if !plan.scan_inputs {
        plan.inputs.retain(|slot, _| !plan.outputs.contains(slot));
    } else if plan.inputs.keys().any(|slot| plan.outputs.contains(slot)) {
        return;
    }
    let reads = plan.inputs.values().sum::<usize>();
    if (reads < 2 && !(while_source.is_some() && reads == 1 && plan.nontrivial_arithmetic))
        || while_source.is_some_and(|source| !plan.inputs.contains_key(&source))
    {
        return;
    }
    if !plan.scan_inputs {
        // Only this loop body was checked. Its else suite may run arbitrary code.
        crate::ir::walk_expressions_mut(body, &mut |value| {
            if let ExprKind::Builtin(operation, args) = &mut value.kind
                && let Builtin::ListGet(origin) = operation
                && let ExprKind::Local(slot) = args[0].kind
                && plan.inputs.contains_key(&slot)
                && args[1].ty == Type::Int
            {
                *operation = Builtin::ListGetStable(origin.take());
            }
        });
    }
    let mut guards = Vec::new();
    for &input in plan.inputs.keys() {
        for &output in &plan.outputs {
            guards.push(builtin(
                Builtin::ListDistinct,
                vec![list_local(input), list_local(output)],
            ));
        }
        guards.push(match &arguments {
            Some([start, stop, step]) => builtin(
                Builtin::ListAllI32,
                vec![list_local(input), start.clone(), stop.clone(), step.clone()],
            ),
            None => builtin(Builtin::ListPresent, vec![list_local(input)]),
        });
    }
    if let (Some(cursor), Some(arguments)) = (induction, &arguments)
        && !written[cursor]
        && arguments[0]
            .tagged_integer_literal()
            .is_some_and(|start| start >= 0)
        && arguments[2]
            .tagged_integer_literal()
            .is_some_and(|step| step > 0)
    {
        prove_range_windows(
            body,
            cursor,
            &arguments[1],
            &written,
            &plan.inputs,
            &mut guards,
        );
    }
    let Plan {
        outputs, appends, ..
    } = plan;
    let mut fast_body = Vec::new();
    let cursor_loop = arguments.is_none();
    if let Some([start, mut stop, step]) = arguments {
        // Every append target passed ListDistinct against the condition's source.
        // Only this arm can reuse its length across the loop backedge.
        if !outputs.is_empty()
            && let Statement::While(test, _, _) = &mut fast
            && let ExprKind::Compare(operands, _) = &mut test.kind
        {
            let slot = locals.len();
            locals.push(Type::Int);
            fast_body.push(Statement::Assign(slot, stop));
            stop = Expr::local(slot, Type::Int);
            operands[1] = stop.clone();
        }
        if outputs.len() == 1 {
            // Hints beyond the runtime's slot budget cannot reserve more space.
            let appends = appends.min(32_768) as i64;
            fast_body.push(Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::ListReserveRange,
                    vec![
                        list_local(*outputs.first().unwrap()),
                        start,
                        stop,
                        step,
                        Expr::integer(appends),
                    ],
                ),
            }));
        }
    }
    if cursor_loop
        && outputs.len() == 1
        && let Some(&output) = outputs.first()
        && buffer_appends(&mut fast, output, locals, private_cursors)
    {
        let marker = |op| {
            Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(op, Vec::new()),
            })
        };
        fast_body.push(marker(Builtin::BeginListAppends(output)));
        fast_body.push(Statement::Try {
            body: vec![fast],
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![marker(Builtin::FinishListAppends(output))],
        });
    } else {
        fast_body.push(fast);
    }
    let fallback = std::mem::replace(statement, Statement::Break(None));
    *statement = Statement::If(
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Boolean(true, guards),
        },
        fast_body,
        vec![fallback],
    );
}

/// A checked leaf loop can defer publishing immediate appends when its only
/// observations of the destination are tail reads. Other inputs are distinct,
/// and all local values released inside the loop have native destructors.
fn buffer_appends(
    statement: &mut Statement,
    output: usize,
    locals: &[Type],
    private_cursors: &[usize],
) -> bool {
    let Statement::While(_, body, orelse) = statement else {
        return false;
    };
    if !orelse.is_empty() {
        return false;
    }
    let scalar = |ty: Type| matches!(ty, Type::Int | Type::Bool | Type::Str | Type::None);
    let mut safe = true;
    let mut tail = false;
    crate::ir::walk_writes(body, |slot, _| {
        safe &= scalar(locals[slot])
            || locals[slot] == Type::ListInt
            || (locals[slot] == Type::Iterator && private_cursors.contains(&slot))
    });
    crate::ir::walk_expressions(body, &mut |value| {
        safe &= match &value.kind {
            ExprKind::Builtin(Builtin::ListGet(_), args) => {
                let reads = matches!(args[0].kind, ExprKind::Local(slot) if slot == output)
                    && args[1].tagged_integer_literal() == Some(-1);
                tail |= reads;
                reads
            }
            ExprKind::Builtin(Builtin::ListGetStable(_), _) => true,
            ExprKind::Builtin(Builtin::Append, args) => {
                args[0].ty == Type::ListInt && args[1].ty == Type::Int
            }
            ExprKind::Builtin(
                Builtin::Int(_)
                | Builtin::Ord
                | Builtin::Bool
                | Builtin::Abs
                | Builtin::IntExtremum { .. },
                args,
            ) => args.iter().all(|arg| scalar(arg.ty)),
            ExprKind::Builtin(..) => false,
            ExprKind::Local(_) => scalar(value.ty) || value.ty == Type::ListInt,
            ExprKind::Scope { value: result, .. } => scalar(result.ty),
            _ => {
                let mut primitive = scalar(value.ty);
                crate::ir::children(value, |child| primitive &= scalar(child.ty));
                primitive
            }
        };
    });
    if !safe || !tail {
        return false;
    }
    crate::ir::walk_expressions_mut(body, &mut |value| {
        if let ExprKind::Builtin(op, _) = &mut value.kind {
            match op {
                Builtin::ListGet(origin) => *op = Builtin::GetInAppendWindow(output, origin.take()),
                Builtin::Append => *op = Builtin::AppendInWindow(output),
                _ => {}
            }
        }
    });
    true
}

struct Plan<'a> {
    locals: &'a [Type],
    scan_inputs: bool,
    /// Number of eligible reads associated with each rooted source slot.
    inputs: BTreeMap<usize, usize>,
    outputs: BTreeSet<usize>,
    /// Upper bound for one iteration; mutually exclusive arms take the maximum.
    appends: usize,
    /// A single list read justifies a scan only when it enables useful arithmetic.
    nontrivial_arithmetic: bool,
}

impl Plan<'_> {
    /// Tracks direct aliases in evaluation order. A written local starts unknown
    /// on every iteration, so a branch or backedge cannot reuse a stale alias.
    fn block(&mut self, body: &mut [Statement], aliases: &mut [Option<usize>]) -> bool {
        for statement in body {
            match statement {
                Statement::Assign(slot, value) => {
                    if !self.expression(value, aliases) {
                        return false;
                    }
                    if self.locals[*slot] == Type::ListInt {
                        aliases[*slot] = alias(value, aliases);
                    }
                }
                Statement::Move(target, source) => {
                    aliases[*target] = aliases[*source];
                    aliases[*source] = None;
                }
                Statement::Clear(slot) => aliases[*slot] = None,
                Statement::Scope { body, clear } => {
                    if !self.block(body, aliases) {
                        return false;
                    }
                    for &slot in clear.iter() {
                        aliases[slot] = None;
                    }
                }
                Statement::Expr(value) | Statement::Return(value) => {
                    if !self.expression(value, aliases) {
                        return false;
                    }
                }
                Statement::If(test, yes, no) => {
                    if !self.expression(test, aliases) {
                        return false;
                    }
                    let mut yes_aliases = aliases.to_vec();
                    let mut no_aliases = aliases.to_vec();
                    let before = self.appends;
                    if !self.block(yes, &mut yes_aliases) {
                        return false;
                    }
                    let yes_appends = self.appends;
                    self.appends = before;
                    if !self.block(no, &mut no_aliases) {
                        return false;
                    }
                    self.appends = self.appends.max(yes_appends);
                    for ((slot, yes), no) in aliases.iter_mut().zip(yes_aliases).zip(no_aliases) {
                        *slot = (yes == no).then_some(yes).flatten();
                    }
                }
                Statement::Raise(value) => {
                    if let Some(value) = value
                        && (!self.expression(&mut value.exception, aliases)
                            || value
                                .cause
                                .as_mut()
                                .is_some_and(|cause| !self.expression(cause, aliases)))
                    {
                        return false;
                    }
                }
                Statement::Match(..) => return false,
                Statement::Position(..)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::Exhaust => {}
                // An exact string cursor cannot call Python or resize a list.
                Statement::IterNext {
                    kind: crate::ir::IterNextKind::CodePoint,
                    ..
                } if !self.scan_inputs => {}
                Statement::SetItem(..)
                | Statement::AugmentItem(..)
                | Statement::SetField(..)
                | Statement::AugmentField(..)
                | Statement::While(..)
                | Statement::For { .. }
                | Statement::IterNext { .. }
                | Statement::With { .. }
                | Statement::Try { .. } => return false,
            }
        }
        true
    }

    /// Appends are the only permitted mutation. Capturing their receiver's root
    /// before later operands preserves the source language's evaluation order.
    fn expression(&mut self, expression: &mut Expr, aliases: &mut [Option<usize>]) -> bool {
        if expression.uses_python_unary_protocol() {
            return false;
        }
        match &mut expression.kind {
            ExprKind::Builtin(operation, args) => {
                let receiver = args.first().and_then(|value| alias(value, aliases));
                if !args.iter_mut().all(|value| self.expression(value, aliases)) {
                    return false;
                }
                match operation {
                    Builtin::ListGet(origin) if args[0].ty == Type::ListInt => {
                        let Some(root) = receiver else {
                            return false;
                        };
                        if self.scan_inputs {
                            *operation = Builtin::ListGetI32(origin.take());
                        } else if args[1].ty == Type::Int {
                            // A loop-local alias denotes this same rooted input.
                            args[0] = list_local(root);
                        } else {
                            return true;
                        }
                        *self.inputs.entry(root).or_default() += 1;
                    }
                    Builtin::Append if args[0].ty == Type::ListInt => {
                        let Some(root) = receiver else {
                            return false;
                        };
                        self.outputs.insert(root);
                        self.appends += 1;
                    }
                    operation if operation.contract(args).preserves_list_storage() => {}
                    _ => return false,
                }
            }
            ExprKind::Scope { body, value, clear } => {
                if !self.block(body, aliases) || !self.expression(value, aliases) {
                    return false;
                }
                for &slot in clear.iter() {
                    aliases[slot] = None;
                }
            }
            ExprKind::List(values) | ExprKind::Tuple(values) | ExprKind::Set { values, .. } => {
                if !values
                    .iter_mut()
                    .all(|value| self.expression(value, aliases))
                {
                    return false;
                }
            }
            ExprKind::Compare(_, ops)
                if crate::ir::Compare::uses_python_protocol(expression.ty, ops) =>
            {
                return false;
            }
            ExprKind::Compare(values, _) | ExprKind::Boolean(_, values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    if index == 0 {
                        if !self.expression(value, aliases) {
                            return false;
                        }
                    } else {
                        let mut taken = aliases.to_vec();
                        if !self.expression(value, &mut taken) {
                            return false;
                        }
                        for (slot, taken) in aliases.iter_mut().zip(taken) {
                            if *slot != taken {
                                *slot = None;
                            }
                        }
                    }
                }
            }
            ExprKind::Dict(entries) => {
                if expression.ty == Type::PythonObject
                    || !entries.iter_mut().all(|entry| {
                        entry
                            .key
                            .as_mut()
                            .is_some_and(|key| self.expression(key, aliases))
                            && self.expression(&mut entry.value, aliases)
                    })
                {
                    return false;
                }
            }
            ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Unary(_, value)
            | ExprKind::Field(value, _) => {
                if !self.expression(value, aliases) {
                    return false;
                }
            }
            ExprKind::Binary(operation, left, right) => {
                self.nontrivial_arithmetic |= expression.ty == Type::Int
                    && matches!(
                        operation,
                        Binary::Multiply
                            | Binary::FloorDivide
                            | Binary::Modulo
                            | Binary::BitAnd
                            | Binary::BitOr
                            | Binary::BitXor
                            | Binary::LeftShift
                            | Binary::RightShift
                    );
                if !self.expression(left, aliases) || !self.expression(right, aliases) {
                    return false;
                }
            }
            ExprKind::BooleanValue(_, left, right, predicate) => {
                if !self.expression(left, aliases)
                    || predicate
                        .as_mut()
                        .is_some_and(|p| !self.expression(&mut p.condition, aliases))
                {
                    return false;
                }
                let mut taken = aliases.to_vec();
                if !self.expression(right, &mut taken) {
                    return false;
                }
                for (slot, taken) in aliases.iter_mut().zip(taken) {
                    if *slot != taken {
                        *slot = None;
                    }
                }
            }
            ExprKind::Conditional(test, yes, no) => {
                if !self.expression(test, aliases) {
                    return false;
                }
                let mut yes_aliases = aliases.to_vec();
                let mut no_aliases = aliases.to_vec();
                let before = self.appends;
                if !self.expression(yes, &mut yes_aliases) {
                    return false;
                }
                let yes_appends = self.appends;
                self.appends = before;
                if !self.expression(no, &mut no_aliases) {
                    return false;
                }
                self.appends = self.appends.max(yes_appends);
                for ((slot, yes), no) in aliases.iter_mut().zip(yes_aliases).zip(no_aliases) {
                    *slot = (yes == no).then_some(yes).flatten();
                }
            }
            ExprKind::Exception { message, .. } => {
                if let Some(value) = message
                    && !self.expression(value, aliases)
                {
                    return false;
                }
            }
            ExprKind::Bind(..)
            | ExprKind::Call(..)
            | ExprKind::Construct { .. }
            | ExprKind::Invoke { .. }
            | ExprKind::Callable { .. } => return false,
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::None
            | ExprKind::Local(_) => {}
        }
        true
    }
}

/// The condition is nonfallible and reads no storage beyond a rooted list's
/// length. Keep the current cursor in the scan budget: an exhausted or nearly
/// exhausted loop must not trigger a full scan of a large source.
fn while_arguments(test: &Expr) -> Option<([Expr; 3], usize)> {
    let ExprKind::Compare(values, operations) = &test.kind else {
        return None;
    };
    if !matches!(operations.as_slice(), [CmpOp::Lt]) {
        return None;
    }
    let [cursor, length] = values.as_slice() else {
        return None;
    };
    if !matches!(cursor.kind, ExprKind::Local(_)) || cursor.ty != Type::Int {
        return None;
    }
    let ExprKind::Builtin(Builtin::Len, args) = &length.kind else {
        return None;
    };
    let [
        Expr {
            ty: Type::ListInt,
            kind: ExprKind::Local(source),
        },
    ] = args.as_slice()
    else {
        return None;
    };
    Some(([cursor.clone(), length.clone(), Expr::integer(1)], *source))
}

/// Repeating these reads cannot allocate, raise, mutate, or call user code.
/// List length is constant time; string length is deliberately excluded.
fn range_arguments(args: &[Expr]) -> Option<[Expr; 3]> {
    if !args.iter().all(|value| {
        matches!(value.kind, ExprKind::Local(_))
            || value.tagged_integer_literal().is_some()
            || matches!(
                &value.kind,
                ExprKind::Builtin(Builtin::Len, args)
                    if matches!(args.as_slice(), [Expr {
                        ty: Type::ListInt,
                        kind: ExprKind::Local(_),
                    }])
            )
    }) {
        return None;
    }
    match args {
        [stop] => Some([Expr::integer(0), stop.clone(), Expr::integer(1)]),
        [start, stop] => Some([start.clone(), stop.clone(), Expr::integer(1)]),
        [start, stop, step] => Some([start.clone(), stop.clone(), step.clone()]),
        _ => None,
    }
}

/// The true arm of `limit - i >= width` covers every offset below the width.
/// The range supplies the lower bound. Only the fast loop can rely on its
/// input buffers staying stable across these reads.
fn prove_range_windows(
    body: &mut [Statement],
    cursor: usize,
    stop: &Expr,
    written: &[bool],
    inputs: &BTreeMap<usize, usize>,
    guards: &mut Vec<Expr>,
) {
    let mut checked_limits = BTreeSet::new();
    crate::ir::rewrite_statements(body, &mut |statement| {
        let Statement::If(test, yes, _) = statement else {
            return;
        };
        let ExprKind::Compare(operands, comparisons) = &test.kind else {
            return;
        };
        let [rest, width] = operands.as_slice() else {
            return;
        };
        let ExprKind::Binary(Binary::Subtract, limit, counter) = &rest.kind else {
            return;
        };
        let Some(width) = width.tagged_integer_literal() else {
            return;
        };
        let width = match comparisons.as_slice() {
            [CmpOp::GtE] => width,
            [CmpOp::Gt] => width + 1,
            _ => return,
        };
        if width <= 0 || !matches!(counter.kind, ExprKind::Local(slot) if slot == cursor) {
            return;
        }
        crate::ir::walk_expressions_mut(yes, &mut |value| {
            let ExprKind::Builtin(op @ Builtin::ListGetI32(_), args) = &mut value.kind else {
                return;
            };
            let [source, index] = args.as_slice() else {
                return;
            };
            let ExprKind::Local(root) = source.kind else {
                return;
            };
            if !inputs.contains_key(&root)
                || !index
                    .local_integer_offset()
                    .is_some_and(|(slot, offset)| slot == cursor && (0..width).contains(&offset))
            {
                return;
            }
            match &limit.kind {
                ExprKind::Builtin(Builtin::Len, items) if matches!(items.as_slice(), [item] if matches!(item.kind, ExprKind::Local(slot) if slot == root)) =>
                    {}
                ExprKind::Local(slot)
                    if !written[*slot]
                        && limit.ty == Type::Int
                        && matches!(stop.kind, ExprKind::Local(bound) if bound == *slot) =>
                {
                    checked_limits.insert(root);
                }
                _ => return,
            }
            *op = Builtin::ListGetBoundedInBounds {
                min: i64::from(i32::MIN),
                max: i64::from(i32::MAX),
            };
        });
    });
    for root in checked_limits {
        let length = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![list_local(root)]),
        };
        guards.push(Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(vec![stop.clone(), length], vec![CmpOp::LtE]),
        });
    }
}

fn alias(expression: &Expr, aliases: &[Option<usize>]) -> Option<usize> {
    match expression.kind {
        ExprKind::Local(slot) if expression.ty == Type::ListInt => aliases[slot],
        _ => None,
    }
}

fn list_local(slot: usize) -> Expr {
    Expr::local(slot, Type::ListInt)
}

fn builtin(operation: Builtin, args: Vec<Expr>) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Builtin(operation, args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::walk_expressions;

    fn optimized(source: &str) -> Module {
        let mut module =
            crate::frontend::lower(&crate::modules::SourceGraph::single(source).unwrap()).unwrap();
        crate::character_locals::specialize(&mut module);
        crate::tuples::scalarize(&mut module);
        specialize(&mut module);
        module
    }

    #[test]
    fn proves_only_dominated_reads_from_stable_range_windows() {
        let source = r#"
def windows(data: list[int], stop: int) -> int:
    total = 0
    for i in range(0, stop, 3):
        if stop - i >= 3:
            total += data[i] + data[1 + i] + data[i + 2]
        else:
            total += data[i]
    return total
print(windows([1, 2, 3, 4], 4))
"#;
        let proven = |source: &str| {
            let module = optimized(source);
            crate::verify::module(&module).unwrap();
            let mut count = 0;
            for function in &module.functions {
                walk_expressions(&function.body, &mut |value| {
                    if matches!(
                        value.kind,
                        ExprKind::Builtin(Builtin::ListGetBoundedInBounds { .. }, _)
                    ) {
                        count += 1;
                    }
                });
            }
            count
        };
        assert_eq!(proven(source), 3);
        assert_eq!(proven(&source.replace("i + 1", "1 + i")), 3);
        assert_eq!(
            proven(&source.replace("stop - i >= 3", "len(data) - i > 2")),
            3
        );
        assert_eq!(proven(&source.replace("data[i + 2]", "data[i + 3]")), 2);
        assert_eq!(
            proven(&source.replace("range(0, stop, 3)", "range(-1, stop, 3)")),
            0
        );
        assert_eq!(
            proven(&source.replace("if stop - i", "i = 2\n        if stop - i")),
            0
        );
        assert_eq!(
            proven(&source.replace("if stop - i", "stop += 1\n        if stop - i")),
            0
        );
    }

    #[test]
    fn certifies_closed_tail_ranges_after_a_scalar_cursor_scan() {
        let source = include_str!("../../../tests/fixtures/iterables/buffered_integer_ranges.py");
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert!(llvm.contains("; inductive list tail -2147483648..=2147483648"));
    }

    #[test]
    fn bounds_integer_updates_after_successful_scalar_pulls() {
        let source = include_str!("../../../tests/fixtures/iterables/scalar_cursor_counters.py");
        let bounds_step_two = |source| {
            let graph = crate::modules::SourceGraph::single(source).unwrap();
            let mut module = crate::frontend::lower(&graph).unwrap();
            crate::specialize(&mut module).unwrap();
            let mut bounded = false;
            for function in &module.functions {
                walk_expressions(&function.body, &mut |value| {
                    bounded |= matches!(value.kind, ExprKind::Builtin(
                        Builtin::BoundedCursorValue { max, .. }, _
                    ) if max == 3 * i64::from(i32::MAX));
                });
            }
            bounded
        };
        assert!(bounds_step_two(source));
        let twice = source.replace(
            "        counter += 2",
            "        counter += 2\n        counter += 1",
        );
        assert!(!bounds_step_two(&twice));
    }

    #[test]
    fn opens_a_spare_append_window_only_for_one_write_per_pull() {
        let source = include_str!("../../../tests/fixtures/iterables/scalar_append_windows.py");
        let uses_spare_window = |source| {
            crate::compile(source, &crate::CompileOptions::default())
                .unwrap()
                .contains("call i32 @imre_list_try_open_append_window(")
        };
        assert!(uses_spare_window(source));
        let twice = source.replace(
            "        out.append(value)",
            "        out.append(value)\n        out.append(value)",
        );
        assert!(!uses_spare_window(&twice));
    }

    #[test]
    fn buffers_only_unobserved_appends_to_one_stable_list() {
        let source = "def fill(data: list[int], out: list[int], text: str) -> None:\n    for c in text:\n        out.append(out[-1] + data[0] + data[1] + ord(c))\nfill([1, 2], [0], 'abc')\n";
        let operations = |source: &str| {
            let module = optimized(source);
            let mut calls = (0, 0, 0);
            for function in &module.functions {
                walk_expressions(&function.body, &mut |value| match value.kind {
                    ExprKind::Builtin(Builtin::AppendInWindow(_), _) => calls.0 += 1,
                    ExprKind::Builtin(Builtin::GetInAppendWindow(..), _) => calls.1 += 1,
                    ExprKind::Builtin(Builtin::FinishListAppends(_), _) => calls.2 += 1,
                    _ => {}
                });
            }
            calls
        };
        assert_eq!(operations(source), (1, 1, 1));
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        let (_, after_call) = llvm
            .split_once(" = call i64 @get_in_append_window(")
            .unwrap();
        assert!(
            after_call
                .lines()
                .nth(1)
                .unwrap()
                .contains(" = icmp eq i64 ")
        );

        for observed in ["out[-2]", "out[-1] + len(out)", "out[-1] + len(data)"] {
            assert_eq!(operations(&source.replace("out[-1]", observed)), (0, 0, 0));
        }
    }

    fn counts(module: &Module) -> (usize, usize, usize) {
        let mut result = (0, 0, 0);
        for function in module.functions.iter().chain([&module.entry]) {
            walk_expressions(&function.body, &mut |value| match &value.kind {
                ExprKind::Builtin(Builtin::ListAllI32, args) => {
                    assert_eq!(args.len(), 4);
                    result.0 += 1;
                }
                ExprKind::Builtin(Builtin::ListDistinct, _) => result.1 += 1,
                ExprKind::Builtin(Builtin::ListGetI32(_), _) => result.2 += 1,
                _ => {}
            });
        }
        result
    }

    #[test]
    fn only_stable_inputs_borrow_words_in_the_loop_body() {
        let module = optimized(
            r#"
def fill(source: list[int], output: list[int], text: str) -> None:
    for ch in text:
        alias = source
        output.append(alias[0] + source[1] + output[-1] + ord(ch))
    else:
        source.append(42)
        print(source[0])
fill([1, 2], [0], "abc")
"#,
        );
        let mut present = 0;
        let mut reads = 0;
        for function in &module.functions {
            walk_expressions(&function.body, &mut |expr| match &expr.kind {
                ExprKind::Builtin(Builtin::ListPresent, _) => present += 1,
                ExprKind::Builtin(Builtin::ListGetStable(_), _) => reads += 1,
                _ => {}
            });
        }
        assert_eq!((present, reads), (1, 2));
        assert_eq!(counts(&module), (0, 1, 0));
    }

    #[test]
    fn finds_loops_through_nested_expression_and_statement_scopes() {
        for has_loop in [false, true] {
            let none = || Expr {
                ty: Type::None,
                kind: ExprKind::None,
            };
            let mut value = Expr {
                ty: Type::None,
                kind: ExprKind::Scope {
                    body: if has_loop {
                        vec![Statement::While(
                            Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Bool(false),
                            },
                            Vec::new(),
                            Vec::new(),
                        )]
                    } else {
                        Vec::new()
                    },
                    value: Box::new(none()),
                    clear: Vec::new(),
                },
            };
            for depth in 0..64 {
                let (body, result) = if depth % 2 == 0 {
                    (
                        vec![Statement::Scope {
                            body: vec![Statement::Expr(value)],
                            clear: Vec::new(),
                        }],
                        none(),
                    )
                } else {
                    (Vec::new(), value)
                };
                value = Expr {
                    ty: Type::None,
                    kind: ExprKind::Scope {
                        body,
                        value: Box::new(result),
                        clear: Vec::new(),
                    },
                };
            }
            assert_eq!(contains_loop(&[Statement::Expr(value)]), has_loop);
        }
    }

    #[test]
    fn versions_triples_with_hidden_extend_receiver_and_retains_fallback() {
        let module = optimized(
            r#"
def triples(data: list[int], output: list[int]) -> None:
    for i in range(0, len(data), 3):
        value = data[i] + data[i + 1] * 45 + data[i + 2] * 2025
        output.extend(divmod(value, 256))
"#,
        );
        assert_eq!(counts(&module), (1, 1, 3));
        let mut fallback_reads = 0;
        walk_expressions(&module.functions[0].body, &mut |value| {
            if matches!(value.kind, ExprKind::Builtin(Builtin::ListGet(_), _)) {
                fallback_reads += 1;
            }
        });
        assert_eq!(fallback_reads, 3);
    }

    #[test]
    fn versions_arithmetic_list_comprehension_inside_expression_scope() {
        let module = optimized(
            r#"
def transform(data: list[int]) -> list[int]:
    return [sum(divmod(2 * value, 10)) for value in data]
"#,
        );
        assert_eq!(counts(&module), (1, 1, 1));
        let mut fallback_reads = 0;
        let mut reservations = 0;
        for function in &module.functions {
            walk_expressions(&function.body, &mut |value| match &value.kind {
                ExprKind::Builtin(Builtin::ListGet(_), _) => fallback_reads += 1,
                ExprKind::Builtin(Builtin::ListReserveRange, args) => {
                    assert!(matches!(&args[4].kind, ExprKind::Int(value) if value == "1"));
                    reservations += 1;
                }
                _ => {}
            });
        }
        assert_eq!(fallback_reads, 1);
        assert_eq!(reservations, 1);
    }

    #[test]
    fn while_scan_budget_uses_the_current_cursor_and_preserves_checked_access() {
        let module = optimized(
            r#"
def transform(data: list[int], cursor: int, output: list[int]) -> None:
    while cursor < len(data):
        output.append(data[cursor] * 2)
        cursor += 1
"#,
        );
        assert_eq!(counts(&module), (1, 1, 1));
        let mut guards = 0;
        walk_expressions(&module.functions[0].body, &mut |value| {
            if let ExprKind::Builtin(Builtin::ListAllI32, args) = &value.kind {
                assert!(matches!(args[0].kind, ExprKind::Local(0)));
                assert!(matches!(args[1].kind, ExprKind::Local(1)));
                assert!(matches!(args[2].kind, ExprKind::Builtin(Builtin::Len, _)));
                assert!(matches!(&args[3].kind, ExprKind::Int(value) if value == "1"));
                guards += 1;
            }
        });
        assert_eq!(guards, 1);
        let Some(Statement::If(_, fast, fallback)) = module.functions[0]
            .body
            .iter()
            .find(|s| matches!(s, Statement::If(..)))
        else {
            panic!("missing guarded loop");
        };
        let [
            Statement::Assign(slot, length),
            Statement::Expr(reserve),
            Statement::While(test, _, _),
        ] = fast.as_slice()
        else {
            panic!("missing fast loop preheader");
        };
        assert!(matches!(length.kind, ExprKind::Builtin(Builtin::Len, _)));
        assert!(
            matches!(&reserve.kind, ExprKind::Builtin(Builtin::ListReserveRange, args)
            if matches!(args[2].kind, ExprKind::Local(i) if i == *slot))
        );
        assert!(matches!(&test.kind, ExprKind::Compare(args, _)
            if matches!(args[1].kind, ExprKind::Local(i) if i == *slot)));
        assert!(matches!(fallback.as_slice(), [Statement::While(test, _, _)]
            if while_arguments(test).is_some()));
        let llvm = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert!(llvm.contains("call i64 @imre_list_get("));
    }

    #[test]
    fn while_versioning_rejects_trivial_loops_callbacks_and_unstable_sources() {
        for source in [
            r#"
def copy(data: list[int]) -> list[int]:
    return [value for value in data]
"#,
            r#"
def total(data: list[int]) -> int:
    return sum(data)
"#,
            r#"
def touch(data: list[int]) -> None:
    data.append(1)

def transform(data: list[int], output: list[int]) -> None:
    for value in data:
        touch(data)
        output.append(value * 2)
"#,
            r#"
def transform(data: list[int], output: list[int]) -> None:
    cursor = 0
    while cursor < len(data):
        output.append(data[cursor] * 2)
        data = output
        cursor += 1
"#,
            r#"
def transform(data: list[int]) -> None:
    cursor = 0
    while cursor < len(data):
        data[cursor] = data[cursor] * 2
        cursor += 1
"#,
        ] {
            assert_eq!(counts(&optimized(source)), (0, 0, 0));
        }
    }

    #[test]
    fn nested_list_loops_only_version_the_leaf() {
        let module = optimized(
            r#"
def transform(data: list[int], output: list[int]) -> None:
    for first in data:
        for second in data:
            output.append(sum(divmod(first + second, 10)))
"#,
        );
        assert_eq!(counts(&module), (1, 1, 1));
    }

    #[test]
    fn rejects_unknown_aliases_mutation_and_calls() {
        for source in [
            r#"
def scan(data: list[int]) -> None:
    for i in range(0, len(data), 2):
        data.append(data[i] + data[i + 1])
"#,
            r#"
def scan(data: list[int], output: list[int], choice: bool) -> None:
    for i in range(0, len(data), 2):
        if choice:
            destination = output
        else:
            destination = data
        destination.append(data[i] + data[i + 1])
"#,
            r#"
def touch(data: list[int]) -> None:
    data.append(1)

def scan(data: list[int], output: list[int]) -> None:
    for i in range(0, len(data), 2):
        touch(data)
        output.append(data[i] + data[i + 1])
"#,
            r#"
def scan(data: list[int], output: list[int]) -> None:
    for i in range(0, len(data), 2):
        data = output
        output.append(data[i] + data[i + 1])
"#,
        ] {
            assert_eq!(counts(&optimized(source)), (0, 0, 0));
        }
    }

    #[test]
    fn guards_each_input_against_known_destination_aliases() {
        let module = optimized(
            r#"
def scan(first: list[int], second: list[int], output: list[int]) -> None:
    for i in range(len(first)):
        destination = output
        destination.append(first[i] + second[i])
"#,
        );
        assert_eq!(counts(&module), (2, 2, 2));
    }

    #[test]
    fn reserves_the_maximum_append_count_of_exclusive_arms() {
        for (tail, expected) in [
            (
                "        if choice:\n            output.append(first)\n            output.append(second)\n        else:\n            output.append(first + second)\n",
                vec!["2"],
            ),
            (
                "        output.append(first) if choice else output.append(second)\n",
                vec!["1"],
            ),
            (
                "        output.append(first)\n        extra.append(second)\n",
                vec![],
            ),
        ] {
            let source = format!(
                "def scan(data: list[int], output: list[int], extra: list[int], choice: bool) -> None:\n    for i in range(len(data)):\n        first = data[i]\n        second = data[i]\n{tail}"
            );
            let module = optimized(&source);
            let mut hints = Vec::new();
            walk_expressions(&module.functions[0].body, &mut |value| {
                if let ExprKind::Builtin(Builtin::ListReserveRange, args) = &value.kind {
                    assert_eq!(args.len(), 5);
                    let ExprKind::Int(count) = &args[4].kind else {
                        panic!("the capacity hint must use a constant append bound")
                    };
                    hints.push(count.clone());
                }
            });
            assert_eq!(hints, expected);
        }
    }
}
