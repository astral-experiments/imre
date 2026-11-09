//! Close a checked function and its implementation helpers for a registered activation.
use super::*;

pub(crate) struct RegisteredNative {
    pub(crate) module: ir::Module,
    pub(crate) local_names: HashMap<usize, String>,
    pub(crate) frame_lists: Vec<(usize, String)>,
    pub(crate) site: TextRange,
    /// These free reads must still select the builtins assumed by the checked operations.
    pub(crate) builtins: Vec<String>,
}

/// Keep the exact checked body at its original definition. A selected body supplies the
/// ordinary checker facts for this definition as well as its native registered entry.
pub(super) fn capture(
    checker: Checker<'_>,
    function: &ast::StmtFunctionDef,
    signature: &Signature,
    first_helper: usize,
    selected: bool,
) -> bool {
    let graph = checker.graph;
    let module = checker.module;
    let native = graph.try_complete_body(module, selected, || {
        close(checker, function, signature, first_helper)
    });
    if let Some(native) = native {
        graph
            .native_functions
            .borrow_mut()
            .entry(module)
            .or_default()
            .insert(native.site.start(), native);
        true
    } else {
        false
    }
}

fn close(
    mut checker: Checker<'_>,
    function: &ast::StmtFunctionDef,
    signature: &Signature,
    first_helper: usize,
) -> Option<RegisteredNative> {
    let helpers = checker.helpers;
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
    let (body, sums) = lower_function(checker, function, signature, Some(&mut local_names)).ok()?;
    if !body.class_reads.is_empty() || !body.calls.iter().all(|&id| id >= first_helper) {
        return None;
    }
    if !body
        .function
        .params
        .iter()
        .zip(&signature.params)
        .all(|(slot, param)| local_names.get(slot) == Some(&param.name))
    {
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
    if !sums.is_empty() || !faithful_integer_result(&functions) {
        return None;
    }
    let frame_lists = frame_lists(&functions, &local_names)?;
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
        frame_lists,
        site: function.range(),
        builtins,
    })
}

/// Source parameters keep their registered bindings. An exceptional exit can publish a
/// uniquely owned list directly from its slot, including the unassigned null state.
fn frame_lists(
    functions: &[ir::Function],
    names: &HashMap<usize, String>,
) -> Option<Vec<(usize, String)>> {
    let root = &functions[0];
    // A reassignment across representations can hide an earlier source slot in the
    // final name table. Leave such roots registered until their binding history is kept.
    if root.locals.len() != names.len() {
        return None;
    }
    let mut lists = Vec::new();
    for (&slot, name) in names {
        if !root.params.contains(&slot) {
            if root.locals[slot] != Type::ListInt {
                return None;
            }
            lists.push((slot, name.clone()));
        }
    }
    let mut seen = HashSet::new();
    let mut complete = true;
    ir::walk_writes(&root.body, |slot, value| {
        complete &= !root.params.contains(&slot)
            && value
                .is_some_and(|v| matches!(v.kind, ExprKind::Builtin(Builtin::StringIntList, _)))
            && seen.insert(slot);
    });
    if !complete
        || lists.len() != seen.len()
        || !lists.is_empty()
            && functions[1..]
                .iter()
                .any(|helper| helper.returns == Type::ListInt)
    {
        return None;
    }
    lists.sort_unstable_by_key(|(slot, _)| *slot);
    Some(lists)
}

/// Converting a native integer result to the Python heap cannot retain the identity
/// of a borrowed argument or a non-permanent literal. Check the producers of every
/// return, including each definition of a private accumulator in an outlined helper.
fn faithful_integer_result(functions: &[ir::Function]) -> bool {
    let mut independent = vec![false; functions.len()];
    loop {
        let mut changed = false;
        for (id, function) in functions.iter().enumerate().rev() {
            if independent[id] || function.returns != Type::Int {
                continue;
            }
            let mut written = HashSet::new();
            let mut borrowed: HashSet<_> = function.params.iter().copied().collect();
            ir::walk_writes(&function.body, |slot, value| {
                written.insert(slot);
                if !value.is_some_and(|v| independent_integer(v, &independent)) {
                    borrowed.insert(slot);
                }
            });
            let mut valid = true;
            ir::walk_statements(&function.body, &mut |s| {
                if let Statement::Return(value) = s {
                    valid &= match &value.kind {
                        ExprKind::Local(slot) => written.contains(slot) && !borrowed.contains(slot),
                        _ => independent_integer(value, &independent),
                    };
                }
            });
            if valid {
                independent[id] = true;
                changed = true;
            }
        }
        if independent[0] || !changed {
            return independent[0];
        }
    }
}

