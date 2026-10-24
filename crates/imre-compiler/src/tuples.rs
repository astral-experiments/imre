//! Scalar replacement for tuple locals whose identity and aggregate never escape.
//!
//! Each component is an ordinary owned local. Arguments and elements are evaluated
//! once in their original order, into owned temporaries, before any old component
//! is replaced. This preserves snapshots of mutable fields and aliases of mutable
//! elements. A read-only key can probe with these components. An insertion
//! materializes the tuple only when its identity can escape at most once.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    Builtin, Expr, ExprKind, Function, Module, Statement, TUPLE_PROBE_ARITY, Type, children_mut,
};

struct Producer {
    params: Vec<usize>,
    elements: Vec<Expr>,
}

struct Candidate {
    eligible: bool,
    fields: Option<Vec<Type>>,
    literal_key: bool,
    probed: bool,
    stored: bool,
}

/// Splits locals built by literals or by a function that only returns a tuple.
/// Producer functions retain their original signatures and boxed runtime ABI.
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
        scalarize_function(function, &producers, &module.types, &module.class_fields);
    }
}

/// Producer expressions may refer only to their own parameters. This guards the
/// substitution independently of how frontend lowering allocates local slots.
fn only_parameters(expression: &Expr, params: &[usize]) -> bool {
    if matches!(expression.kind, ExprKind::Bind(..) | ExprKind::Scope { .. }) {
        return false;
    }
    if let ExprKind::Local(slot) = expression.kind {
        return params.contains(&slot);
    }
    let mut result = true;
    children(expression, |child| result &= only_parameters(child, params));
    result
}

fn scalarize_function(
    function: &mut Function,
    producers: &[Option<Producer>],
    types: &crate::types::Types,
    class_fields: &[Vec<Type>],
) {
    // Named bindings can replace a tuple midway through another expression.
    // Retain aggregate ownership until the projection pass models those writes.
    if function
        .expression_rebound()
        .into_iter()
        .any(|written| written)
    {
        return;
    }
    let mut candidates = function
        .locals
        .iter()
        .enumerate()
        .map(|(slot, ty)| Candidate {
            // Splitting a file owner's tuple changes the order of its final drops.
            eligible: matches!(ty, Type::Tuple(_))
                && !function.params.contains(&slot)
                && !types.may_own_file(*ty, class_fields, true),
            fields: None,
            literal_key: true,
            probed: false,
            stored: false,
        })
        .collect::<Vec<_>>();
    inspect(
        &function.body,
        &mut candidates,
        producers,
        &mut HashSet::new(),
    );
    let mut components = vec![None; candidates.len()];
    for (slot, candidate) in candidates.into_iter().enumerate() {
        if candidate.eligible
            && (!candidate.stored || candidate.probed)
            && let Some(fields) = candidate.fields
            && (!candidate.probed || candidate.literal_key)
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

/// A private expression scope may stage arguments before returning a fixed
/// tuple. Its cleanup changes ownership, not the final tuple's field layout.
fn tuple_elements<'a>(
    expression: &'a Expr,
    producers: &'a [Option<Producer>],
) -> Option<&'a [Expr]> {
    match &expression.kind {
        ExprKind::Tuple(elements) => Some(elements.as_slice()),
        ExprKind::Call(id, _) => producers[*id]
            .as_ref()
            .map(|producer| producer.elements.as_slice()),
        ExprKind::Scope { value, .. } => tuple_elements(value, producers),
        _ => None,
    }
}

