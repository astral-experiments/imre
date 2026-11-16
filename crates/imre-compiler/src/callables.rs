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

/// A static function may present its declaring entry through an inherited receiver view.
pub(crate) fn singleton_entry(operand: &ir::Expr) -> &ir::Expr {
    if let ExprKind::Builtin(ir::Builtin::RecordView, args) = &operand.kind
        && let [entry] = args.as_slice()
    {
        entry
    } else {
        operand
    }
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
    wrapper_owners: Vec<(usize, Type)>,
    source_frames: Vec<bool>,
    pub(crate) constructors: BTreeMap<usize, Type>,
}

impl TargetCollector {
    pub(crate) fn inspect(&mut self, id: usize, types: &crate::types::Types, value: &ir::Expr) {
        if let Some(owner) = wrapped_owner(value)
            && self.wrapper_owners.last() != Some(&(id, owner.ty))
        {
            self.wrapper_owners.push((id, owner.ty));
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
        // The inherited spelling and the declaring entry can both call this singleton.
        if let ExprKind::Builtin(ir::Builtin::StaticFunction(function), args) = &value.kind
            && let [operand] = args.as_slice()
            && singleton_entry(operand).ty != value.ty
        {
            self.constructed.entry(value.ty).or_default().push(Target {
                function: *function,
                bound: false,
            });
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

    pub(crate) fn keep_source_frames(&mut self, frames: Vec<bool>) {
        if !self.wrapper_owners.is_empty() {
            self.source_frames = frames;
        }
    }

    pub(crate) fn plain_sources(&self, ty: Type, source: impl Fn(usize) -> bool) -> bool {
        !self.open.contains(&ty)
            && !self.wrapped.iter().any(|(signature, _)| *signature == ty)
            && self.constructed.get(&ty).is_some_and(|targets| {
                !targets.is_empty()
                    && targets
                        .iter()
                        .all(|target| !target.bound && source(target.function))
            })
    }

    pub(crate) fn deduplicate(&mut self) {
        for targets in self.constructed.values_mut() {
            targets.sort_unstable();
            targets.dedup();
        }
    }

    fn finish(mut self, link_types: &HashSet<Type>) -> Layouts {
        for (ty, target) in self.wrapped.drain(..) {
            self.constructed.entry(ty).or_default().push(target);
        }
        // An open signature can still have a known function whose record owns a
        // wrapper link. Only indirect calls require the complete set of targets.
        let linked_functions = link_types
            .iter()
            .filter_map(|ty| self.constructed.get(ty))
            .flatten()
            .filter(|target| !target.bound)
            .map(|target| target.function)
            .collect();
        self.constructed.retain(|ty, _| !self.open.contains(ty));
        self.deduplicate();
        Layouts {
            targets: Targets(self.constructed),
            linked_functions,
        }
    }

    #[cfg(test)]
    pub(crate) fn analyze(module: &Module) -> Self {
        let mut collector = Self::default();
        for (id, function) in module.functions.iter().chain([&module.entry]).enumerate() {
            ir::walk_expressions(&function.body, &mut |value| {
                collector.inspect(id, &module.types, value);
            });
        }
        if !collector.wrapper_owners.is_empty() {
            collector.source_frames = module.source_frames.iter().map(Option::is_some).collect();
        }
        collector
    }
}

impl Targets {
    #[cfg(test)]
    pub(crate) fn analyze(module: &Module) -> Self {
        TargetCollector::analyze(module)
            .finish(&HashSet::new())
            .targets
    }

    /// An empty set is unknown, not proof that an invocation is unreachable.
    pub(crate) fn get(&self, ty: Type) -> &[Target] {
        self.0.get(&ty).map_or(&[], Vec::as_slice)
    }
}

/// Keep operands whose origin cannot be established from local writes and every closed call site.
pub(crate) fn retain_unproven_operands<'a>(
    functions: &'a [ir::Function],
    entry: &'a ir::Function,
    types: &crate::types::Types,
    operands: &mut BTreeMap<usize, Vec<&'a ir::Expr>>,
    operand: impl Fn(&'a ir::Expr) -> Option<&'a ir::Expr>,
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
        ir::retain_unproven_operands(
            function,
            owners,
            inputs.map_or(&[], Vec::as_slice),
            &operand,
            &proven,
        );
        !owners.is_empty() && (!inputs.is_some_and(Vec::is_empty) || may_enter_uncalled(id))
    });
}

fn wrapped_owner(expr: &ir::Expr) -> Option<&ir::Expr> {
    match &expr.kind {
        ExprKind::Builtin(
            ir::Builtin::FunctionWrappedSet | ir::Builtin::FunctionWrappedGet(_),
            args,
        ) => Some(&args[0]),
        _ => None,
    }
}

/// Callable representation selected from the original checked operations.
pub(crate) struct Layouts {
    pub(crate) targets: Targets,
    pub(crate) linked_functions: HashSet<usize>,
}

pub(crate) fn layouts(
    module: &ir::Module,
    mut targets: TargetCollector,
) -> Result<Layouts, LoweringDiagnostic> {
    let wrapper_owners = std::mem::take(&mut targets.wrapper_owners);
    let mut link_types: HashSet<_> = wrapper_owners.iter().map(|&(_, ty)| ty).collect();
    let source_frames = std::mem::take(&mut targets.source_frames);
    let layouts = targets.finish(&link_types);
    let callable_targets = &layouts.targets;
    let cached = |id| module.types.cache_adapter(id).is_some();
    let unknown_signature = || {
        LoweringDiagnostic::plain(
            "Imre: wrapper links require a callable signature with known native functions or source methods".to_owned(),
        )
        .classified(LoweringKind::Object)
    };
    if link_types
        .iter()
        .any(|&ty| module.types.dispatcher(ty).is_some())
    {
        return Err(unknown_signature());
    }
    link_types.retain(|ty| {
        let targets = callable_targets.get(*ty);
        targets.is_empty()
            || targets
                .iter()
                .any(|target| target.bound && !cached(target.function))
    });
    let mut link_owners = BTreeMap::<usize, Vec<&ir::Expr>>::new();
    for (id, ty) in wrapper_owners {
        if link_types.contains(&ty) {
            link_owners.entry(id).or_default();
        }
    }
    retain_unproven_operands(
        &module.functions,
        &module.entry,
        &module.types,
        &mut link_owners,
        |value| wrapped_owner(value).filter(|owner| link_types.contains(&owner.ty)),
        |value| {
            // A lifted def allocates a fresh record. Some builtin adapters have
            // the same unbound call ABI, but do not own a Python source frame.
            let entry = match &value.kind {
                ExprKind::Builtin(ir::Builtin::StaticFunction(_), args) => &args[0],
                ExprKind::Callable { function, .. }
                    if source_frames.get(*function) == Some(&true) =>
                {
                    value
                }
                _ => return false,
            };
            matches!(entry.kind, ExprKind::Callable { function, receiver: None }
                if !cached(function)
                    && module.types.constructor_base(entry.ty).is_none()
                    && layouts.linked_functions.contains(&function))
        },
        |_| true,
    );
    // Only the owner needs link storage. A plain function can wrap a bound
    // builtin, even when the two callables have the same signature.
    if link_owners.values().flatten().any(|owner| {
        let targets = callable_targets.get(owner.ty);
        targets.is_empty()
            || targets.iter().any(|target| {
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
    Ok(layouts)
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
            recursive_functions: Default::default(),
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