fn independent_integer(value: &Expr, functions: &[bool]) -> bool {
    if value.ty != Type::Int {
        return false;
    }
    match &value.kind {
        ExprKind::Int(text) => text
            .parse()
            .ok()
            .is_some_and(crate::dynamic::is_permanent_integer),
        ExprKind::Binary(Binary::Add | Binary::Subtract | Binary::Multiply, ..) => {
            // A literal-only arithmetic expression may be a constant in Python's code
            // object. A loaded operand makes this a computation in the active call.
            let mut runtime_operand = false;
            ir::walk_operand_expressions(value, &mut |operand| {
                runtime_operand |= matches!(operand.kind, ExprKind::Local(_) | ExprKind::Call(..));
            });
            runtime_operand
        }
        ExprKind::Call(id, _) => functions[*id],
        ExprKind::Conditional(_, a, b) => {
            independent_integer(a, functions) && independent_integer(b, functions)
        }
        ExprKind::Builtin(Builtin::Len, _) => true,
        _ => false,
    }
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
    fn native_frames_preserve_integer_identity_and_visible_scalar_bindings() {
        let mut graph = SourceGraph::single(concat!(
            "def alias(x: int) -> int:\n    return x\n",
            "def constant(x: int) -> int:\n    return 12345678901234567890\n",
            "def folded(x: int) -> int:\n    return 10000 + 7\n",
            "def rebind(x: int) -> int:\n    x = x + 1\n    return x + 1\n",
            "def quotient(value: int, divisor: int) -> int:\n    saved = value + 1\n    return saved // divisor\n",
            "def produced(x: int) -> int:\n    return x + 1\n",
        )).unwrap();
        graph.registered_startup = true;
        super::super::lower(&graph).unwrap();
        let closures = graph.native_functions.borrow();
        let native = &closures[&graph.entry];
        assert_eq!(native.len(), 1);
        let retained = native.values().next().unwrap();
        let source = &graph.modules[graph.entry].source;
        assert!(source[retained.site].starts_with("def produced("));
    }

    #[test]
    fn representation_failure_keeps_solved_neighbors_and_its_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("app.py");
        std::fs::write(
            &main,
            concat!(
                "import types\n",
                "def bump(x: int) -> int:\n    return x + 1\n",
                "Active = types.new_class('Active')\n",
                "print(bump(7))\n",
            ),
        )
        .unwrap();
        let graph = SourceGraph::load(&main, None, &[]).unwrap();
        let Err(report) = super::super::lower(&graph) else {
            panic!("a registered class factory is required")
        };
        assert!(
            report.contains("types.new_class requires native Python object operations"),
            "{report}"
        );
        assert_eq!(graph.native_functions.borrow()[&graph.entry].len(), 1);
        assert!(graph.registered_functions().is_empty());
    }

    #[test]
    fn solved_functions_are_captured_for_a_registered_entry_only() {
        let file = tempfile::tempdir().unwrap();
        let source = "def bump(x: int) -> int:\n    return x + 1\n\nprint(bump(7))\n";
        let mut graph = SourceGraph::single(source).unwrap();
        super::super::lower(&graph).unwrap();
        assert!(graph.native_functions.borrow().is_empty());

        let main = file.path().join("__main__.py");
        std::fs::write(&main, source).unwrap();
        let options = crate::CompileOptions {
            path_entry: true,
            module_entry: Some("__main__".to_owned()),
            ..crate::CompileOptions::default()
        };
        let registered = crate::compile_file(file.path(), None, &options).unwrap();
        assert!(
            registered
                .llvm_ir
                .contains("define internal i64 @imre_registered_")
        );

        graph.registered_startup = true;
        super::super::lower(&graph).unwrap();
        assert_eq!(graph.native_functions.borrow()[&graph.entry].len(), 1);
        graph.registered_startup = false;
        super::super::lower(&graph).unwrap();
        assert!(graph.native_functions.borrow().is_empty());
    }

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
        let (result, definitions) = caller_cache::observe_lowerings(|| super::super::lower(&graph));
        assert!(result.is_err());
        assert_eq!(
            definitions
                .iter()
                .filter(|name| *name == "reduction")
                .count(),
            1
        );
        let closures = graph.native_functions.borrow();
        let closure = closures
            .get(&graph.entry)
            .and_then(|m| m.values().next())
            .expect("checked function retained");
        assert_eq!(closure.builtins, ["int", "list", "map", "sum"]);
        assert_eq!(closure.module.functions.len(), 2);
        assert_eq!(closure.frame_lists, [(1, "values".to_string())]);
        assert!(crate::verify::module(&closure.module).is_ok());
    }
}
