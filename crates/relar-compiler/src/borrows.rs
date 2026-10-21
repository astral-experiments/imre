//! Borrows projections only while unchanged caller roots preserve every heap edge.

use crate::ir::{self, Builtin, Expr, ExprKind, Function, Module, Statement, Type};
use crate::list_specialization::readonly_builtin;
use crate::types::Types;

#[derive(Default)]
pub(crate) struct FunctionBorrows {
    /// Includes stable incoming parameters and their proven local projections.
    pub rooted: Vec<bool>,
    /// These local slots carry borrowed words, never independent owners.
    pub locals: Vec<bool>,
}

/// Computes closed-callgraph edge preservation before deriving local roots.
/// The separate initializer summary applies only to a newly allocated receiver.
pub(crate) fn analyze(module: &Module) -> Vec<FunctionBorrows> {
    // Unknown callees and unexpected recursive cycles retain ordinary owners.
    // The checked call graph is acyclic, so proving upward loses no precision.
    let mut preserves = vec![false; module.functions.len()];
    let mut initializes = preserves.clone();
    loop {
        let mut changed = false;
        for (id, function) in module.functions.iter().enumerate() {
            let safe = preserves_edges(function, &preserves, &initializes, false);
            let fresh = preserves_edges(function, &preserves, &initializes, true);
            changed |= preserves[id] != safe || initializes[id] != fresh;
            preserves[id] = safe;
            initializes[id] = fresh;
        }
        if !changed {
            break;
        }
    }
    module
        .functions
        .iter()
        .chain([&module.entry])
        .map(|function| {
            if preserves_edges(function, &preserves, &initializes, false) {
                local_roots(function, &module.types)
            } else {
                FunctionBorrows {
                    rooted: vec![false; function.locals.len()],
                    locals: vec![false; function.locals.len()],
                }
            }
        })
        .collect()
}

/// Visits statement nodes once, including statements nested inside expressions.
fn statements(body: &[Statement], visit: &mut impl FnMut(&Statement)) {
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
    let mut written = function.expression_rebound();
    statements(&function.body, &mut |statement| match statement {
        Statement::Assign(slot, _) | Statement::Clear(slot) => written[*slot] = true,
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
            Statement::SetItem(..) | Statement::AugmentItem(..) | Statement::AugmentField(..) => {
                false
            }
            Statement::SetField(value, _, _) => {
                matches!(value.kind, ExprKind::Local(slot) if Some(slot) == receiver)
            }
            _ => true,
        };
    });
    ir::walk_expressions(&function.body, &mut |expression| {
        safe &= match &expression.kind {
            ExprKind::Call(id, _) => calls.get(*id).copied().unwrap_or(false),
            ExprKind::Construct { initializer, .. } => {
                constructors.get(*initializer).copied().unwrap_or(false)
            }
            ExprKind::Builtin(operation, _) => {
                readonly_builtin(operation)
                    || matches!(operation, Builtin::Append | Builtin::ListReserveRange)
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
pub(crate) fn projection_root(expression: &Expr, types: &Types) -> Option<usize> {
    if types.payload(expression.ty) == Some(Type::Float) {
        return None;
    }
    match &expression.kind {
        ExprKind::Local(slot) => Some(*slot),
        ExprKind::Field(receiver, _) => projection_root(receiver, types),
        ExprKind::Builtin(Builtin::SequenceGetInBounds, args) => projection_root(&args[0], types),
        ExprKind::Present(value) if value.ty != Type::Float => projection_root(value, types),
        ExprKind::Unwrap(value) if types.payload(value.ty) != Some(Type::Float) => {
            projection_root(value, types)
        }
        ExprKind::Scope { value, .. } => projection_root(value, types),
        _ => None,
    }
}

/// A local is borrowed only if all its definitions preserve an incoming root.
/// Elimination rejects mixed owned definitions; reachability rejects rootless cycles.
fn local_roots(function: &Function, types: &Types) -> FunctionBorrows {
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
            if let Some(source) = projection_root(value, types) {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::CallArgs;

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
            types: Types::default(),
            class_fields: vec![vec![Type::Class(0)]],
            globals: Vec::new(),
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
        let facts = analyze(&module(vec![initializer, fresh, ordinary]));
        assert!(facts[1].locals[1]);
        assert!(!facts[0].rooted.iter().any(|&rooted| rooted));
        assert!(!facts[2].rooted.iter().any(|&rooted| rooted));
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
        assert_eq!(projection_root(&boxed, &types), None);
        assert_eq!(projection_root(&local(0, optional), &types), None);
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
        let llvm = crate::llvm::emit(module, &ranges, false);
        llvm.split_once(&format!("@fn_{id}("))
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0
            .to_owned()
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
        assert!(body.contains("call i64 @relar_seq_borrow_index_proven"));
        assert!(!body.contains("call void @relar_record_retain"), "{body}");
        assert!(!body.contains("call void @relar_record_release"), "{body}");
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
    for row in rows:
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
            body.matches("call void @relar_record_retain").count(),
            1,
            "{body}"
        );
        assert!(!body.contains("call void @relar_record_release"), "{body}");
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
        for row in rows:
            if fail:
                raise ValueError("failed")
            result += row.value
    finally:
        for row in rows:
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
        assert!(body.contains("call void @relar_exception_raise"));
        assert!(!body.contains("call void @relar_record_retain"), "{body}");
        assert!(!body.contains("call void @relar_record_release"), "{body}");
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
        // One acquisition transfers the borrow to slot2; the other returns it.
        assert_eq!(
            body.matches("call void @relar_str_retain").count(),
            2,
            "{body}"
        );
        let acquisition = body.find("call void @relar_str_retain").unwrap();
        let replacement = body[acquisition..]
            .find("call void @relar_str_release")
            .unwrap();
        assert!(body[acquisition..acquisition + replacement].contains("ptr %local2"));
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
}
