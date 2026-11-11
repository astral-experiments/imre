//! Closed target sets for exact callable signatures in the final typed program.

use std::collections::{HashMap, HashSet};

#[cfg(test)]
use crate::ir::Module;
use crate::ir::{self, ExprKind, Type};

/// Bound targets use the callable record's receiver as the first native argument.
/// Their effects still come from the ordinary function, never a fresh initializer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Target {
    pub function: usize,
    pub bound: bool,
}

#[derive(Default)]
pub(crate) struct Targets(HashMap<Type, Vec<Target>>);

/// Collect closed targets and open runtime entries during the checked module inventory.
#[derive(Default)]
pub(crate) struct TargetCollector {
    targets: Targets,
    open: HashSet<Type>,
}

impl TargetCollector {
    pub(crate) fn inspect(&mut self, types: &crate::types::Types, value: &ir::Expr) {
        if matches!(
            value.kind,
            ExprKind::Builtin(
                ir::Builtin::BuiltinsLen | ir::Builtin::FnmatchFunction { .. },
                _
            )
        ) {
            // These builtins have runtime entry points. A singleton Python
            // function of this signature cannot prove the target of a live lookup.
            self.open.insert(value.ty);
        }
        if let ExprKind::Callable { function, receiver } = &value.kind {
            let target = Target {
                function: *function,
                bound: receiver.is_some(),
            };
            self.targets.0.entry(value.ty).or_default().push(target);
            if let Some(raw) = types.original_cache_body(*function) {
                let signature = types.wrapped_signature(value.ty).unwrap_or(value.ty);
                self.targets.0.entry(signature).or_default().push(Target {
                    function: raw,
                    bound: false,
                });
            }
            if let Some(signature) = types.wrapped_signature(value.ty) {
                // A wrapper can cross a plain Callable parameter or return
                // without losing its real invocation target.
                self.targets.0.entry(signature).or_default().push(target);
            }
        }
    }

    pub(crate) fn finish(self) -> Targets {
        let Self { mut targets, open } = self;
        targets.0.retain(|ty, _| !open.contains(ty));
        for values in targets.0.values_mut() {
            values.sort_unstable();
            values.dedup();
        }
        targets
    }
}

impl Targets {
    #[cfg(test)]
    pub(crate) fn analyze(module: &Module) -> Self {
        let mut collector = TargetCollector::default();
        for function in module.functions.iter().chain([&module.entry]) {
            ir::walk_expressions(&function.body, &mut |value| {
                collector.inspect(&module.types, value);
            });
        }
        collector.finish()
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
            types: Default::default(),
            class_fields: Vec::new(),
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            command_flags: None,
            builtin_module_sources: None,
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
                            value: Box::new(Expr::local(0, Type::Class(0))),
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
