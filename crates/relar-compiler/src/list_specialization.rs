//! Checked loop versions for bounded integer elements in stable input lists.
//!
//! An entry guard scans each input once. Fast-loop reads retain their ordinary
//! index checks, but carry an i32 value proof into integer interval analysis.
//! Inputs cannot be rebound or mutated in the loop. Appends to other lists are
//! permitted only after a runtime identity check excludes aliases of every input.

use std::collections::BTreeSet;

use crate::ir::{
    Binary, Builtin, Compare, Expr, ExprKind, Module, Statement, Type, walk_expressions,
};

/// Versions leaf range and canonical list loops after tuple scalarization.
/// Unknown calls and mutations retain the original arbitrary-precision path.
pub(crate) fn specialize(module: &mut Module) {
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        if function.expression_rebound().into_iter().any(|value| value) {
            continue;
        }
        rewrite_block(&mut function.body, &function.locals);
    }
}

/// Rewrites children before considering a loop, without revisiting new versions.
fn rewrite_block(body: &mut [Statement], locals: &[Type]) {
    for statement in body {
        match statement {
            Statement::Assign(_, value)
            | Statement::Expr(value)
            | Statement::Return(value)
            | Statement::Assert(value) => rewrite_expression(value, locals),
            Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    rewrite_expression(value, locals);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                rewrite_expression(a, locals);
                rewrite_expression(b, locals);
            }
            Statement::Scope { body, .. } => rewrite_block(body, locals),
            Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
                rewrite_expression(test, locals);
                rewrite_block(yes, locals);
                rewrite_block(no, locals);
            }
            Statement::For {
                args, body, orelse, ..
            } => {
                for arg in args {
                    rewrite_expression(arg, locals);
                }
                rewrite_block(body, locals);
                rewrite_block(orelse, locals);
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                rewrite_block(body, locals);
                for handler in handlers {
                    rewrite_block(&mut handler.body, locals);
                }
                rewrite_block(orelse, locals);
                rewrite_block(finalbody, locals);
            }
            Statement::Raise(Some(value)) => rewrite_expression(value, locals),
            _ => {}
        }
        let (arguments, body, orelse, while_source) = match statement {
            Statement::For {
                args, body, orelse, ..
            } => (range_arguments(args), body, orelse, None),
            Statement::While(test, body, orelse) => {
                let Some((args, source)) = while_arguments(test) else {
                    continue;
                };
                (Some(args), body, orelse, Some(source))
            }
            _ => continue,
        };
        // The guard also rejects scans whose cost cannot be amortized by the
        // captured range. Only cheap, nonfallible operands may be read twice.
        let Some([start, stop, step]) = arguments else {
            continue;
        };
        if contains_loop(orelse) {
            continue;
        }
        let mut written = vec![false; locals.len()];
        mark_writes(body, &mut written);
        walk_expressions(body, &mut |expression| {
            if let ExprKind::Scope { body, clear, .. } = &expression.kind {
                mark_writes(body, &mut written);
                for &slot in clear {
                    written[slot] = true;
                }
            }
        });
        if while_source.is_some_and(|source| written[source]) {
            continue;
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
            inputs: BTreeSet::new(),
            outputs: BTreeSet::new(),
            reads: 0,
            appends: 0,
            nontrivial_arithmetic: false,
        };
        if !plan.block(body, &mut aliases)
            || (plan.reads < 2
                && !(while_source.is_some() && plan.reads == 1 && plan.nontrivial_arithmetic))
            || while_source.is_some_and(|source| !plan.inputs.contains(&source))
        {
            continue;
        }
        if plan.inputs.iter().any(|slot| plan.outputs.contains(slot)) {
            continue;
        }
        let mut guards = Vec::new();
        for &input in &plan.inputs {
            for &output in &plan.outputs {
                guards.push(builtin(
                    Builtin::ListDistinct,
                    vec![list_local(input), list_local(output)],
                ));
            }
            guards.push(builtin(
                Builtin::ListAllI32,
                vec![list_local(input), start.clone(), stop.clone(), step.clone()],
            ));
        }
        let mut fast_body = Vec::new();
        if plan.outputs.len() == 1 {
            // Hints beyond the runtime's slot budget cannot reserve more space.
            let appends = plan.appends.min(32_768) as i64;
            fast_body.push(Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::ListReserveRange,
                    vec![
                        list_local(*plan.outputs.first().unwrap()),
                        start,
                        stop,
                        step,
                        integer(appends),
                    ],
                ),
            }));
        }
        fast_body.push(fast);
        let fallback = std::mem::replace(statement, Statement::Break);
        *statement = Statement::If(
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(true, guards),
            },
            fast_body,
            vec![fallback],
        );
    }
}