/// `fresh` lists unconditional tuple assignments in this block or its parents
/// since the innermost loop header. A single insertion dominated by one of
/// these assignments can never publish two distinct objects for one source key.
fn inspect(
    statements: &[Statement],
    candidates: &mut [Candidate],
    producers: &[Option<Producer>],
    fresh: &mut HashSet<usize>,
) {
    let mut introduced = Vec::new();
    for statement in statements {
        match statement {
            Statement::Assign(slot, value) => {
                if let Some(elements) = tuple_elements(value, producers) {
                    let fields = elements.iter().map(|value| value.ty).collect::<Vec<_>>();
                    let candidate = &mut candidates[*slot];
                    if candidate.fields.as_ref().is_some_and(|old| old != &fields) {
                        candidate.eligible = false;
                    }
                    // Keep calls at probe-producing assignments: inlining their
                    // arguments can change destructor order if a later one fails.
                    candidate.literal_key &= matches!(value.kind, ExprKind::Tuple(_))
                        && TUPLE_PROBE_ARITY.contains(&fields.len());
                    candidate.fields = Some(fields);
                    if candidate.eligible && candidate.literal_key && fresh.insert(*slot) {
                        introduced.push(*slot);
                    }
                } else {
                    candidates[*slot].eligible = false;
                }
                inspect_expression(value, candidates, producers);
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
            Statement::SetItem(dict, key, value)
                if matches!(dict.ty, Type::Dict(_))
                    && let ExprKind::Local(slot) = key.kind
                    && matches!(key.ty, Type::Tuple(_)) =>
            {
                let candidate = &mut candidates[slot];
                candidate.eligible &= !candidate.stored && fresh.contains(&slot);
                candidate.stored = true;
                inspect_expression(dict, candidates, producers);
                inspect_expression(value, candidates, producers);
            }
            Statement::SetItem(a, b, c) | Statement::AugmentItem(a, b, _, c) => {
                for value in [a, b, c] {
                    inspect_expression(value, candidates, producers);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                inspect_expression(a, candidates, producers);
                inspect_expression(b, candidates, producers);
            }
            Statement::Expr(value) | Statement::Return(value) | Statement::Assert(value) => {
                inspect_expression(value, candidates, producers);
            }
            Statement::If(condition, yes, no) => {
                inspect_expression(condition, candidates, producers);
                inspect(yes, candidates, producers, fresh);
                inspect(no, candidates, producers, fresh);
            }
            Statement::While(condition, body, orelse) => {
                inspect_expression(condition, candidates, producers);
                inspect(body, candidates, producers, &mut HashSet::new());
                inspect(orelse, candidates, producers, fresh);
            }
            Statement::Scope { body, .. } => inspect(body, candidates, producers, fresh),
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                for body in [body, orelse, finalbody] {
                    inspect(body, candidates, producers, fresh);
                }
                for handler in handlers {
                    inspect(&handler.body, candidates, producers, fresh);
                }
            }
            Statement::Raise(value) => {
                if let Some(value) = value {
                    inspect_expression(value, candidates, producers);
                }
            }
            Statement::For {
                target,
                args,
                body,
                orelse,
                ..
            } => {
                candidates[*target].eligible = false;
                for arg in args {
                    inspect_expression(arg, candidates, producers);
                }
                inspect(body, candidates, producers, &mut HashSet::new());
                inspect(orelse, candidates, producers, fresh);
            }
            Statement::Break | Statement::Continue | Statement::Exhaust => {}
        }
    }
    for slot in introduced {
        fresh.remove(&slot);
    }
}

fn inspect_expression(
    expression: &Expr,
    candidates: &mut [Candidate],
    producers: &[Option<Producer>],
) {
    match &expression.kind {
        ExprKind::Scope { body, value, .. } => {
            inspect(body, candidates, producers, &mut HashSet::new());
            inspect_expression(value, candidates, producers);
        }
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
        ExprKind::Builtin(builtin, args) if let Some(key) = builtin.tuple_probe_index() => {
            for (index, arg) in args.iter().enumerate() {
                if index == key
                    && let ExprKind::Local(slot) = arg.kind
                {
                    candidates[slot].probed = true;
                } else {
                    inspect_expression(arg, candidates, producers);
                }
            }
        }
        _ => children(expression, |child| {
            inspect_expression(child, candidates, producers)
        }),
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
        ExprKind::Scope {
            mut body,
            value,
            clear,
        } => {
            // Publish only after the RHS scope has cleaned up: its cleanup may
            // release the old destination as well as private argument roots.
            let staged = targets
                .iter()
                .map(|&slot| {
                    let ty = locals[slot];
                    new_local(locals, ty)
                })
                .collect::<Vec<_>>();
            expand_assignment(*value, &staged, locals, producers, &mut body);
            let mut body = vec![Statement::Scope { body, clear }];
            body.extend(
                targets
                    .iter()
                    .zip(&staged)
                    .map(|(&target, &source)| Statement::Move(target, source)),
            );
            output.push(Statement::Scope {
                body,
                clear: staged,
            });
            return;
        }
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
                rewrite_expression(value, locals, components, producers);
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
                    rewrite_expression(value, locals, components, producers);
                }
            }
            Statement::SetField(a, _, b) | Statement::AugmentField(a, _, _, b) => {
                rewrite_expression(a, locals, components, producers);
                rewrite_expression(b, locals, components, producers);
            }
            Statement::Expr(value) | Statement::Return(value) | Statement::Assert(value) => {
                rewrite_expression(value, locals, components, producers);
            }
            Statement::If(condition, yes, no) => {
                rewrite_expression(condition, locals, components, producers);
                rewrite_block(yes, locals, components, producers);
                rewrite_block(no, locals, components, producers);
            }
            Statement::While(condition, body, orelse) => {
                rewrite_expression(condition, locals, components, producers);
                rewrite_block(body, locals, components, producers);
                rewrite_block(orelse, locals, components, producers);
            }
            Statement::Scope { body, clear } => {
                rewrite_block(body, locals, components, producers);
                rewrite_cleanup(clear, components);
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
                    rewrite_expression(value, locals, components, producers);
                }
            }
            Statement::For {
                args, body, orelse, ..
            } => {
                for arg in args {
                    rewrite_expression(arg, locals, components, producers);
                }
                rewrite_block(body, locals, components, producers);
                rewrite_block(orelse, locals, components, producers);
            }
            Statement::Move(..)
            | Statement::IterNext { .. }
            | Statement::Break
            | Statement::Continue
            | Statement::Exhaust => {}
        }
        output.push(statement);
    }
    *statements = output;
}

