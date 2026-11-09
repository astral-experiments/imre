//! Close a checked function and its implementation helpers for a registered activation.
use super::*;

pub(crate) struct RegisteredNative {
    pub(crate) module: ir::Module,
    pub(crate) parameters: Vec<String>,
    pub(crate) local_names: HashMap<usize, String>,
    pub(crate) site: TextRange,
    /// These free reads must still select the builtins assumed by the checked operations.
    pub(crate) builtins: Vec<String>,
}

pub(super) fn capture(
    mut checker: Checker<'_>,
    function: &ast::StmtFunctionDef,
    signature: &Signature,
    first_helper: usize,
    helpers: &Helpers,
) -> Option<RegisteredNative> {
    if signature.params.is_empty()
        || signature.returns != Type::Int
        || !signature
            .params
            .iter()
            .all(|p| matches!(p.ty, Type::Int | Type::Str))
        || function.is_async
        || !function.decorator_list.is_empty()
        || function.type_params.is_some()
        || function.parameters.vararg.is_some()
        || function.parameters.kwarg.is_some()
    {
        return None;
    }
    let graph = checker.graph;
    let source = &graph.modules[checker.module];
    let builtin = |name: &str| ruff_python_stdlib::builtins::is_python_builtin(name, 14, false);
    // Some fixed-layout builtin folds consume the converter by name. Such a fold cannot
    // be moved to a registered activation where a local binding supplies the converter.
    if signature.params.iter().any(|p| builtin(&p.name))
        || narrowing::assigned_names(&function.body)
            .iter()
            .any(|name| builtin(name))
    {
        return None;
    }
    let mut builtins: Vec<_> = comprehensions::function_reads(function)
        .into_iter()
        .collect();
    if !builtins
        .iter()
        .all(|name| builtin(name) && !source.bindings.contains_key(name))
    {
        return None;
    }
    builtins.sort_unstable();
    let owner = checker.module;
    checker.native_activation = true;
    let mut local_names = HashMap::new();
    let (body, sums) = graph
        .try_complete_body(owner, || {
            lower_function(checker, function, signature, Some(&mut local_names))
        })?
        .ok()?;
    if !body.class_reads.is_empty() || !body.calls.iter().all(|&id| id >= first_helper) {
        return None;
    }
    let list_builds = helpers
        .list_builds
        .take()
        .into_iter()
        .map(|(id, plan)| (1 + id - first_helper, plan))
        .collect();
    let mut functions = vec![body.function];
    for helper in helpers.take_functions() {
        if helper.module != owner
            || helper.source_frame
            || !helper.class_reads.is_empty()
            || !helper.calls.iter().all(|&id| id >= first_helper)
        {
            return None;
        }
        functions.push(helper.function);
    }
    let end = first_helper + functions.len() - 1;
    for f in &mut functions {
        if !f.locals.iter().copied().all(compact_type) || !compact_type(f.returns) {
            return None;
        }
        let mut closed = true;
        ir::walk_statements(&f.body, &mut |s| {
            closed &= matches!(
                s,
                Statement::Assign(..)
                    | Statement::Move(..)
                    | Statement::Clear(..)
                    | Statement::Scope { .. }
                    | Statement::Expr(_)
                    | Statement::If(..)
                    | Statement::While(..)
                    | Statement::For { .. }
                    | Statement::Return(_)
                    | Statement::Break
                    | Statement::Continue
            );
        });
        ir::walk_expressions_mut(&mut f.body, &mut |v| {
            closed &= compact_type(v.ty)
                && match &mut v.kind {
                    ExprKind::Int(_)
                    | ExprKind::Bool(_)
                    | ExprKind::None
                    | ExprKind::Str(_)
                    | ExprKind::Local(_)
                    | ExprKind::List(_)
                    | ExprKind::Binary(..)
                    | ExprKind::Compare(..)
                    | ExprKind::Boolean(..)
                    | ExprKind::Conditional(..)
                    | ExprKind::Not(_)
                    | ExprKind::Negate(_) => true,
                    ExprKind::Call(id, _) if (first_helper..end).contains(id) => {
                        *id = 1 + *id - first_helper;
                        true
                    }
                    ExprKind::Builtin(b, _) => matches!(
                        b,
                        Builtin::StringIntList
                            | Builtin::Int
                            | Builtin::Len
                            | Builtin::ListGet
                            | Builtin::ListSum
                            | Builtin::RegisteredStatement(..)
                    ),
                    _ => false,
                };
        });
        if !closed {
            return None;
        }
    }
    if !sums.is_empty() {
        return None;
    }
    // All types above have stable primitive representations. There are no interned IDs,
    // class layouts, module globals, foreign call targets or separately owned frames.
    let mut module = ir::Module {
        builtin_module_sources: None,
        command_flags: None,
        types: Types::default(),
        class_metadata: Vec::new(),
        class_fields: Vec::new(),
        globals: Vec::new(),
        json_records: Vec::new(),
        list_builds,
        source_frames: Vec::new(),
        functions,
        entry: ir::Function {
            params: Vec::new(),
            locals: Vec::new(),
            returns: Type::None,
            body: Vec::new(),
        },
    };
    crate::specialize(&mut module).ok()?;
    Some(RegisteredNative {
        module,
        local_names,
        site: function.range(),
        parameters: signature.params.iter().map(|p| p.name.clone()).collect(),
        builtins,
    })
}

fn compact_type(ty: Type) -> bool {
    matches!(
        ty,
        Type::Int | Type::Bool | Type::None | Type::Str | Type::ListInt
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_ir_keeps_application_bindings_and_object_effects_on_the_registered_path() {
        let graph = SourceGraph::single(concat!(
            "settings = 7\n",
            "def pending(value: int, sink):\n    sink(value)\n",
            "def member(value: int) -> int:\n    return value + settings\n",
            "def explicit_effect(value: int) -> int:\n    print(value)\n    return value\n",
            "def shadow(text: str, int: int) -> int:\n    return sum(list(map(int, text)))\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        assert!(graph.native_functions.borrow().is_empty());
    }

    #[test]
    fn keeps_checked_callee_and_outlined_reduction() {
        let graph = SourceGraph::single(concat!(
            "def send_result(x: int, sink):\n    sink(x)\n",
            "def reduction(s: str) -> int:\n",
            "    values = list(map(int, s))\n",
            "    return sum([n * (n + 1) for n in values])\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let closures = graph.native_functions.borrow();
        let closure = closures
            .get(&graph.entry)
            .and_then(|m| m.values().next())
            .expect("checked function retained");
        assert_eq!(closure.builtins, ["int", "list", "map", "sum"]);
        assert_eq!(closure.module.functions.len(), 2);
        assert!(crate::verify::module(&closure.module).is_ok());
    }
}
