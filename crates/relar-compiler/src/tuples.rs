//! Scalar replacement for tuple locals whose identity and aggregate never escape.
//!
//! Each component is an ordinary owned local. Arguments and elements are evaluated
//! once in their original order, into owned temporaries, before any old component
//! is replaced. This preserves snapshots of mutable fields and aliases of mutable
//! elements. Tuples used as whole values keep the existing boxed representation.

use std::collections::HashMap;

use crate::ir::{Builtin, Expr, ExprKind, Function, Module, Statement, Type};

struct Producer {
    params: Vec<usize>,
    elements: Vec<Expr>,
}

struct Candidate {
    eligible: bool,
    fields: Option<Vec<Type>>,
}

/// Optimizes only projection/length-only locals built by literals or a function
/// consisting solely of `return (elements...)`. Producer functions and every
/// escaping tuple retain their original signatures and boxed runtime ABI.
pub(crate) fn scalarize(module: &mut Module) {
    let producers = module
        .functions
        .iter()
        .map(|function| {
            let [
                Statement::Return(Expr {
                    kind: ExprKind::Tuple(elements),
                    ..
                }),
            ] = function.body.as_slice()
            else {
                return None;
            };
            if !elements
                .iter()
                .all(|element| only_parameters(element, &function.params))
            {
                return None;
            }
            Some(Producer {
                params: function.params.clone(),
                elements: elements.clone(),
            })
        })
        .collect::<Vec<_>>();
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        scalarize_function(function, &producers);
    }
}

/// Producer expressions may refer only to their own parameters. This guards the
/// substitution independently of how frontend lowering allocates local slots.
fn only_parameters(expression: &Expr, params: &[usize]) -> bool {
    if let ExprKind::Local(slot) = expression.kind {
        return params.contains(&slot);
    }
    let mut result = true;
    children(expression, |child| result &= only_parameters(child, params));
    result
}

fn scalarize_function(function: &mut Function, producers: &[Option<Producer>]) {
    let mut candidates = function
        .locals
        .iter()
        .enumerate()
        .map(|(slot, ty)| Candidate {
            eligible: matches!(ty, Type::Tuple(_)) && !function.params.contains(&slot),
            fields: None,
        })
        .collect::<Vec<_>>();
    inspect(&function.body, &mut candidates, producers);
    let mut components = vec![None; candidates.len()];
    for (slot, candidate) in candidates.into_iter().enumerate() {
        if candidate.eligible
            && let Some(fields) = candidate.fields
        {
            components[slot] = Some(
                fields
                    .into_iter()
                    .map(|ty| new_local(&mut function.locals, ty))
                    .collect::<Vec<_>>(),
            );
            // Every use/assignment will be rewritten. No unused boxed owner is
            // needed in the original slot, including on early cleanup paths.
            function.locals[slot] = Type::None;
        }
    }
    if components.iter().any(Option::is_some) {
        rewrite_block(
            &mut function.body,
            &mut function.locals,
            &components,
            producers,
        );
    }
}

