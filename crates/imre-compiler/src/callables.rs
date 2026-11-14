//! Closed target sets for exact callable signatures in the final typed program.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{LoweringDiagnostic, LoweringKind};

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
pub(crate) struct Targets(SourceTargets);

/// The checked call graph follows every constructed source function, even when
/// a builtin of the same signature also has a runtime entry point.
pub(crate) type SourceTargets = HashMap<Type, Vec<Target>>;

/// Collect source targets and the extra operands needed by the native calling convention.
#[derive(Default)]
pub(crate) struct TargetCollector {
    pub(crate) constructed: SourceTargets,
    wrapped: Vec<(Type, Target)>,
    open: HashSet<Type>,
    wrapper_owners: Vec<usize>,
    pub(crate) constructors: BTreeMap<usize, Type>,
}

impl TargetCollector {
    pub(crate) fn inspect(&mut self, id: usize, types: &crate::types::Types, value: &ir::Expr) {
        if matches!(
            value.kind,
            ExprKind::Builtin(
                ir::Builtin::FunctionWrappedSet | ir::Builtin::FunctionWrappedGet(_),
                _
            )
        ) && self.wrapper_owners.last() != Some(&id)
        {
            self.wrapper_owners.push(id);
        }
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
            for ty in types.source_callable_types(value.ty) {
                self.constructed.entry(ty).or_default().push(target);
            }
            if !target.bound
                && let Some(class) = types.constructor_class(*function, value.ty)
            {
                self.constructors.entry(class).or_insert(value.ty);
            }
            if let Some(raw) = types.original_cache_body(*function) {
                let signature = types.wrapped_signature(value.ty).unwrap_or(value.ty);
                self.wrapped.push((
                    signature,
                    Target {
                        function: raw,
                        bound: false,
                    },
                ));
            }
            if let Some(signature) = types.wrapped_signature(value.ty) {
                // A wrapper can cross a plain Callable parameter or return
                // without losing its real invocation target.
                self.wrapped.push((signature, target));
            }
        }
    }

    pub(crate) fn deduplicate(&mut self) {
        for targets in self.constructed.values_mut() {
            targets.sort_unstable();
            targets.dedup();
        }
    }

    fn finish(mut self) -> Targets {
        for (ty, target) in self.wrapped.drain(..) {
            self.constructed.entry(ty).or_default().push(target);
        }
        self.constructed.retain(|ty, _| !self.open.contains(ty));
        self.deduplicate();
        Targets(self.constructed)
    }

    #[cfg(test)]
    pub(crate) fn analyze(module: &Module) -> Self {
        let mut collector = Self::default();
        for (id, function) in module.functions.iter().chain([&module.entry]).enumerate() {
            ir::walk_expressions(&function.body, &mut |value| {
                collector.inspect(id, &module.types, value);
            });
        }
        collector
    }
}

impl Targets {
    #[cfg(test)]
    pub(crate) fn analyze(module: &Module) -> Self {
        TargetCollector::analyze(module).finish()
    }

    /// An empty set is unknown, not proof that an invocation is unreachable.
    pub(crate) fn get(&self, ty: Type) -> &[Target] {
        self.0.get(&ty).map_or(&[], Vec::as_slice)
    }
}

/// Keep operands whose origin cannot be established from local writes and every closed call site.
pub(crate) fn retain_unproven_operands(
    functions: &[ir::Function],
    entry: &ir::Function,
    types: &crate::types::Types,
    operands: &mut BTreeMap<usize, Vec<&ir::Expr>>,
    proven: impl Fn(&ir::Expr) -> bool,
    may_enter_uncalled: impl Fn(usize) -> bool,
) {
    let mut direct_inputs: HashMap<_, Vec<_>> = operands
        .keys()
        .filter_map(|&id| {
            let function = functions.get(id)?;
            (!function.params.is_empty()).then(|| (id, Vec::new()))
        })
        .collect();
    if !direct_inputs.is_empty() {
        for function in functions.iter().chain([entry]) {
            ir::walk_expressions(&function.body, &mut |expr| match &expr.kind {
                ExprKind::Call(id, args) => {
                    if let Some(inputs) = direct_inputs.get_mut(id) {
                        inputs.push(args);
                    }
                }
                ExprKind::Callable { function, .. } => {
                    direct_inputs.remove(function);
                    if let Some(raw) = types.original_cache_body(*function) {
                        direct_inputs.remove(&raw);
                    }
                }
                ExprKind::Construct { initializer, .. } => {
                    direct_inputs.remove(initializer);
                }
                _ => {}
            });
        }
    }
    operands.retain(|&id, owners| {
        let function = functions.get(id).unwrap_or(entry);
        let inputs = direct_inputs.get(&id);
        ir::retain_unproven_operands(function, owners, inputs.map_or(&[], Vec::as_slice), &proven);
        !owners.is_empty() && (!inputs.is_some_and(Vec::is_empty) || may_enter_uncalled(id))
    });
}

