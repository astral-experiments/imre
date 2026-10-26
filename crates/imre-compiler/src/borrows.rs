//! Borrows projections only while unchanged caller roots preserve every heap edge.

pub(crate) mod getters;
mod private;

use crate::callables::Targets;
use crate::ir::{self, Builtin, Expr, ExprKind, Function, Module, Statement, Type};
use crate::types::Types;

#[derive(Default)]
pub(crate) struct FunctionBorrows {
    /// Includes stable incoming parameters and their proven local projections.
    pub rooted: Vec<bool>,
    /// These local slots carry borrowed words, never independent owners.
    pub locals: Vec<bool>,
    /// Identity-only aliases of unchanged list parameters in readonly functions.
    /// Clear/Move can reset an alias, so reads must still compare current identity.
    pub length_roots: Vec<Option<usize>>,
}

/// Computes closed-callgraph edge preservation before deriving local roots.
/// The separate initializer summary applies only to a newly allocated receiver.
pub(crate) fn analyze(module: &Module, targets: &Targets) -> Vec<FunctionBorrows> {
    let private = private::analyze(module);
    let (preserves, initializes) = summaries(module, true, targets, &private);
    let (readonly, readonly_initializes) = summaries(module, false, targets, &private);
    module
        .functions
        .iter()
        .chain([&module.entry])
        .zip(&private)
        .map(|(function, private)| {
            let mut facts = if preserves_edges(
                function,
                &preserves,
                &initializes,
                false,
                true,
                targets,
                private,
            ) {
                local_roots(function, &module.types, &module.functions)
            } else {
                FunctionBorrows {
                    rooted: vec![false; function.locals.len()],
                    locals: vec![false; function.locals.len()],
                    length_roots: vec![None; function.locals.len()],
                }
            };
            if preserves_edges(
                function,
                &readonly,
                &readonly_initializes,
                false,
                false,
                targets,
                private,
            ) {
                facts.length_roots = length_roots(function, &facts, &module.types);
            }
            facts
        })
        .collect()
}

/// Appending preserves element owners. Incoming lengths and buffers also stay
/// intact when an append or reserve targets a freshly allocated private list.
fn summaries(
    module: &Module,
    append: bool,
    targets: &Targets,
    private: &[Vec<bool>],
) -> (Vec<bool>, Vec<bool>) {
    // Unknown callees and unexpected recursive cycles retain ordinary owners.
    // The checked call graph is acyclic, so proving upward loses no precision.
    let mut preserves = vec![false; module.functions.len()];
    let mut initializes = preserves.clone();
    loop {
        let mut changed = false;
        for (id, function) in module.functions.iter().enumerate() {
            let safe = preserves_edges(
                function,
                &preserves,
                &initializes,
                false,
                append,
                targets,
                &private[id],
            );
            let fresh = preserves_edges(
                function,
                &preserves,
                &initializes,
                true,
                append,
                targets,
                &private[id],
            );
            changed |= preserves[id] != safe || initializes[id] != fresh;
            preserves[id] = safe;
            initializes[id] = fresh;
        }
        if !changed {
            break;
        }
    }
    (preserves, initializes)
}

/// Visits statement nodes once, including statements nested inside expressions.
pub(crate) fn statements(body: &[Statement], visit: &mut impl FnMut(&Statement)) {
    fn block(body: &[Statement], visit: &mut impl FnMut(&Statement)) {
        for statement in body {
            visit(statement);
            match statement {
                Statement::Scope { body, .. } => block(body, visit),
                Statement::If(_, yes, no) => {
                    block(yes, visit);
                    block(no, visit);
                }
                Statement::While(_, body, orelse) | Statement::For { body, orelse, .. } => {
                    block(body, visit);
                    block(orelse, visit);
                }
                Statement::Try {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                } => {
                    block(body, visit);
                    for handler in handlers {
                        block(&handler.body, visit);
                    }
                    block(orelse, visit);
                    block(finalbody, visit);
                }
                _ => {}
            }
        }
    }
    block(body, visit);
    ir::walk_expressions(body, &mut |expression| {
        if let ExprKind::Scope { body, .. } = &expression.kind {
            block(body, visit);
        }
    });
}

/// Records every local writer, including cleanup and hidden iterator outputs.
pub(crate) fn written(function: &Function) -> Vec<bool> {
    written_except(function, |_| false)
}

