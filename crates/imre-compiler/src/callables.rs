//! Closed target sets for exact callable signatures in the final typed program.

use std::collections::{HashMap, HashSet};

use crate::ir::{self, ExprKind, Module, Type};

/// Bound targets use the callable record's receiver as the first native argument.
/// Their effects still come from the ordinary function, never a fresh initializer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Target {
    pub function: usize,
    pub bound: bool,
}

#[derive(Default)]
pub(crate) struct Targets(HashMap<Type, Vec<Target>>);

impl Targets {
    /// Join constructions of each signature across returns, forwarding and selection.
    /// A signature originating in the runtime has an open set of targets.
    pub(crate) fn analyze(module: &Module) -> Self {
        let mut targets = Self::default();
        let mut open = HashSet::new();
        for function in module.functions.iter().chain([&module.entry]) {
            ir::walk_expressions(&function.body, &mut |value| {
                if matches!(value.kind, ExprKind::Builtin(ir::Builtin::BuiltinsLen, _)) {
                    // The original builtin has a runtime entry point. A singleton Python
                    // function of this signature cannot prove the target of a live lookup.
                    open.insert(value.ty);
                }
                if let ExprKind::Callable { function, receiver } = &value.kind {
                    let target = Target {
                        function: *function,
                        bound: receiver.is_some(),
                    };
                    targets.0.entry(value.ty).or_default().push(target);
                    if let Some(signature) = module.types.cache_signature(value.ty) {
                        // A cache can cross a plain Callable parameter or return
                        // without losing its real invocation target.
                        targets.0.entry(signature).or_default().push(target);
                    }
                }
            });
        }
        targets.0.retain(|ty, _| !open.contains(ty));
        for values in targets.0.values_mut() {
            values.sort_unstable();
            values.dedup();
        }
        targets
    }

    /// An empty set is unknown, not proof that an invocation is unreachable.
    pub(crate) fn get(&self, ty: Type) -> &[Target] {
        self.0.get(&ty).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Expr, Function, Statement};

    fn function(body: Vec<Statement>) -> Function {
        Function {
            params: Vec::new(),
            returns: Type::None,
            locals: Vec::new(),
            body,
        }
    }

    #[test]
    fn nested_and_entry_constructions_join_by_exact_signature() {
        let callable = |ty, function, receiver| Expr {
            ty,
            kind: ExprKind::Callable { function, receiver },
        };
        let first = Type::Callable(0);
        let second = Type::Callable(1);
        let module = Module {
            ignore_python_environment: false,
            types: Default::default(),
            class_fields: Vec::new(),
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            functions: vec![function(vec![Statement::Expr(Expr {
                ty: first,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(callable(first, 3, None))],
                    value: Box::new(callable(first, 1, None)),
                    clear: Vec::new(),
                },
            })])],
            entry: function(vec![
                Statement::Expr(callable(first, 3, None)),
                Statement::Expr(callable(
                    first,
                    1,
                    Some(Box::new(Expr {
                        ty: Type::Class(0),
                        kind: ExprKind::Scope {
                            body: vec![Statement::Expr(callable(second, 2, None))],
                            value: Box::new(Expr {
                                ty: Type::Class(0),
                                kind: ExprKind::Local(0),
                            }),
                            clear: Vec::new(),
                        },
                    })),
                )),
            ]),
        };
        let targets = Targets::analyze(&module);
        assert_eq!(
            targets.get(first),
            [
                Target {
                    function: 1,
                    bound: false
                },
                Target {
                    function: 1,
                    bound: true
                },
                Target {
                    function: 3,
                    bound: false
                },
            ]
        );
        assert_eq!(
            targets.get(second),
            [Target {
                function: 2,
                bound: false
            }]
        );
        assert!(targets.get(Type::Callable(2)).is_empty());
    }
}
