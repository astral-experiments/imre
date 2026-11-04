//! Skip an ASCII case conversion when a string set excludes its byte length.
use crate::ir::{self, Builtin, Expr, ExprKind, Function, Module, Statement, TextOperation, Type};
use crate::types::Types;

/// The guard and the ordinary membership test may both read this receiver.
fn stable_string_set(types: &Types, value: &Expr) -> bool {
    types.set_element(value.ty) == Some(Type::Str)
        && (matches!(value.kind, ExprKind::Local(_))
            || matches!(&value.kind, ExprKind::Builtin(Builtin::GlobalGet(_), args) if args.is_empty()))
}

/// Finds the operand of an exact native string case conversion.
fn case_input(value: &mut Expr) -> Option<&mut Expr> {
    if let ExprKind::Builtin(Builtin::Text(TextOperation::Lower | TextOperation::Upper), args) =
        &mut value.kind
        && let [text] = args.as_mut_slice()
        && text.ty == Type::Str
    {
        Some(text)
    } else {
        None
    }
}

/// Builds the length guard for an already evaluated string.
fn possible(set: &Expr, text: usize) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Builtin(
            Builtin::SetCaseLengthPossible,
            vec![set.clone(), Expr::local(text, Type::Str)],
        ),
    }
}

/// The miss result must be independent of the skipped assignment.
fn independent(value: &Expr, target: usize) -> bool {
    match &value.kind {
        ExprKind::None
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_) => true,
        ExprKind::Local(slot) => *slot != target,
        ExprKind::Present(value) | ExprKind::Absent(value) => independent(value, target),
        _ => false,
    }
}

/// A normalizer may return the converted spelling on a hit. On a definite
/// miss, its final return bypasses the assignment without exposing the local.
fn guard_return(function: &mut Function, types: &Types) {
    let [
        ..,
        assignment,
        Statement::If(test, _, otherwise),
        Statement::Return(miss),
    ] = function.body.as_mut_slice()
    else {
        return;
    };
    let Statement::Assign(target, converted) = assignment else {
        return;
    };
    let ExprKind::Builtin(Builtin::SetContains, args) = &test.kind else {
        return;
    };
    let [key, set] = args.as_slice() else { return };
    if !otherwise.is_empty()
        || !matches!(key.kind, ExprKind::Local(slot) if slot == *target)
        || !stable_string_set(types, set)
        || !independent(miss, *target)
    {
        return;
    }
    let Some(input) = case_input(converted) else {
        return;
    };
    let temp = function.locals.len();
    function.locals.push(Type::Str);
    let source = std::mem::replace(input, Expr::local(temp, Type::Str));
    let skip = Expr {
        ty: Type::Bool,
        kind: ExprKind::Not(Box::new(possible(set, temp))),
    };
    let original = std::mem::replace(assignment, Statement::Clear(temp));
    *assignment = Statement::Scope {
        body: vec![
            Statement::Assign(temp, source),
            Statement::If(skip, vec![Statement::Return(miss.clone())], vec![]),
            original,
        ],
        clear: vec![temp],
    };
}

/// Guard inline membership and normalize-and-return functions. A private slot
/// evaluates the source exactly once, even when obtaining it has side effects.
pub(crate) fn specialize(module: &mut Module) {
    for function in module.functions.iter_mut().chain([&mut module.entry]) {
        let original_locals = function.locals.len();
        let types = &module.types;
        guard_return(function, types);
        ir::walk_expressions_mut(&mut function.body, &mut |expr| {
            let ExprKind::Builtin(Builtin::SetContains, args) = &mut expr.kind else {
                return;
            };
            let [key, set] = args.as_mut_slice() else {
                return;
            };
            if !stable_string_set(types, set) {
                return;
            }
            let Some(input) = case_input(key) else { return };
            // The preorder walk also visits the ordinary conversion in our fallback.
            if matches!(input.kind, ExprKind::Local(slot) if slot >= original_locals) {
                return;
            }
            let temp = function.locals.len();
            function.locals.push(Type::Str);
            let source = std::mem::replace(input, Expr::local(temp, Type::Str));
            let check = possible(set, temp);
            let ordinary = std::mem::replace(expr, Expr::none());
            let absent = Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(false),
            };
            *expr = Expr {
                ty: Type::Bool,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(temp, source)],
                    value: Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Conditional(
                            Box::new(check),
                            Box::new(ordinary),
                            Box::new(absent),
                        ),
                    }),
                    clear: vec![temp],
                },
            };
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts guards in the lowered program, including the early return on a definite miss.
    fn guards(source: &str) -> usize {
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        specialize(&mut module);
        let mut count = 0;
        for function in module.functions.iter().chain([&module.entry]) {
            ir::walk_expressions(&function.body, &mut |value| {
                if matches!(
                    value.kind,
                    ExprKind::Builtin(Builtin::SetCaseLengthPossible, _)
                ) {
                    count += 1;
                }
            });
        }
        count
    }

    #[test]
    fn guards_case_membership_with_an_independent_miss() {
        assert_eq!(
            guards(
                "def check(s: str, names: set[str]) -> bool:\n    return s.lower() in names\nprint(check('A', {'a'}))\n"
            ),
            1
        );
        assert_eq!(
            guards(
                "def find(s: str, names: set[str]) -> str | None:\n    key = s.upper()\n    if key in names:\n        return key\n    return None\nfind('a', {'A'})\n"
            ),
            1
        );
    }

    #[test]
    fn preserves_a_converted_miss_and_computed_set_receiver() {
        assert_eq!(
            guards(
                "def find(s: str, names: set[str]) -> str:\n    key = s.upper()\n    if key in names:\n        return s\n    return key\nprint(find('a', {'A'}))\n"
            ),
            0
        );
        assert_eq!(
            guards(
                "def names() -> set[str]:\n    return {'x'}\ndef check(s: str) -> bool:\n    return s.upper() in names()\nprint(check('x'))\n"
            ),
            0
        );
    }
}