/// A local is eligible only when every assignment has a known tuple shape and
/// every read is a direct field projection or len; whole-value aliases escape.
fn inspect(statements: &[Statement], candidates: &mut [Candidate], producers: &[Option<Producer>]) {
    for statement in statements {
        match statement {
            Statement::Assign(slot, value) => {
                let elements = match &value.kind {
                    ExprKind::Tuple(elements) => Some(elements.as_slice()),
                    ExprKind::Call(id, _) => producers[*id]
                        .as_ref()
                        .map(|producer| producer.elements.as_slice()),
                    _ => None,
                };
                if let Some(elements) = elements {
                    let fields = elements.iter().map(|value| value.ty).collect::<Vec<_>>();
                    let candidate = &mut candidates[*slot];
                    if candidate.fields.as_ref().is_some_and(|old| old != &fields) {
                        candidate.eligible = false;
                    }
                    candidate.fields = Some(fields);
                } else {
                    candidates[*slot].eligible = false;
                }
                inspect_expression(value, candidates);
            }
            Statement::Move(target, source) => {
                candidates[*target].eligible = false;
                candidates[*source].eligible = false;
            }
            Statement::IterNext {
                iterator,
                key,
                value,
                ready,
            } => {
                // Runtime iteration transfers whole values, including boxed
                // tuple payloads whose components are not known statically.
                for slot in [Some(*iterator), Some(*key), *value, Some(*ready)]
                    .into_iter()
                    .flatten()
                {
                    candidates[slot].eligible = false;
                }
            }
            // Clearing the aggregate releases exactly the component owners.
            // It neither observes tuple identity nor lets the aggregate escape.
            Statement::Clear(_) => {}
            Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    inspect_expression(value, candidates);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                inspect_expression(a, candidates);
                inspect_expression(b, candidates);
            }
            Statement::Expr(value) | Statement::Return(value) | Statement::Assert(value) => {
                inspect_expression(value, candidates);
            }
            Statement::If(condition, yes, no) => {
                inspect_expression(condition, candidates);
                inspect(yes, candidates, producers);
                inspect(no, candidates, producers);
            }
            Statement::While(condition, body) => {
                inspect_expression(condition, candidates);
                inspect(body, candidates, producers);
            }
            Statement::Scope { body, .. } => inspect(body, candidates, producers),
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody] {
                    inspect(body, candidates, producers);
                }
                for handler in handlers {
                    inspect(&handler.body, candidates, producers);
                }
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    inspect_expression(value, candidates);
                }
            }
            Statement::For {
                target, args, body, ..
            } => {
                candidates[*target].eligible = false;
                for arg in args {
                    inspect_expression(arg, candidates);
                }
                inspect(body, candidates, producers);
            }
            Statement::Break | Statement::Continue => {}
        }
    }
}

fn inspect_expression(expression: &Expr, candidates: &mut [Candidate]) {
    match &expression.kind {
        ExprKind::Local(slot) => candidates[*slot].eligible = false,
        ExprKind::Field(receiver, _) if matches!(receiver.kind, ExprKind::Local(_)) => {}
        ExprKind::Builtin(Builtin::Len, args)
            if matches!(
                args.as_slice(),
                [Expr {
                    kind: ExprKind::Local(_),
                    ..
                }]
            ) => {}
        _ => children(expression, |child| inspect_expression(child, candidates)),
    }
}

fn new_local(locals: &mut Vec<Type>, ty: Type) -> usize {
    let slot = locals.len();
    locals.push(ty);
    slot
}

/// Stages the complete RHS before publishing any component. Argument temporaries
/// stay rooted through all element evaluations; moves consume staging owners, so
/// neither loop backedges nor function cleanup retain an extra temporary owner.
fn expand_assignment(
    value: Expr,
    targets: &[usize],
    locals: &mut Vec<Type>,
    producers: &[Option<Producer>],
    output: &mut Vec<Statement>,
) {
    let mut staged_output = Vec::new();
    let mut arguments = Vec::new();
    let elements = match value.kind {
        ExprKind::Tuple(elements) => elements,
        ExprKind::Call(id, args) => {
            let producer = producers[id].as_ref().expect("checked tuple producer");
            debug_assert_eq!(producer.params.len(), args.order.len());
            for arg in args.values {
                let slot = new_local(locals, arg.ty);
                arguments.push(slot);
                staged_output.push(Statement::Assign(slot, arg));
            }
            let bindings = producer
                .params
                .iter()
                .zip(args.order)
                .map(|(&parameter, index)| (parameter, arguments[index]))
                .collect();
            let mut elements = producer.elements.clone();
            for element in &mut elements {
                substitute_parameters(element, &bindings);
            }
            elements
        }
        _ => unreachable!("candidate assignments have a known tuple construction"),
    };
    debug_assert_eq!(targets.len(), elements.len());
    let mut staged = Vec::new();
    for element in elements {
        let slot = new_local(locals, element.ty);
        staged.push(slot);
        staged_output.push(Statement::Assign(slot, element));
    }
    staged_output.extend(arguments.iter().copied().map(Statement::Clear));
    for (&target, &source) in targets.iter().zip(&staged) {
        staged_output.push(Statement::Move(target, source));
    }
    arguments.extend(staged);
    output.push(Statement::Scope {
        body: staged_output,
        clear: arguments,
    });
}