/// Scoped expressions own the same local roots as statement scopes, including
/// scalarized tuple components that must be cleared on each exit.
fn rewrite_expression(
    expression: &mut Expr,
    locals: &mut Vec<Type>,
    components: &[Option<Vec<usize>>],
    producers: &[Option<Producer>],
) {
    if let ExprKind::Scope { body, value, clear } = &mut expression.kind {
        rewrite_block(body, locals, components, producers);
        rewrite_expression(value, locals, components, producers);
        rewrite_cleanup(clear, components);
        return;
    }
    let replacement = match &expression.kind {
        // The only surviving whole reads are a read-only probe (stack storage)
        // or its single insertion (owned storage). Both use the saved fields.
        ExprKind::Local(slot) => components
            .get(*slot)
            .and_then(Option::as_ref)
            .map(|fields| {
                ExprKind::Tuple(
                    fields
                        .iter()
                        .map(|&slot| Expr {
                            ty: locals[slot],
                            kind: ExprKind::Local(slot),
                        })
                        .collect(),
                )
            }),
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
        children_mut(expression, |child| {
            rewrite_expression(child, locals, components, producers)
        });
    }
}

fn rewrite_cleanup(clear: &mut Vec<usize>, components: &[Option<Vec<usize>>]) {
    *clear = clear
        .iter()
        .flat_map(|&slot| components[slot].clone().unwrap_or_else(|| vec![slot]))
        .collect();
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
        ExprKind::Callable { receiver, .. } => {
            if let Some(receiver) = receiver {
                visit(receiver);
            }
        }
        ExprKind::Invoke { callable, args } => {
            visit(callable);
            for value in &args.values {
                visit(value);
            }
        }
        ExprKind::Scope { .. } => {
            unreachable!("scoped statements require the full inspection or rewrite pass")
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => visit(value),
        ExprKind::Binary(_, left, right) | ExprKind::BooleanValue(_, left, right) => {
            visit(left);
            visit(right);
        }
        ExprKind::Conditional(test, yes, no) => {
            visit(test);
            visit(yes);
            visit(no);
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
                    exhaustive_enum_match: false,
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
        // A list-pattern assignment exercises tuple scalarization, while a plain
        // tuple of local targets already stages each RHS directly in the frontend.
        let source = r#"
def total(values: list[int]) -> int:
    [number, name, payload] = (1000000000000000000000000000000, "snapshot", [1])
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
        let before = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert_eq!(before.matches("call ptr @imre_record_try_new(").count(), 2);
        scalarize(&mut module);
        let after = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert!(!after.contains("call ptr @imre_record_try_new("));
        let function = &module.functions[0];
        fn collect_cleared(body: &[Statement], locals: &[Type], cleared: &mut Vec<Type>) {
            for statement in body {
                match statement {
                    Statement::Clear(slot) => cleared.push(locals[*slot]),
                    Statement::Scope { body, clear } => {
                        cleared.extend(clear.iter().map(|&slot| locals[slot]));
                        collect_cleared(body, locals, cleared);
                    }
                    Statement::While(_, body, orelse) => {
                        collect_cleared(body, locals, cleared);
                        collect_cleared(orelse, locals, cleared);
                    }
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

    #[test]
    fn scoped_tuple_constructor_publishes_after_rhs_cleanup() {
        let graph = crate::modules::SourceGraph::single(
            r#"
def replace() -> str:
    pair = ("old", "owner")
    pair = ("new", "value")
    return pair[0] + pair[1]
"#,
        )
        .unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        let Statement::Assign(slot, value) = &mut module.functions[0].body[1] else {
            panic!("fixture replaces the tuple");
        };
        *value = Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: Vec::new(),
                value: Box::new(value.clone()),
                clear: vec![*slot],
            },
        };
        scalarize(&mut module);
        let after = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert!(!after.contains("call ptr @imre_record_try_new("));
        let Statement::Scope {
            body,
            clear: staged,
        } = &module.functions[0].body[1]
        else {
            panic!("staging owners need cleanup on failed RHS evaluation");
        };
        let [
            Statement::Scope { clear: old, .. },
            Statement::Move(a, x),
            Statement::Move(b, y),
        ] = body.as_slice()
        else {
            panic!("the RHS cleanup must precede both destination writes");
        };
        assert_eq!(old, &vec![*a, *b]);
        assert_eq!(staged, &vec![*x, *y]);
    }

    #[test]
    fn divmod_scalarization_keeps_single_operand_evaluation() {
        let graph = crate::modules::SourceGraph::single(
            r#"
def operand(value: int) -> int:
    print(value)
    return value

def calculate(left: int, right: int) -> int:
    quotient, remainder = divmod(operand(left), operand(right))
    return quotient + remainder
"#,
        )
        .unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        scalarize(&mut module);
        let after = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert!(!after.contains("call ptr @imre_record_try_new("));
        assert_eq!(after.matches("call i64 @fn_0(").count(), 2);
    }

    #[test]
    fn fixed_tuple_sum_uses_scalar_components() {
        for source in [
            r#"
def total(left: int, right: int, start: int) -> int:
    return sum(divmod(left, right), start)
"#,
            r#"
def total(left: int, right: int, start: int) -> int:
    return sum((left, right), start=start)
"#,
            r#"
def total(left: str, right: str, start: int) -> int:
    return sum(map(int, (left, right)), start)
"#,
        ] {
            let graph = crate::modules::SourceGraph::single(source).unwrap();
            let mut module = crate::frontend::lower(&graph).unwrap();
            let before =
                crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
            assert_eq!(before.matches("call ptr @imre_record_try_new(").count(), 1);

            scalarize(&mut module);

            let after =
                crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
            assert!(!after.contains("call ptr @imre_record_try_new("));
        }
    }

    #[test]
    fn fixed_tuple_sum_preserves_source_start_and_conversion_order() {
        for (source, expected) in [
            (
                r#"
def operand(value: int) -> int:
    print(value)
    return value

def initial() -> int:
    print(99)
    return 100

def total(left: int, right: int) -> int:
    return sum(divmod(operand(left), operand(right)), initial())
"#,
                vec!["operand", "operand", "floor", "modulo", "start"],
            ),
            (
                r#"
def operand(value: str) -> str:
    print(value)
    return value

def initial() -> int:
    print(99)
    return 100

def total(left: str, right: str) -> int:
    return sum(map(int, (operand(left), operand(right))), initial())
"#,
                vec!["operand", "operand", "start", "convert", "convert"],
            ),
        ] {
            let graph = crate::modules::SourceGraph::single(source).unwrap();
            let mut module = crate::frontend::lower(&graph).unwrap();
            scalarize(&mut module);
            let mut order = Vec::new();
            crate::ir::walk_expressions(&module.functions[2].body, &mut |value| match value.kind {
                ExprKind::Call(0, _) => order.push("operand"),
                ExprKind::Call(1, _) => order.push("start"),
                ExprKind::Binary(crate::ir::Binary::FloorDivide, _, _) => order.push("floor"),
                ExprKind::Binary(crate::ir::Binary::Modulo, _, _) => order.push("modulo"),
                ExprKind::Builtin(Builtin::Int, _) => order.push("convert"),
                _ => {}
            });
            assert_eq!(order, expected);
        }
    }

    #[test]
    fn fixed_tuple_sum_keeps_escaping_aggregate() {
        let graph = crate::modules::SourceGraph::single(
            r#"
def identity(pair: tuple[int, int]) -> tuple[int, int]:
    return pair

def total(left: int, right: int) -> int:
    pair = divmod(left, right)
    other = identity(pair)
    return sum(pair) + other[0]
"#,
        )
        .unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        scalarize(&mut module);
        let after = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert_eq!(after.matches("call ptr @imre_record_try_new(").count(), 1);
        assert_eq!(after.matches("call ptr @fn_0(").count(), 1);
    }

    #[test]
    fn expression_scope_scalarizes_tuple_and_keeps_failure_cleanup() {
        let graph = crate::modules::SourceGraph::single(
            r#"
def scoped(fail: bool) -> str:
    pair = ("owned text", [1])
    if fail:
        raise ValueError("before projection")
    return pair[0]
"#,
        )
        .unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        let function = &mut module.functions[0];
        let tuple = function
            .locals
            .iter()
            .position(|ty| matches!(ty, Type::Tuple(_)))
            .unwrap();
        let Some(Statement::Return(value)) = function.body.pop() else {
            panic!("fixture returns a tuple projection");
        };
        // Put the producer and a possible exception inside an expression scope.
        // Its result keeps one component alive after the aggregate is released.
        let body = std::mem::take(&mut function.body);
        function.body.push(Statement::Return(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear: vec![tuple],
            },
        }));
        let before = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert_eq!(before.matches("call ptr @imre_record_try_new(").count(), 1);

        scalarize(&mut module);

        let after = crate::llvm::emit(&module, &crate::ranges::analyze(&module), false).unwrap();
        assert!(!after.contains("call ptr @imre_record_try_new("));
        let function = &module.functions[0];
        let [
            Statement::Return(Expr {
                kind: ExprKind::Scope { body, value, clear },
                ..
            }),
        ] = function.body.as_slice()
        else {
            panic!("scalarization preserves the enclosing cleanup scope");
        };
        assert_eq!(
            clear
                .iter()
                .map(|&slot| function.locals[slot])
                .collect::<Vec<_>>(),
            vec![Type::Str, Type::ListInt]
        );
        let ExprKind::Local(result) = value.kind else {
            panic!("the projection should read its owned scalar component");
        };
        assert!(clear.contains(&result));
        assert!(body.iter().any(|statement| matches!(
            statement,
            Statement::If(_, yes, _) if yes.iter().any(|statement| matches!(statement, Statement::Raise(_)))
        )));
    }
}