fn written_except(function: &Function, permitted: impl Fn(&Expr) -> bool) -> Vec<bool> {
    let mut written = function.expression_rebound();
    statements(&function.body, &mut |statement| match statement {
        Statement::Assign(slot, value) => written[*slot] |= !permitted(value),
        Statement::Clear(slot) => written[*slot] = true,
        Statement::Move(target, source) => {
            written[*target] = true;
            written[*source] = true;
        }
        Statement::Scope { clear, .. } => {
            for &slot in clear {
                written[slot] = true;
            }
        }
        Statement::For {
            target,
            cursor,
            stop,
            step,
            ..
        } => {
            for &slot in [target, cursor, stop, step] {
                written[slot] = true;
            }
        }
        Statement::IterNext {
            key, value, ready, ..
        } => {
            written[*key] = true;
            written[*ready] = true;
            if let Some(slot) = value {
                written[*slot] = true;
            }
        }
        Statement::Try { handlers, .. } => {
            for handler in handlers {
                if let Some(slot) = handler.name {
                    written[slot] = true;
                }
            }
        }
        _ => {}
    });
    ir::walk_expressions(&function.body, &mut |expression| {
        if let ExprKind::Scope { clear, .. } = &expression.kind {
            for &slot in clear {
                written[slot] = true;
            }
        }
    });
    written
}

/// Appends can move sequence storage, but preserve every existing element owner.
/// Borrowed values are object words, never pointers into movable storage.
fn preserves_edges(
    function: &Function,
    calls: &[bool],
    constructors: &[bool],
    fresh: bool,
    append: bool,
    targets: &Targets,
    private: &[bool],
) -> bool {
    let written = written(function);
    let receiver = function
        .params
        .first()
        .copied()
        .filter(|&slot| fresh && !written[slot]);
    let mut safe = true;
    statements(&function.body, &mut |statement| {
        safe &= match statement {
            Statement::SetItem(..) | Statement::AugmentItem(..) => false,
            Statement::AugmentField(value, _, _, _) => {
                matches!(value.kind, ExprKind::Local(slot) if private[slot])
            }
            Statement::SetField(value, _, _) => {
                matches!(value.kind, ExprKind::Local(slot) if Some(slot) == receiver || private[slot])
            }
            _ => true,
        };
    });
    ir::walk_expressions(&function.body, &mut |expression| {
        safe &= match &expression.kind {
            ExprKind::Call(id, _) => calls.get(*id).copied().unwrap_or(false),
            ExprKind::Invoke { callable, .. } => {
                let candidates = targets.get(callable.ty);
                // Even a bound initializer is an ordinary call on an existing
                // receiver; only Construct may use the fresh-object summary.
                !candidates.is_empty()
                    && candidates
                        .iter()
                        .all(|target| calls.get(target.function).copied().unwrap_or(false))
            }
            ExprKind::Construct { initializer, .. } => {
                constructors.get(*initializer).copied().unwrap_or(false)
            }
            ExprKind::Builtin(operation, args) => {
                let contract = operation.contract(args);
                contract.preserves_list_storage()
                    || (matches!(
                        operation,
                        Builtin::SetAdd | Builtin::Append | Builtin::ListReserveRange
                    ) && matches!(args[0].kind, ExprKind::Local(slot) if private[slot]))
                    || (append && contract.preserves_owners())
            }
            // A binding can replace an earlier borrowed operand during evaluation.
            ExprKind::Bind(..) => false,
            _ => true,
        };
    });
    safe
}

/// Returns the local anchoring a nonallocating projection, independently of
/// whether that local has a proven caller root. Optional float boxes are owned.
pub(crate) fn projection_root(
    expression: &Expr,
    types: &Types,
    functions: &[Function],
) -> Option<usize> {
    if types.payload(expression.ty) == Some(Type::Float) || expression.ty == Type::ActivePopen {
        return None;
    }
    match &expression.kind {
        ExprKind::Local(slot) => Some(*slot),
        ExprKind::Call(..) => Some(getters::selected(expression, functions)?.root),
        ExprKind::Field(receiver, _) => projection_root(receiver, types, functions),
        ExprKind::Builtin(Builtin::SequenceGetInBounds, args)
            if !types.is_value_struct(expression.ty) =>
        {
            projection_root(&args[0], types, functions)
        }
        ExprKind::Present(value) if value.ty != Type::Float => {
            projection_root(value, types, functions)
        }
        ExprKind::Unwrap(value) if types.payload(value.ty) != Some(Type::Float) => {
            projection_root(value, types, functions)
        }
        ExprKind::Scope { value, .. } => projection_root(value, types, functions),
        _ => None,
    }
}