fn substitute_parameters(expression: &mut Expr, bindings: &HashMap<usize, usize>) {
    if let ExprKind::Local(slot) = &mut expression.kind {
        *slot = bindings[slot];
    } else {
        children_mut(expression, |child| substitute_parameters(child, bindings));
    }
}

fn rewrite_block(
    statements: &mut Vec<Statement>,
    locals: &mut Vec<Type>,
    components: &[Option<Vec<usize>>],
    producers: &[Option<Producer>],
) {
    let mut output = Vec::new();
    for mut statement in std::mem::take(statements) {
        match &mut statement {
            Statement::Assign(slot, value) => {
                rewrite_expression(value, components);
                if let Some(targets) = &components[*slot] {
                    let Statement::Assign(_, value) = statement else {
                        unreachable!()
                    };
                    expand_assignment(value, targets, locals, producers, &mut output);
                    continue;
                }
            }
            Statement::Clear(slot) => {
                if let Some(fields) = &components[*slot] {
                    output.extend(fields.iter().copied().map(Statement::Clear));
                    continue;
                }
            }
            Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    rewrite_expression(value, components);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                rewrite_expression(a, components);
                rewrite_expression(b, components);
            }
            Statement::Expr(value) | Statement::Return(value) | Statement::Assert(value) => {
                rewrite_expression(value, components);
            }
            Statement::If(condition, yes, no) => {
                rewrite_expression(condition, components);
                rewrite_block(yes, locals, components, producers);
                rewrite_block(no, locals, components, producers);
            }
            Statement::While(condition, body) => {
                rewrite_expression(condition, components);
                rewrite_block(body, locals, components, producers);
            }
            Statement::Scope { body, clear } => {
                rewrite_block(body, locals, components, producers);
                *clear = clear
                    .iter()
                    .flat_map(|&slot| components[slot].clone().unwrap_or_else(|| vec![slot]))
                    .collect();
            }
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody] {
                    rewrite_block(body, locals, components, producers);
                }
                for handler in handlers {
                    rewrite_block(&mut handler.body, locals, components, producers);
                }
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    rewrite_expression(value, components);
                }
            }
            Statement::For { args, body, .. } => {
                for arg in args {
                    rewrite_expression(arg, components);
                }
                rewrite_block(body, locals, components, producers);
            }
            Statement::Move(..)
            | Statement::IterNext { .. }
            | Statement::Break
            | Statement::Continue => {}
        }
        output.push(statement);
    }
    *statements = output;
}