/// Callable representation selected from the original checked operations.
pub(crate) struct Layouts {
    pub(crate) targets: Targets,
    pub(crate) linked_functions: HashSet<usize>,
}

pub(crate) fn layouts(
    module: &ir::Module,
    targets: TargetCollector,
) -> Result<Layouts, LoweringDiagnostic> {
    let mut function_link_types = HashSet::new();
    let mut link_owners = BTreeMap::<usize, Vec<&ir::Expr>>::new();
    for &id in &targets.wrapper_owners {
        let function = module.functions.get(id).unwrap_or(&module.entry);
        ir::walk_expressions(&function.body, &mut |expr| {
            let ExprKind::Builtin(builtin, args) = &expr.kind else {
                return;
            };
            match builtin {
                ir::Builtin::FunctionWrappedSet | ir::Builtin::FunctionWrappedGet(_) => {
                    let owner = &args[0];
                    function_link_types.insert(owner.ty);
                    link_owners.entry(id).or_default().push(owner);
                }
                _ => {}
            }
        });
    }
    let callable_targets = targets.finish();
    let cached = |id| module.types.cache_adapter(id).is_some();
    let unknown_signature = || {
        LoweringDiagnostic::plain(
            "Imre: wrapper links require a callable signature with known native functions or source methods".to_owned(),
        )
        .classified(LoweringKind::Object)
    };
    if function_link_types
        .iter()
        .any(|&ty| module.types.dispatcher(ty).is_some() || callable_targets.get(ty).is_empty())
    {
        return Err(unknown_signature());
    }
    let linked_functions = function_link_types
        .iter()
        .flat_map(|&ty| callable_targets.get(ty))
        .filter(|target| !target.bound)
        .map(|target| target.function)
        .collect::<HashSet<_>>();
    link_owners.retain(|_, owners| {
        owners.retain(|owner| {
            callable_targets
                .get(owner.ty)
                .iter()
                .any(|target| target.bound && !cached(target.function))
        });
        !owners.is_empty()
    });

    retain_unproven_operands(
        &module.functions,
        &module.entry,
        &module.types,
        &mut link_owners,
        |value| {
            matches!(&value.kind, ExprKind::Builtin(ir::Builtin::StaticFunction(_), args)
            if matches!(args[0].kind, ExprKind::Callable { function, receiver: None }
                if !cached(function)
                    && module.types.constructor_base(args[0].ty).is_none()
                    && linked_functions.contains(&function)))
        },
        |_| true,
    );
    // Only the owner needs link storage. A plain function can wrap a bound
    // builtin, even when the two callables have the same signature.
    if link_owners.values().flatten().any(|owner| {
        callable_targets.get(owner.ty).iter().any(|target| {
            let function = &module.functions[target.function];
            target.bound
                && !cached(target.function)
                && !matches!(function.locals[function.params[0]], Type::Class(_))
        })
    }) {
        return Err(unknown_signature());
    }
    if !link_owners.is_empty() {
        return Err(LoweringDiagnostic::plain(
            "Imre: native wrapper links require a plain function as their owner".to_owned(),
        )
        .classified(LoweringKind::ScopedObject));
    }
    Ok(Layouts {
        targets: callable_targets,
        linked_functions,
    })
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