/// A local is borrowed only if all its definitions preserve an incoming root.
/// Elimination rejects mixed owned definitions; reachability rejects rootless cycles.
fn local_roots(function: &Function, types: &Types, functions: &[Function]) -> FunctionBorrows {
    let written = written(function);
    let mut parameters = vec![false; function.locals.len()];
    let mut roots = parameters.clone();
    for &slot in &function.params {
        parameters[slot] = true;
        roots[slot] = !written[slot];
    }
    let mut dependencies = vec![Vec::new(); function.locals.len()];
    let mut candidates = function
        .locals
        .iter()
        .map(|&ty| {
            !matches!(ty, Type::Float | Type::Bool | Type::None | Type::Iterator)
                && types.payload(ty) != Some(Type::Float)
        })
        .collect::<Vec<_>>();
    statements(&function.body, &mut |statement| match statement {
        Statement::Assign(slot, value) => {
            if let Some(source) = projection_root(value, types, functions) {
                dependencies[*slot].push(source);
            } else {
                candidates[*slot] = false;
            }
        }
        Statement::Move(target, source) => dependencies[*target].push(*source),
        Statement::For {
            target,
            cursor,
            stop,
            step,
            ..
        } => {
            for &slot in [target, cursor, stop, step] {
                candidates[slot] = false;
            }
        }
        Statement::IterNext {
            key, value, ready, ..
        } => {
            candidates[*key] = false;
            candidates[*ready] = false;
            if let Some(slot) = value {
                candidates[*slot] = false;
            }
        }
        Statement::Try { handlers, .. } => {
            for handler in handlers {
                if let Some(slot) = handler.name {
                    candidates[slot] = false;
                }
            }
        }
        _ => {}
    });
    for slot in 0..candidates.len() {
        candidates[slot] &= !parameters[slot] && !dependencies[slot].is_empty();
    }
    loop {
        let mut changed = false;
        for slot in 0..candidates.len() {
            if candidates[slot]
                && dependencies[slot]
                    .iter()
                    .any(|&source| !roots[source] && !candidates[source])
            {
                candidates[slot] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    loop {
        let mut changed = false;
        for slot in 0..candidates.len() {
            if candidates[slot]
                && !roots[slot]
                && dependencies[slot].iter().any(|&source| roots[source])
            {
                roots[slot] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // A candidate may have one rooted definition and a second definition from
    // a disconnected cycle. Every possible source must itself retain a root.
    loop {
        let mut changed = false;
        for slot in 0..candidates.len() {
            if candidates[slot]
                && roots[slot]
                && dependencies[slot].iter().any(|&source| !roots[source])
            {
                roots[slot] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for slot in 0..candidates.len() {
        candidates[slot] &= roots[slot];
    }
    FunctionBorrows {
        rooted: roots,
        locals: candidates,
        length_roots: vec![None; function.locals.len()],
    }
}

/// Keeps only exact list identities. A field projection or a new allocation
/// cannot reuse its enclosing parameter's length. Resets remain guarded at use.
fn length_roots(function: &Function, facts: &FunctionBorrows, types: &Types) -> Vec<Option<usize>> {
    let mut roots = vec![None; function.locals.len()];
    for &slot in &function.params {
        // A Popen finalizer can append to the shared _active list even in a
        // function that contains no Python list mutation.
        if facts.rooted[slot]
            && matches!(function.locals[slot], Type::ListInt | Type::List(_))
            && types.element(function.locals[slot]) != Some(Type::ActivePopen)
        {
            roots[slot] = Some(slot);
        }
    }
    let mut dependencies = vec![Vec::new(); function.locals.len()];
    let mut eligible = facts.locals.clone();
    for (slot, eligible) in eligible.iter_mut().enumerate() {
        *eligible &= matches!(function.locals[slot], Type::ListInt | Type::List(_));
    }
    statements(&function.body, &mut |statement| match statement {
        Statement::Assign(slot, value) => {
            if let ExprKind::Local(source) = value.kind {
                dependencies[*slot].push(source);
            } else {
                eligible[*slot] = false;
            }
        }
        Statement::Move(target, source) => dependencies[*target].push(*source),
        _ => {}
    });
    loop {
        let mut changed = false;
        for slot in 0..roots.len() {
            if !eligible[slot] || roots[slot].is_some() {
                continue;
            }
            let Some(root) = dependencies[slot].first().and_then(|&source| roots[source]) else {
                continue;
            };
            if dependencies[slot]
                .iter()
                .all(|&source| roots[source] == Some(root))
            {
                roots[slot] = Some(root);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::CallArgs;

    fn analyze(module: &Module) -> Vec<FunctionBorrows> {
        super::analyze(module, &Targets::analyze(module))
    }

    fn local(slot: usize, ty: Type) -> Expr {
        Expr {
            ty,
            kind: ExprKind::Local(slot),
        }
    }

    fn function(params: Vec<usize>, locals: Vec<Type>, body: Vec<Statement>) -> Function {
        Function {
            params,
            returns: Type::None,
            locals,
            body,
        }
    }

    fn module(functions: Vec<Function>) -> Module {
        Module {
            ignore_python_environment: false,
            types: Types::default(),
            class_fields: vec![vec![Type::Class(0)]],
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            command_utf8_mode: None,
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            functions,
            entry: function(Vec::new(), Vec::new(), Vec::new()),
        }
    }

    #[test]
    fn every_definition_must_reach_a_stable_parameter() {
        let ty = Type::Class(0);
        let function = function(
            vec![0],
            vec![ty; 7],
            vec![
                Statement::Assign(1, local(0, ty)),
                Statement::Move(2, 1),
                Statement::Assign(3, local(4, ty)),
                Statement::Assign(4, local(3, ty)),
                // One rooted definition cannot legitimize the disconnected cycle.
                Statement::Assign(5, local(0, ty)),
                Statement::Assign(5, local(3, ty)),
                Statement::Assign(6, local(5, ty)),
                Statement::Clear(2),
            ],
        );
        let facts = analyze(&module(vec![function]));
        assert_eq!(
            facts[0].rooted,
            [true, true, true, false, false, false, false]
        );
        assert_eq!(
            facts[0].locals,
            [false, true, true, false, false, false, false]
        );
    }

    #[test]
    fn unknown_callbacks_disable_projection_and_buffer_borrows() {
        let ty = Type::Class(0);
        let callback = Type::Callable(0);
        let function = function(
            vec![0, 1],
            vec![ty, callback, ty],
            vec![
                Statement::Assign(2, local(0, ty)),
                Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Invoke {
                        callable: Box::new(local(1, callback)),
                        args: CallArgs::positional(vec![local(2, ty)]),
                    },
                }),
            ],
        );
        let facts = analyze(&module(vec![function]));
        assert!(facts[0].locals.iter().all(|&borrowed| !borrowed));
        assert!(facts[0].length_roots.iter().all(Option::is_none));
    }

    #[test]
    fn callback_target_joins_distinguish_edge_preservation_from_readonly() {
        let list = Type::ListInt;
        let callback = Type::Callable(0);
        let mut module = module(vec![
            function(vec![0], vec![list], Vec::new()),
            function(vec![0], vec![list], Vec::new()),
            function(
                vec![0, 1],
                vec![list, callback, list],
                vec![
                    Statement::Assign(2, local(0, list)),
                    Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Invoke {
                            callable: Box::new(local(1, callback)),
                            args: CallArgs::positional(vec![local(2, list)]),
                        },
                    }),
                ],
            ),
        ]);
        module.entry.body = [0, 1]
            .into_iter()
            .map(|function| {
                Statement::Expr(Expr {
                    ty: callback,
                    kind: ExprKind::Callable {
                        function,
                        receiver: None,
                    },
                })
            })
            .collect();
        let facts = analyze(&module);
        assert!(facts[2].locals[2]);
        assert_eq!(facts[2].length_roots[2], Some(0));

        let integer = Expr {
            ty: Type::Int,
            kind: ExprKind::Int("1".into()),
        };
        module.functions[1].body = vec![Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::Append, vec![local(0, list), integer.clone()]),
        })];
        let facts = analyze(&module);
        assert!(facts[2].locals[2]);
        assert!(facts[2].length_roots.iter().all(Option::is_none));

        module.functions[1].body =
            vec![Statement::SetItem(local(0, list), integer.clone(), integer)];
        let facts = analyze(&module);
        assert!(facts[2].rooted.iter().all(|&rooted| !rooted));
        assert!(facts[2].length_roots.iter().all(Option::is_none));
    }

    #[test]
    fn expression_scopes_include_definitions_and_parameter_cleanup() {
        let ty = Type::Class(0);
        let scoped = |clear| Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(1, local(0, ty))],
                value: Box::new(local(1, ty)),
                clear,
            },
        };
        let facts = analyze(&module(vec![
            function(
                vec![0],
                vec![ty; 3],
                vec![Statement::Assign(2, scoped(vec![1]))],
            ),
            function(
                vec![0],
                vec![ty; 3],
                vec![Statement::Assign(2, scoped(vec![0, 1]))],
            ),
        ]));
        assert_eq!(facts[0].locals, [false, true, true]);
        assert!(!facts[1].rooted.iter().any(|&rooted| rooted));
    }

    #[test]
    fn fresh_initialization_does_not_authorize_calls_on_existing_receivers() {
        let ty = Type::Class(0);
        let initializer = function(
            vec![0, 1],
            vec![ty; 2],
            vec![Statement::SetField(local(0, ty), 0, local(1, ty))],
        );
        let fresh = function(
            vec![0],
            vec![ty; 2],
            vec![
                Statement::Assign(1, local(0, ty)),
                Statement::Expr(Expr {
                    ty,
                    kind: ExprKind::Construct {
                        initializer: 0,
                        args: CallArgs::positional(vec![local(0, ty)]),
                    },
                }),
            ],
        );
        let ordinary = function(
            vec![0],
            vec![ty; 2],
            vec![
                Statement::Assign(1, local(0, ty)),
                Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Call(0, CallArgs::positional(vec![local(0, ty), local(0, ty)])),
                }),
            ],
        );
        let callback = Type::Callable(0);
        let indirect = function(
            vec![0, 1],
            vec![ty, callback, ty],
            vec![
                Statement::Expr(Expr {
                    ty: callback,
                    kind: ExprKind::Callable {
                        function: 0,
                        receiver: Some(Box::new(local(0, ty))),
                    },
                }),
                Statement::Assign(2, local(0, ty)),
                Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Invoke {
                        callable: Box::new(local(1, callback)),
                        args: CallArgs::positional(vec![local(0, ty)]),
                    },
                }),
            ],
        );
        let module = module(vec![initializer, fresh, ordinary, indirect]);
        let facts = analyze(&module);
        assert!(facts[1].locals[1]);
        assert!(!facts[0].rooted.iter().any(|&rooted| rooted));
        assert!(!facts[2].rooted.iter().any(|&rooted| rooted));
        assert!(!facts[3].rooted.iter().any(|&rooted| rooted));
    }

    #[test]
    fn mutation_in_transitive_expression_scope_and_constructor_arguments_is_seen() {
        let ty = Type::Class(0);
        let mutating = function(
            vec![0],
            vec![ty],
            vec![Statement::SetField(local(0, ty), 0, local(0, ty))],
        );
        let nested = Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Call(0, CallArgs::positional(vec![local(0, ty)])),
                })],
                value: Box::new(local(0, ty)),
                clear: Vec::new(),
            },
        };
        let caller = function(
            vec![0],
            vec![ty; 2],
            vec![
                Statement::Assign(1, local(0, ty)),
                Statement::Expr(Expr {
                    ty,
                    kind: ExprKind::Construct {
                        initializer: 0,
                        args: CallArgs::positional(vec![nested]),
                    },
                }),
            ],
        );
        let facts = analyze(&module(vec![mutating, caller]));
        assert!(!facts[1].rooted.iter().any(|&rooted| rooted));
    }

    #[test]
    fn optional_float_boxes_and_fresh_local_sources_keep_owners() {
        let types = Types::default();
        let optional = types.optional(Type::Float);
        let boxed = Expr {
            ty: optional,
            kind: ExprKind::Present(Box::new(local(0, Type::Float))),
        };
        assert_eq!(projection_root(&boxed, &types, &[]), None);
        assert_eq!(projection_root(&local(0, optional), &types, &[]), None);
        let function = function(
            vec![0],
            vec![Type::Str; 3],
            vec![
                Statement::Assign(
                    1,
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str("fresh".into()),
                    },
                ),
                Statement::Assign(2, local(1, Type::Str)),
            ],
        );
        let facts = analyze(&module(vec![function]));
        assert_eq!(facts[0].locals, [false, false, false]);
    }

    /// Exercises the same finalized IR that LLVM sees after pipeline scalarization.
    fn lower(source: &str) -> Module {
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        crate::tuples::scalarize(&mut module);
        crate::list_bounds::specialize(&mut module);
        crate::list_specialization::specialize(&mut module);
        crate::range_specialization::specialize(&mut module);
        crate::reductions::specialize(&mut module);
        module
    }

    fn emitted_body(module: &Module, id: usize) -> String {
        let ranges = crate::ranges::analyze(module);
        let llvm = crate::llvm::emit(module, &ranges, false).unwrap();
        llvm.split_once(&format!("@fn_{id}("))
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0
            .to_owned()
    }

    #[test]
    fn only_appends_to_a_fresh_private_list_preserve_the_input_length() {
        let module = lower(include_str!(
            "../../../tests/fixtures/containers/fresh_list_length.py"
        ));
        let facts = analyze(&module);
        assert_eq!(facts[0].length_roots[0], Some(0));
        assert_eq!(facts[1].length_roots[0], None);
    }

    #[test]
    fn only_private_accumulators_preserve_incoming_list_elements() {
        let module = lower(
            r#"
class Row:
    value: int
    def __init__(self, value: int) -> None:
        self.value = value

def private(rows: list[Row]) -> Row:
    acc = Row(0)
    for row in rows:
        acc.value += row.value
    return acc

def stored(rows: list[Row]) -> Row:
    acc = Row(0)
    rows.append(acc)
    acc.value += 1
    return acc

"#,
        );
        let facts = analyze(&module);
        let scans = module
            .functions
            .iter()
            .zip(facts)
            .filter(|(f, _)| f.params.len() == 1 && matches!(f.locals[f.params[0]], Type::List(_)))
            .map(|(f, facts)| (facts.rooted[f.params[0]], facts.length_roots[f.params[0]]))
            .collect::<Vec<_>>();
        assert_eq!(scans, [(true, Some(0)), (false, None)]);
    }

    #[test]
    fn a_constructor_that_publishes_its_receiver_cannot_create_private_state() {
        let ty = Type::Class(0);
        let initializer = function(
            vec![0, 1],
            vec![ty; 2],
            vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::Append, vec![local(1, ty), local(0, ty)]),
            })],
        );
        let caller = function(
            vec![0],
            vec![ty; 2],
            vec![
                Statement::Assign(
                    1,
                    Expr {
                        ty,
                        kind: ExprKind::Construct {
                            initializer: 0,
                            args: CallArgs::positional(vec![local(0, ty)]),
                        },
                    },
                ),
                Statement::Return(local(1, ty)),
            ],
        );
        assert!(!private::analyze(&module(vec![initializer, caller]))[1][1]);
    }

    #[test]
    fn scalarized_zip_scan_borrows_records_through_expression_scopes() {
        let module = lower(
            r#"
class Row:
    value: int
    def __init__(self, value: int) -> None:
        self.value = value

def scan(left: list[Row], right: list[Row]) -> int:
    return sum(a.value - b.value for a, b in zip(left, right, strict=True))
"#,
        );
        let id = module
            .functions
            .iter()
            .position(|function| function.returns == Type::Int)
            .unwrap();
        let facts = analyze(&module);
        assert!(
            module.functions[id]
                .locals
                .iter()
                .enumerate()
                .any(|(slot, ty)| { matches!(ty, Type::Class(_)) && facts[id].locals[slot] })
        );
        let body = emitted_body(&module, id);
        assert!(body.contains("call ptr @imre_seq_data_readonly"), "{body}");
        assert!(body.contains("call i64 @imre_seq_borrow_index_proven"));
        assert!(!body.contains("call void @imre_record_retain"), "{body}");
        assert!(!body.contains("call void @imre_record_release"), "{body}");
    }

    #[test]
    fn returning_a_borrowed_record_acquires_an_independent_owner() {
        let module = lower(
            r#"
class Row:
    value: int
    def __init__(self, value: int) -> None:
        self.value = value

def first(rows: list[Row]) -> Row:
    for row in (item for item in rows):
        return row
    raise ValueError("empty")
"#,
        );
        let id = module
            .functions
            .iter()
            .position(|function| matches!(function.returns, Type::Class(_)))
            .unwrap();
        let body = emitted_body(&module, id);
        assert_eq!(
            body.matches("call void @imre_record_retain").count(),
            1,
            "{body}"
        );
        assert!(!body.contains("call void @imre_record_release"), "{body}");
    }

    #[test]
    fn exception_and_finally_paths_keep_caller_roots_without_local_owners() {
        let module = lower(
            r#"
class Row:
    value: int
    def __init__(self, value: int) -> None:
        self.value = value

def total(rows: list[Row], fail: bool) -> int:
    result = 0
    try:
        for row in (item for item in rows):
            if fail:
                raise ValueError("failed")
            result += row.value
    finally:
        for row in (item for item in rows):
            result += row.value
    return result
"#,
        );
        let id = module
            .functions
            .iter()
            .position(|function| function.returns == Type::Int)
            .unwrap();
        let facts = analyze(&module);
        assert!(
            module.functions[id]
                .locals
                .iter()
                .enumerate()
                .any(|(slot, ty)| { matches!(ty, Type::Class(_)) && facts[id].locals[slot] })
        );
        let body = emitted_body(&module, id);
        assert!(body.contains("call void @imre_exception_raise"));
        assert!(!body.contains("call void @imre_record_retain"), "{body}");
        assert!(!body.contains("call void @imre_record_release"), "{body}");
    }

    #[test]
    fn borrowed_to_owned_move_acquires_before_releasing_the_old_slot() {
        let mut function = function(
            vec![0],
            vec![Type::Str; 3],
            vec![
                Statement::Assign(1, local(0, Type::Str)),
                Statement::Assign(
                    2,
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str("owned".into()),
                    },
                ),
                Statement::Move(2, 1),
                Statement::Return(local(2, Type::Str)),
            ],
        );
        function.returns = Type::Str;
        let module = module(vec![function]);
        let facts = analyze(&module);
        assert_eq!(facts[0].locals, [false, true, false]);
        let body = emitted_body(&module, 0);
        let source = body
            .lines()
            .find(|line| line.contains("load ptr, ptr %local1"))
            .unwrap()
            .trim()
            .split_once(" = ")
            .unwrap()
            .0;
        let acquisition = body
            .find(&format!("call void @imre_str_retain(ptr {source})"))
            .unwrap();
        let replacement = body[acquisition..]
            .find("call void @str_release_if_present")
            .unwrap();
        assert!(
            body[acquisition..acquisition + replacement]
                .contains(&format!("store ptr {source}, ptr %local2"))
        );
    }

    #[test]
    fn unknown_or_recursive_calls_keep_the_owned_fallback() {
        for callee in [0, 99] {
            let ty = Type::Class(0);
            let function = function(
                vec![0],
                vec![ty; 2],
                vec![
                    Statement::Assign(1, local(0, ty)),
                    Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Call(callee, CallArgs::positional(vec![local(0, ty)])),
                    }),
                ],
            );
            let facts = analyze(&module(vec![function]));
            assert!(!facts[0].rooted.iter().any(|&rooted| rooted));
        }
    }

    fn length(slot: usize, ty: Type) -> Expr {
        Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![local(slot, ty)]),
        }
    }

    fn sequence_read(slot: usize, cursor: usize, ty: Type) -> Expr {
        Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::SequenceGetInBounds,
                vec![local(slot, ty), local(cursor, Type::Int)],
            ),
        }
    }

    #[test]
    fn readonly_sequence_views_share_nested_loops_but_not_sibling_branches() {
        let mut module = module(Vec::new());
        let ty = module.types.list(Type::Str);
        let read = Statement::Expr(sequence_read(0, 2, ty));
        let inner = Statement::While(
            local(1, Type::Bool),
            vec![read.clone(), Statement::Break],
            Vec::new(),
        );
        let outer = Statement::While(
            local(1, Type::Bool),
            vec![read.clone(), inner, Statement::Break],
            Vec::new(),
        );
        module.functions.push(function(
            vec![0, 1],
            vec![ty, Type::Bool, Type::Int],
            vec![
                Statement::Assign(
                    2,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Int("0".into()),
                    },
                ),
                Statement::If(local(1, Type::Bool), vec![outer.clone()], vec![outer]),
                read,
            ],
        ));
        let body = emitted_body(&module, 0);
        // Each sibling preheader owns a view; its nested loop reuses that view.
        assert_eq!(
            body.matches("call ptr @imre_seq_data_readonly").count(),
            2,
            "{body}"
        );
        // The read after the branches cannot reuse either branch's SSA pointer.
        assert_eq!(
            body.matches("call i64 @imre_seq_borrow_index_proven")
                .count(),
            1,
            "{body}"
        );
        assert!(!body.contains("icmp eq ptr"), "{body}");
    }

    #[test]
    fn list_length_snapshots_are_shared_only_by_dominated_nested_loops() {
        let read = Statement::Expr(length(0, Type::ListInt));
        let inner = Statement::While(
            local(1, Type::Bool),
            vec![read.clone(), Statement::Break],
            Vec::new(),
        );
        let outer = Statement::While(
            local(1, Type::Bool),
            vec![read.clone(), inner, Statement::Break],
            Vec::new(),
        );
        let function = function(
            vec![0, 1],
            vec![Type::ListInt, Type::Bool],
            vec![
                Statement::If(local(1, Type::Bool), vec![outer.clone()], vec![outer]),
                // This read is outside the conditional snapshot's dominance.
                read,
            ],
        );
        let module = module(vec![function]);
        let body = emitted_body(&module, 0);
        assert_eq!(body.matches("call i64 @imre_list_len").count(), 3, "{body}");
        assert!(!body.contains("icmp eq ptr"));
    }

    #[test]
    fn cleared_and_moved_alias_lengths_keep_identity_guards_and_original_fallbacks() {
        for reset in [Statement::Clear(1), Statement::Move(2, 1)] {
            let mut module = module(Vec::new());
            let ty = module.types.list(Type::Str);
            module.functions.push(function(
                vec![0],
                vec![ty, ty, ty, Type::Int],
                vec![
                    Statement::Assign(1, local(0, ty)),
                    Statement::Assign(
                        3,
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("0".into()),
                        },
                    ),
                    Statement::While(
                        Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Bool(true),
                        },
                        vec![
                            reset,
                            Statement::Expr(length(1, ty)),
                            Statement::Expr(sequence_read(1, 3, ty)),
                            Statement::Break,
                        ],
                        Vec::new(),
                    ),
                ],
            ));
            let facts = analyze(&module);
            assert_eq!(facts[0].length_roots[1], Some(0));
            let body = emitted_body(&module, 0);
            // One preheader snapshot and one original read of the current alias.
            assert_eq!(body.matches("call i64 @imre_seq_len").count(), 2, "{body}");
            let guard = body.find("icmp eq ptr").unwrap();
            let fallback = body[guard..].find("call i64 @imre_seq_len").unwrap();
            let merged = body[guard..].find("phi i64").unwrap();
            assert!(fallback < merged, "{body}");
            assert!(body[guard..guard + fallback].contains("br i1"), "{body}");
            assert_eq!(
                body.matches("call ptr @imre_seq_data_readonly").count(),
                1,
                "{body}"
            );
            let (_, read_guard) = body.rsplit_once("icmp eq ptr").unwrap();
            let cached = read_guard.find("getelementptr inbounds i64").unwrap();
            let fallback = read_guard
                .find("call i64 @imre_seq_borrow_index_proven")
                .unwrap();
            let merged = read_guard.find("phi i64").unwrap();
            assert!(cached < fallback && fallback < merged, "{body}");
            assert!(read_guard[..cached].contains("br i1"), "{body}");
        }
    }

    #[test]
    fn transitive_generator_append_keeps_live_sequence_reads() {
        let module = lower(
            "def grow(values: list[str]) -> bool:\n    values.append('added')\n    return True\n\ndef scan(values: list[str]) -> bool:\n    return any(value for value in values if grow(values))\n",
        );
        let facts = analyze(&module);
        assert!(facts[1].rooted[0]);
        assert!(facts[1].length_roots.iter().all(Option::is_none));
        let body = emitted_body(&module, 1);
        assert!(!body.contains("call ptr @imre_seq_data_readonly"), "{body}");
        assert!(
            body.contains("call i64 @imre_seq_borrow_index_proven"),
            "{body}"
        );
    }

    #[test]
    fn transitive_append_keeps_borrowing_but_disables_length_snapshots() {
        let append = function(
            vec![0],
            vec![Type::ListInt],
            vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::Append,
                    vec![
                        local(0, Type::ListInt),
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("1".into()),
                        },
                    ],
                ),
            })],
        );
        let caller = function(
            vec![0],
            vec![Type::ListInt; 2],
            vec![
                Statement::Assign(1, local(0, Type::ListInt)),
                Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Scope {
                        body: vec![Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Call(
                                0,
                                CallArgs::positional(vec![local(1, Type::ListInt)]),
                            ),
                        })],
                        value: Box::new(Expr {
                            ty: Type::None,
                            kind: ExprKind::None,
                        }),
                        clear: Vec::new(),
                    },
                }),
            ],
        );
        let facts = analyze(&module(vec![append, caller]));
        assert!(facts[1].locals[1]);
        assert_eq!(facts[1].length_roots, [None, None]);
    }

    #[test]
    fn different_parameter_aliases_and_bytes_lengths_are_not_snapshotted() {
        let function = function(
            vec![0, 1, 2],
            vec![Type::ListInt, Type::ListInt, Type::Bytes, Type::ListInt],
            vec![
                Statement::Assign(3, local(0, Type::ListInt)),
                Statement::Assign(3, local(1, Type::ListInt)),
            ],
        );
        let facts = analyze(&module(vec![function]));
        assert!(facts[0].locals[3]);
        assert_eq!(facts[0].length_roots, [Some(0), Some(1), None, None]);
    }
}