fn rewrite_expression(expression: &mut Expr, components: &[Option<Vec<usize>>]) {
    let replacement = match &expression.kind {
        ExprKind::Field(receiver, field) => {
            if let ExprKind::Local(slot) = receiver.kind {
                components
                    .get(slot)
                    .and_then(Option::as_ref)
                    .map(|fields| ExprKind::Local(fields[*field]))
            } else {
                None
            }
        }
        ExprKind::Builtin(Builtin::Len, args) => {
            if let [
                Expr {
                    kind: ExprKind::Local(slot),
                    ..
                },
            ] = args.as_slice()
            {
                components
                    .get(*slot)
                    .and_then(Option::as_ref)
                    .map(|fields| ExprKind::Int(fields.len().to_string()))
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(kind) = replacement {
        expression.kind = kind;
    } else {
        children_mut(expression, |child| rewrite_expression(child, components));
    }
}

fn children(expression: &Expr, mut visit: impl FnMut(&Expr)) {
    match &expression.kind {
        ExprKind::Exception { message, .. } => {
            if let Some(message) = message {
                visit(message);
            }
        }
        ExprKind::Dict { entries, .. } => {
            for (key, value) in entries {
                visit(key);
                visit(value);
            }
        }
        ExprKind::List(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Tuple(values)
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Builtin(_, values) => {
            for value in values {
                visit(value);
            }
        }
        ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
            for value in &args.values {
                visit(value);
            }
        }
        ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => visit(value),
        ExprKind::Binary(_, left, right) => {
            visit(left);
            visit(right);
        }
        ExprKind::Int(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Local(_) => {}
    }
}

fn children_mut(expression: &mut Expr, mut visit: impl FnMut(&mut Expr)) {
    match &mut expression.kind {
        ExprKind::Exception { message, .. } => {
            if let Some(message) = message {
                visit(message);
            }
        }
        ExprKind::Dict { entries, .. } => {
            for (key, value) in entries {
                visit(key);
                visit(value);
            }
        }
        ExprKind::List(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Tuple(values)
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Builtin(_, values) => {
            for value in values {
                visit(value);
            }
        }
        ExprKind::Call(_, args) | ExprKind::Construct { args, .. } => {
            for value in &mut args.values {
                visit(value);
            }
        }
        ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => visit(value),
        ExprKind::Binary(_, left, right) => {
            visit(left);
            visit(right);
        }
        ExprKind::Int(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Local(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordered_producer_arguments_keep_evaluation_order_and_parameter_types() {
        let mut locals = vec![Type::Int, Type::Str];
        let producer = Producer {
            params: vec![5, 2],
            elements: vec![
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Local(5),
                },
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Local(2),
                },
            ],
        };
        let value = Expr {
            ty: Type::Tuple(0),
            kind: ExprKind::Call(
                0,
                crate::ir::CallArgs {
                    values: vec![
                        Expr {
                            ty: Type::Str,
                            kind: ExprKind::Str("first expression".into()),
                        },
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("7".into()),
                        },
                    ],
                    order: vec![1, 0],
                },
            ),
        };
        let mut output = Vec::new();
        expand_assignment(value, &[0, 1], &mut locals, &[Some(producer)], &mut output);
        let [Statement::Scope { body, clear }] = output.as_slice() else {
            panic!("argument and component owners require one cleanup scope");
        };
        let [
            Statement::Assign(
                second,
                Expr {
                    kind: ExprKind::Str(_),
                    ..
                },
            ),
            Statement::Assign(
                first,
                Expr {
                    kind: ExprKind::Int(_),
                    ..
                },
            ),
            Statement::Assign(
                _,
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Local(bound_first),
                },
            ),
            Statement::Assign(
                _,
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Local(bound_second),
                },
            ),
            ..,
        ] = body.as_slice()
        else {
            panic!("arguments must stage in source order before parameter substitution");
        };
        assert_eq!(first, bound_first);
        assert_eq!(second, bound_second);
        assert!(clear.contains(first) && clear.contains(second));
    }

    #[test]
    fn cleared_unpacking_roots_do_not_materialize_tuples() {
        let source = r#"
def total(values: list[int]) -> int:
    number, name, payload = (1000000000000000000000000000000, "snapshot", [1])
    print(number)
    print(name)
    print(payload[0])
    result = 0
    for index, value in enumerate(values, 4):
        result += index + value
    return result

print(total([10, 20]))
"#;
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        let before = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false);
        assert_eq!(before.matches("call ptr @relar_record_new(").count(), 2);
        scalarize(&mut module);
        let after = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false);
        assert!(!after.contains("call ptr @relar_record_new("));
        let function = &module.functions[0];
        fn collect_cleared(body: &[Statement], locals: &[Type], cleared: &mut Vec<Type>) {
            for statement in body {
                match statement {
                    Statement::Clear(slot) => cleared.push(locals[*slot]),
                    Statement::Scope { body, clear } => {
                        cleared.extend(clear.iter().map(|&slot| locals[slot]));
                        collect_cleared(body, locals, cleared);
                    }
                    Statement::While(_, body) => collect_cleared(body, locals, cleared),
                    _ => {}
                }
            }
        }
        let mut cleared = Vec::new();
        collect_cleared(&function.body, &function.locals, &mut cleared);
        assert!(cleared.contains(&Type::Int));
        assert!(cleared.contains(&Type::Str));
        assert!(cleared.contains(&Type::ListInt));
    }
}
