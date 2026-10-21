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
            Statement::Clear(slot) => candidates[*slot].eligible = false,
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
    let mut arguments = Vec::new();
    let elements = match value.kind {
        ExprKind::Tuple(elements) => elements,
        ExprKind::Call(id, args) => {
            let producer = producers[id].as_ref().expect("checked tuple producer");
            debug_assert_eq!(producer.params.len(), args.len());
            let mut bindings = HashMap::new();
            for (&parameter, arg) in producer.params.iter().zip(args) {
                let slot = new_local(locals, arg.ty);
                bindings.insert(parameter, slot);
                arguments.push(slot);
                output.push(Statement::Assign(slot, arg));
            }
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
        output.push(Statement::Assign(slot, element));
    }
    output.extend(arguments.into_iter().map(Statement::Clear));
    for (&target, source) in targets.iter().zip(staged) {
        output.push(Statement::Move(target, source));
    }
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
            Statement::For { args, body, .. } => {
                for arg in args {
                    rewrite_expression(arg, components);
                }
                rewrite_block(body, locals, components, producers);
            }
            Statement::Move(..) | Statement::Clear(_) | Statement::Break | Statement::Continue => {}
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
        ExprKind::List(values)
        | ExprKind::Tuple(values)
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Call(_, values)
        | ExprKind::Builtin(_, values)
        | ExprKind::Construct { args: values, .. } => {
            for value in values {
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
        ExprKind::List(values)
        | ExprKind::Tuple(values)
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values)
        | ExprKind::Call(_, values)
        | ExprKind::Builtin(_, values)
        | ExprKind::Construct { args: values, .. } => {
            for value in values {
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