/// Visits each eager expression once, descending into its own statement scopes.
/// Newly generated loop versions are not revisited by this child-first walk.
fn rewrite_expression(expression: &mut Expr, locals: &[Type]) {
    match &mut expression.kind {
        ExprKind::Scope { body, value, .. } => {
            rewrite_block(body, locals);
            rewrite_expression(value, locals);
        }
        ExprKind::List(values)
        | ExprKind::Tuple(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Builtin(_, values) => {
            for value in values {
                rewrite_expression(value, locals);
            }
        }
        ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
            for value in &mut args.values {
                rewrite_expression(value, locals);
            }
        }
        ExprKind::Dict { entries, .. } => {
            for (key, value) in entries {
                rewrite_expression(key, locals);
                rewrite_expression(value, locals);
            }
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => rewrite_expression(value, locals),
        ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
            rewrite_expression(left, locals);
            rewrite_expression(right, locals);
        }
        ExprKind::Conditional(test, yes, no) => {
            rewrite_expression(test, locals);
            rewrite_expression(yes, locals);
            rewrite_expression(no, locals);
        }
        ExprKind::Exception { message, .. } => {
            if let Some(message) = message {
                rewrite_expression(message, locals);
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

/// An else suite is cloned with the loop, so nested loops there would multiply
/// versions even though they are outside the input-stability proof's body.
fn contains_loop(body: &[Statement]) -> bool {
    let mut found = statements_contain_loop(body);
    walk_expressions(body, &mut |value| {
        if let ExprKind::Scope { body, .. } = &value.kind {
            found |= statements_contain_loop(body);
        }
    });
    found
}

/// Checks each statement region without entering expression scopes. The shared
/// expression walk visits those scopes once, including their nested expressions.
fn statements_contain_loop(body: &[Statement]) -> bool {
    body.iter().any(|statement| match statement {
        Statement::For { .. } | Statement::While(..) => true,
        Statement::Scope { body, .. } => statements_contain_loop(body),
        Statement::If(_, yes, no) => statements_contain_loop(yes) || statements_contain_loop(no),
        Statement::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            statements_contain_loop(body)
                || handlers
                    .iter()
                    .any(|handler| statements_contain_loop(&handler.body))
                || statements_contain_loop(orelse)
                || statements_contain_loop(finalbody)
        }
        _ => false,
    })
}

/// Marks all statement-owned writes, including cleanup on abrupt scope exits.
/// Embedded expression scopes are visited separately by the shared IR walker.
fn mark_writes(body: &[Statement], written: &mut [bool]) {
    for statement in body {
        match statement {
            Statement::Assign(slot, _) | Statement::Clear(slot) => written[*slot] = true,
            Statement::Move(target, source) => {
                written[*target] = true;
                written[*source] = true;
            }
            Statement::Scope { body, clear } => {
                mark_writes(body, written);
                for &slot in clear {
                    written[slot] = true;
                }
            }
            Statement::If(_, yes, no) | Statement::While(_, yes, no) => {
                mark_writes(yes, written);
                mark_writes(no, written);
            }
            Statement::For {
                target,
                cursor,
                stop,
                step,
                body,
                orelse,
                ..
            } => {
                for &slot in [target, cursor, stop, step] {
                    written[slot] = true;
                }
                mark_writes(body, written);
                mark_writes(orelse, written);
            }
            Statement::IterNext {
                key, value, ready, ..
            } => {
                for slot in [Some(*key), *value, Some(*ready)].into_iter().flatten() {
                    written[slot] = true;
                }
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                mark_writes(body, written);
                for handler in handlers {
                    if let Some(slot) = handler.name {
                        written[slot] = true;
                    }
                    mark_writes(&handler.body, written);
                }
                mark_writes(orelse, written);
                mark_writes(finalbody, written);
            }
            _ => {}
        }
    }
}

struct Plan<'a> {
    locals: &'a [Type],
    inputs: BTreeSet<usize>,
    outputs: BTreeSet<usize>,
    reads: usize,
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
                Statement::Expr(value) | Statement::Return(value) | Statement::Assert(value) => {
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
                        && !self.expression(value, aliases)
                    {
                        return false;
                    }
                }
                Statement::Break | Statement::Continue | Statement::Exhaust => {}
                Statement::SetItem(..)
                | Statement::AugmentItem(..)
                | Statement::SetField(..)
                | Statement::AugmentField(..)
                | Statement::While(..)
                | Statement::For { .. }
                | Statement::IterNext { .. }
                | Statement::Try { .. } => return false,
            }
        }
        true
    }

    /// Appends are the only permitted mutation. Capturing their receiver's root
    /// before later operands preserves the source language's evaluation order.
    fn expression(&mut self, expression: &mut Expr, aliases: &mut [Option<usize>]) -> bool {
        match &mut expression.kind {
            ExprKind::Builtin(operation, args) => {
                let receiver = args.first().and_then(|value| alias(value, aliases));
                if !args.iter_mut().all(|value| self.expression(value, aliases)) {
                    return false;
                }
                match operation {
                    Builtin::ListGet if args[0].ty == Type::ListInt => {
                        let Some(root) = receiver else {
                            return false;
                        };
                        self.inputs.insert(root);
                        self.reads += 1;
                        *operation = Builtin::ListGetI32;
                    }
                    Builtin::Append if args[0].ty == Type::ListInt => {
                        let Some(root) = receiver else {
                            return false;
                        };
                        self.outputs.insert(root);
                        self.appends += 1;
                    }
                    operation if readonly_builtin(operation) => {}
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
            ExprKind::Dict { entries, .. } => {
                if !entries.iter_mut().all(|(key, value)| {
                    self.expression(key, aliases) && self.expression(value, aliases)
                }) {
                    return false;
                }
            }
            ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Not(value)
            | ExprKind::Negate(value)
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
            ExprKind::BooleanValue(_, left, right) => {
                if !self.expression(left, aliases) {
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
            ExprKind::Bind(..) | ExprKind::Call(..) | ExprKind::Construct { .. } => return false,
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

/// These closed operations neither call user code nor mutate existing containers.
fn readonly_builtin(operation: &Builtin) -> bool {
    matches!(
        operation,
        Builtin::GlobalGet(_)
            | Builtin::Int
            | Builtin::Float
            | Builtin::Bool
            | Builtin::Str
            | Builtin::Len
            | Builtin::ListGet
            | Builtin::StringGet
            | Builtin::Bytes
            | Builtin::BytesGet
            | Builtin::Encode
            | Builtin::Decode
            | Builtin::Slice
            | Builtin::IsNone
            | Builtin::UnionInject
            | Builtin::UnionProject
            | Builtin::IsInstance(_)
            | Builtin::Split
            | Builtin::SplitLines
            | Builtin::RStrip
            | Builtin::DictGet
            | Builtin::DictGetDefault
            | Builtin::DictTryGet(_)
            | Builtin::DictContains
            | Builtin::SetContains
            | Builtin::ExceptionMessage
            | Builtin::ExceptionCause
            | Builtin::ExceptionSuppressContext
    )
}

/// The condition is nonfallible and reads no storage beyond a rooted list's
/// length. Keep the current cursor in the scan budget: an exhausted or nearly
/// exhausted loop must not trigger a full scan of a large source.
fn while_arguments(test: &Expr) -> Option<([Expr; 3], usize)> {
    let ExprKind::Compare(values, operations) = &test.kind else {
        return None;
    };
    if !matches!(operations.as_slice(), [Compare::Less]) {
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
    Some(([cursor.clone(), length.clone(), integer(1)], *source))
}

/// Repeating these reads cannot allocate, raise, mutate, or call user code.
/// List length is constant time; string length is deliberately excluded.
fn range_arguments(args: &[Expr]) -> Option<[Expr; 3]> {
    if !args.iter().all(|value| {
        matches!(value.kind, ExprKind::Local(_))
            || small_literal(value).is_some()
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
        [stop] => Some([integer(0), stop.clone(), integer(1)]),
        [start, stop] => Some([start.clone(), stop.clone(), integer(1)]),
        [start, stop, step] => Some([start.clone(), stop.clone(), step.clone()]),
        _ => None,
    }
}

fn small_literal(value: &Expr) -> Option<i64> {
    let value = match &value.kind {
        ExprKind::Int(value) => value.parse::<i64>().ok()?,
        ExprKind::Negate(value) => small_literal(value)?.checked_neg()?,
        _ => return None,
    };
    (-(1_i64 << 62)..(1_i64 << 62))
        .contains(&value)
        .then_some(value)
}

fn integer(value: i64) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

fn alias(expression: &Expr, aliases: &[Option<usize>]) -> Option<usize> {
    match expression.kind {
        ExprKind::Local(slot) if expression.ty == Type::ListInt => aliases[slot],
        _ => None,
    }
}

fn list_local(slot: usize) -> Expr {
    Expr {
        ty: Type::ListInt,
        kind: ExprKind::Local(slot),
    }
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

    fn optimized(source: &str) -> Module {
        let mut module =
            crate::frontend::lower(&crate::modules::SourceGraph::single(source).unwrap()).unwrap();
        crate::tuples::scalarize(&mut module);
        specialize(&mut module);
        module
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
                ExprKind::Builtin(Builtin::ListGetI32, _) => result.2 += 1,
                _ => {}
            });
        }
        result
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
            if matches!(value.kind, ExprKind::Builtin(Builtin::ListGet, _)) {
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
                ExprKind::Builtin(Builtin::ListGet, _) => fallback_reads += 1,
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
        let llvm = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false);
        assert!(llvm.contains("call i64 @relar_list_get("));
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
