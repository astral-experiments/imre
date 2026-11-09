//! Lower functions whose live Python arguments are supplied by the registered binder.

use super::*;

pub(super) mod values;

pub(super) struct Frame<'a> {
    pub(super) literals: &'a values::Literals,
    pub(super) deferred: std::cell::RefCell<Option<Failure>>,
}

impl Frame<'_> {
    pub(super) fn require_object(&self, source: &str, site: &impl Ranged) {
        self.deferred.borrow_mut().get_or_insert_with(|| {
            error(
                source,
                site,
                "operation requires its compiled object representation",
            )
            .classify(Kind::ScopedObject)
        });
    }
}

pub(crate) struct RegisteredFunction {
    pub(crate) function: ir::Function,
    pub(crate) local_names: HashMap<usize, String>,
}

/// Lower a definition whose binder supplies Python object handles in its normal function scope.
pub(super) fn lower_definition<'a>(
    mut checker: Checker<'a>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
    literals: &'a values::Literals,
) -> Result<RegisteredFunction, String> {
    checker.registered = Some(Frame {
        literals,
        deferred: Default::default(),
    });
    let source = checker.source;
    let helpers = checker.helpers;
    let first_helper = helpers.len();
    let mut local_names = HashMap::new();
    let (body, _) = lower_function(checker, definition, signature, Some(&mut local_names))?;
    if !body.calls.is_empty() || helpers.len() != first_helper {
        return Err(error(
            source,
            definition,
            "the registered frame requires a callable with a Python binding",
        )
        .classify(Kind::ScopedObject));
    }
    let function = RegisteredFunction {
        function: body.function,
        local_names,
    };
    if !crate::dynamic::supports_registered_function(&function) {
        return Err(error(
            source,
            definition,
            "function body has no checked registered-frame representation",
        )
        .classify(Kind::ScopedObject));
    }
    Ok(function)
}

pub(super) fn capture(
    graph: &SourceGraph,
    signatures: &HashMap<DefinitionId, Signature>,
    classes: &ClassInfo,
    types: &Types,
    inference: &inference::Inference,
    globals: &ModuleValues,
    retain_live_parameters: bool,
) {
    let mut registered = HashMap::<ModuleId, HashMap<TextSize, RegisteredFunction>>::new();
    let mut literals = HashMap::new();
    for (&owner, signature) in signatures {
        let id = owner.module;
        let module = &graph.modules[id];
        if !module.is_program_source() {
            continue;
        }
        // Use the original definition. The registered binder retains its names, defaults and frame.
        let Some(function) = module.parsed.syntax().body[owner.statement].as_function_def_stmt()
        else {
            continue;
        };
        let Some(signature) = inference.registered_signature(signature) else {
            if let Some(signature) = inference.checked_signature(signature) {
                let helpers = Helpers::new(inference.len());
                let checker =
                    Checker::new(graph, id, signatures, classes, types, &helpers, globals);
                retained::capture(checker, function, &signature, inference.len(), false);
            }
            continue;
        };
        if !retain_live_parameters {
            continue;
        }
        let helpers = Helpers::new(inference.len());
        let checker = Checker::new(graph, id, signatures, classes, types, &helpers, globals);
        let literals = literals
            .entry(id)
            .or_insert_with(|| values::Literals::new(module));
        if let Ok(body) = lower_definition(checker, function, &signature, literals) {
            registered
                .entry(id)
                .or_default()
                .insert(function.start(), body);
        }
    }
    if retain_live_parameters {
        graph.set_registered_functions(registered);
    }
}

pub(super) fn checkpoint(stmt: &ast::Stmt) -> Statement {
    Statement::Expr(operation(
        Builtin::RegisteredStatement(stmt.range(), imre_ast::unhighlighted_call(stmt)),
        Type::None,
        vec![],
    ))
}

fn operation(op: Builtin, ty: Type, operands: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(op, operands),
    }
}

pub(super) fn located(node: &ast::Expr, value: Expr) -> Expr {
    operation(
        Builtin::RegisteredLocation(node.range()),
        value.ty,
        vec![value],
    )
}

/// Preserve the requested module separately from the names bound in the current namespace.
pub(crate) fn registered_import(stmt: &ast::Stmt, private_names: &imre_ast::Names) -> Option<Expr> {
    let import = match stmt {
        ast::Stmt::Import(import) => ir::NamespaceImport::Modules(
            import
                .names
                .iter()
                .map(|alias| {
                    let name = alias.name.as_str();
                    let root = name.split('.').next().unwrap_or(name);
                    ir::ModuleImport {
                        name: name.to_owned(),
                        binding: alias
                            .asname
                            .as_ref()
                            .map_or(root, ast::Identifier::as_str)
                            .to_owned(),
                        bind_leaf: alias.asname.is_some()
                            && !private_names.imports_root(&alias.name),
                    }
                })
                .collect(),
        ),
        ast::Stmt::ImportFrom(import) => ir::NamespaceImport::From {
            module: import
                .module
                .as_ref()
                .map_or("", ast::Identifier::as_str)
                .to_owned(),
            level: import.level,
            names: import
                .names
                .iter()
                .map(|alias| {
                    (
                        alias.name.to_string(),
                        alias.asname.as_ref().unwrap_or(&alias.name).to_string(),
                    )
                })
                .collect(),
        },
        _ => return None,
    };
    Some(operation(
        Builtin::RegisteredImport(import),
        Type::None,
        vec![],
    ))
}

#[derive(Clone, Copy)]
pub(crate) enum RegisteredScope {
    Module,
    Class,
    Function { is_async: bool },
}

/// Lower a physical suite for an existing Python namespace. Binding indices name entries
/// in the runtime scope; they never cache a global value across executable statements.
pub(crate) fn registered_suite(
    stmts: &[ast::Stmt],
    kind: RegisteredScope,
    private_names: &imre_ast::Names,
    replacements: &HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
) -> Option<RegisteredFunction> {
    let module = matches!(kind, RegisteredScope::Module);
    let mut scope = SourceScope {
        kind,
        private_names,
        replacements,
        has_templates,
        names: HashMap::new(),
    };
    let mut body = Vec::new();
    for stmt in stmts {
        body.push(checkpoint(stmt));
        let (lowered, _) =
            statements::StatementLowering::source_statement(&mut scope, stmt, module).ok()?;
        body.extend(lowered);
    }
    let locals = vec![Type::PythonObject; scope.names.len()];
    Some(RegisteredFunction {
        function: ir::Function {
            params: vec![],
            returns: Type::PythonObject,
            locals,
            body,
        },
        local_names: scope
            .names
            .into_iter()
            .map(|(name, slot)| (slot, name))
            .collect(),
    })
}

fn plain_class_header(class: &ast::StmtClassDef) -> bool {
    class.type_params.is_none() && class.arguments.as_ref().is_none_or(|args| args.is_empty())
}

/// Keep the real class namespace for suites with ordinary bindings and no annotation setup.
pub(crate) fn registered_class_body(
    class: &ast::StmtClassDef,
    private_names: &imre_ast::Names,
    replacements: &HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
) -> Option<RegisteredFunction> {
    if !plain_class_header(class)
        || !class.body.iter().all(|stmt| {
            matches!(
                stmt,
                ast::Stmt::Assign(_) | ast::Stmt::Expr(_) | ast::Stmt::Pass(_)
            )
        })
    {
        return None;
    }
    registered_suite(
        &class.body,
        RegisteredScope::Class,
        private_names,
        replacements,
        has_templates,
    )
}

impl statements::StatementLowering for SourceScope<'_> {
    type Error = ();

    fn assignment(&mut self, assign: &ast::StmtAssign) -> StdResult<Vec<Statement>, ()> {
        let [ast::Expr::Name(target)] = assign.targets.as_slice() else {
            return Err(());
        };
        let value = self.operand(&assign.value).ok_or(())?;
        Ok(vec![self.store(target.id.as_str(), value)])
    }

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> StdResult<Option<Statement>, ()> {
        if def.type_params.is_some() {
            return Err(());
        }
        let decorators = def.decorator_list.iter().map(|d| &d.expression);
        let defaults = def
            .parameters
            .posonlyargs
            .iter()
            .chain(&def.parameters.args)
            .chain(&def.parameters.kwonlyargs)
            .filter_map(|p| p.default.as_deref());
        let header = decorators
            .chain(defaults)
            .map(|e| self.ordinary_operand(e))
            .collect::<Option<_>>()
            .ok_or(())?;
        let function = operation(
            Builtin::RegisteredDefinition(def.start()),
            Type::PythonObject,
            header,
        );
        Ok(Some(self.store(def.name.as_str(), function)))
    }

    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), ()> {
        let effect = if value.is_string_literal_expr() {
            None
        } else {
            Some(Statement::Expr(self.operand(value).ok_or(())?))
        };
        Ok((effect, true))
    }

    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Expr, ()> {
        match ret.value.as_deref() {
            Some(value) => self.operand(value).ok_or(()),
            None => Ok(Expr::none()),
        }
    }

    fn other_statement(&mut self, stmt: &ast::Stmt) -> StdResult<statements::Block, ()> {
        match stmt {
            ast::Stmt::ClassDef(class) if plain_class_header(class) => {
                let decorators = class
                    .decorator_list
                    .iter()
                    .map(|d| self.ordinary_operand(&d.expression))
                    .collect::<Option<_>>()
                    .ok_or(())?;
                let value = operation(
                    Builtin::RegisteredDefinition(class.start()),
                    Type::PythonObject,
                    decorators,
                );
                Ok((vec![self.store(class.name.as_str(), value)], true))
            }
            ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => Ok((Vec::new(), true)),
            _ => registered_import(stmt, self.private_names)
                .map(|import| (vec![Statement::Expr(import)], true))
                .ok_or(()),
        }
    }
}

struct SourceScope<'a> {
    kind: RegisteredScope,
    private_names: &'a imre_ast::Names,
    replacements: &'a HashMap<TextRange, Vec<u32>>,
    has_templates: bool,
    names: HashMap<String, usize>,
}

impl SourceScope<'_> {
    fn slot(&mut self, name: &str) -> usize {
        let next = self.names.len();
        *self.names.entry(name.to_owned()).or_insert(next)
    }

    fn store(&mut self, name: &str, value: Expr) -> Statement {
        let slot = self.slot(name);
        if matches!(self.kind, RegisteredScope::Module) {
            Statement::Expr(operation(Builtin::GlobalSet(slot), Type::None, vec![value]))
        } else {
            Statement::Assign(slot, value)
        }
    }

    fn ordinary_operand(&self, expr: &ast::Expr) -> Option<Expr> {
        values::registered_value(expr, self.replacements, self.has_templates)
    }

    fn operand(&self, expr: &ast::Expr) -> Option<Expr> {
        if let RegisteredScope::Function { is_async } = self.kind
            && let Some((kind, value)) = ir::SuspendValue::operand(expr)
            && !matches!(
                (kind, is_async),
                (ir::SuspendValue::Await, false) | (ir::SuspendValue::Delegate, true)
            )
        {
            // Admit a suspension only at this whole-expression boundary. All previous
            // statements have committed their owners to the observable frame.
            let value = match value {
                Some(value) => self.ordinary_operand(value)?,
                None => Expr::none(),
            };
            return Some(located(
                expr,
                operation(
                    Builtin::RegisteredSuspend(kind, expr.range()),
                    Type::PythonObject,
                    vec![value],
                ),
            ));
        }
        self.ordinary_operand(expr)
    }
}

impl ir::SuspendValue {
    /// Preserve the original suspension kind and the optional operand of a bare yield.
    pub(crate) fn operand(expr: &ast::Expr) -> Option<(Self, Option<&ast::Expr>)> {
        match expr {
            ast::Expr::Await(expr) => Some((Self::Await, Some(&expr.value))),
            ast::Expr::Yield(expr) => Some((Self::Yield, expr.value.as_deref())),
            ast::Expr::YieldFrom(expr) => Some((Self::Delegate, Some(&expr.value))),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dotted_import_keeps_the_root_binding_in_checked_ir() {
        let mut body = ruff_python_parser::parse_module("class C:\n    import __pkg.child\n")
            .unwrap()
            .into_syntax()
            .body;
        let names = imre_ast::lower(&mut body);
        let class = body[0].as_class_def_stmt().unwrap();
        let import = registered_import(&class.body[0], &names).unwrap();
        let ExprKind::Builtin(Builtin::RegisteredImport(ir::NamespaceImport::Modules(modules)), _) =
            import.kind
        else {
            panic!("expected an import operation");
        };
        let [module] = modules.as_slice() else {
            panic!("expected one module")
        };
        assert_eq!(module.name, "__pkg.child");
        assert_eq!(module.binding, "_C__pkg");
        assert!(!module.bind_leaf);
    }

    #[test]
    fn module_and_function_stores_share_the_registered_dictionary_operand() {
        let graph = SourceGraph::single("out = {key: callback(arg), **more}[index]\n").unwrap();
        let source = &graph.modules[graph.entry].parsed.syntax().body;
        for module in [false, true] {
            let suite = registered_suite(
                source,
                if module {
                    RegisteredScope::Module
                } else {
                    RegisteredScope::Function { is_async: false }
                },
                &Default::default(),
                &HashMap::new(),
                false,
            )
            .unwrap();
            assert_eq!(suite.local_names, HashMap::from([(0, "out".to_owned())]));
            let [Statement::Expr(_), store] = suite.function.body.as_slice() else {
                panic!("expected one checkpoint followed by one store")
            };
            let operand = match (module, store) {
                (false, Statement::Assign(0, value)) => value,
                (
                    true,
                    Statement::Expr(Expr {
                        kind: ExprKind::Builtin(Builtin::GlobalSet(0), operands),
                        ..
                    }),
                ) => &operands[0],
                _ => panic!("expected the destination's namespace store"),
            };
            let ExprKind::Builtin(Builtin::RegisteredLocation(site), operands) = &operand.kind
            else {
                panic!("expected the subscript's original range")
            };
            let ExprKind::Builtin(Builtin::RegisteredSubscript(index, _), indexed) =
                &operands[0].kind
            else {
                panic!("expected registered subscription")
            };
            assert_eq!(site, index);
            let ExprKind::Builtin(Builtin::RegisteredLocation(_), display) = &indexed[0].kind
            else {
                panic!("expected a located dictionary operand")
            };
            let ExprKind::Builtin(Builtin::RegisteredDict(entries), elements) = &display[0].kind
            else {
                panic!("expected the registered Python dictionary protocol")
            };
            assert_eq!(
                entries,
                &[ir::DictEntryKind::Pair, ir::DictEntryKind::Unpack]
            );
            assert_eq!(elements.len(), 3);
            let mut names = Vec::new();
            ir::walk_expressions(&suite.function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &expr.kind {
                    names.push(name.clone());
                }
            });
            assert_eq!(names, ["key", "callback", "arg", "more", "index"]);
        }
    }

    #[test]
    fn registered_arguments_share_the_checked_integer_body() {
        let graph = SourceGraph::single(
            "def report(x: int, sink, y: int):\n    answer = x + y\n    pass\n    sink(answer)\n",
        )
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&TextSize::default()].function;
        assert_eq!(
            body.locals,
            [Type::Int, Type::PythonObject, Type::Int, Type::Int]
        );
        let mut checkpoints = Vec::new();
        let mut checked_adds = 0;
        let mut object_calls = 0;
        ir::walk_expressions(&body.body, &mut |expr| match expr.kind {
            ExprKind::Builtin(Builtin::RegisteredStatement(range, _), _) => {
                checkpoints.push(&graph.modules[graph.entry].source[range]);
            }
            ExprKind::Binary(Binary::Add, ..) => checked_adds += 1,
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => object_calls += 1,
            _ => {}
        });
        assert_eq!(checkpoints, ["answer = x + y", "pass", "sink(answer)"]);
        assert_eq!((checked_adds, object_calls), (1, 1));
    }

    #[test]
    fn callback_result_uses_python_addition_in_the_checked_body() {
        let graph = SourceGraph::single(
            "def report(x: int, sink, y: int):\n    first = x + y\n    reply = sink(first)\n    sink(reply + y)\n",
        ).unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let checked = &functions[&graph.entry][&TextSize::default()];
        assert!(crate::dynamic::supports_registered_function(checked));
        let mut additions = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Binary(Binary::Add, ref left, ref right) = expr.kind {
                additions.push((left.ty, right.ty, expr.ty));
            }
        });
        assert_eq!(
            additions,
            [
                (Type::Int, Type::Int, Type::Int),
                (Type::PythonObject, Type::Int, Type::PythonObject),
            ]
        );
    }

    #[test]
    fn registered_conditional_keeps_the_object_test_and_mixed_arm_types() {
        let graph = SourceGraph::single(concat!(
            "def report(flag, x: int, y: int, sink):\n",
            "    sink(((x + y) if flag else sink(x)) + y)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let checked = &bodies[&graph.entry][&TextSize::default()];
        assert!(crate::dynamic::supports_registered_function(checked));
        let mut branches = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Conditional(test, yes, no) = &expr.kind {
                branches.push((test.ty, yes.ty, no.ty, expr.ty));
            }
        });
        assert_eq!(
            branches,
            [(
                Type::PythonObject,
                Type::Int,
                Type::PythonObject,
                Type::PythonObject
            )]
        );
    }

    #[test]
    fn callback_body_uses_registered_dictionary_global_and_subscript_with_checked_add() {
        let graph = SourceGraph::single(concat!(
            "KEY = 'target'\n",
            "def report(x: int, sink, y: int):\n",
            "    answer = x + y\n",
            "    callbacks = {KEY: sink}\n",
            "    callbacks[KEY](answer)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let body = bodies[&graph.entry]
            .values()
            .next()
            .expect("registered callback");
        let mut operations = Vec::new();
        ir::walk_expressions(&body.function.body, &mut |value| match &value.kind {
            ExprKind::Builtin(Builtin::RegisteredDict(entries), _) => {
                assert_eq!(entries, &[ir::DictEntryKind::Pair]);
                operations.push("dict");
            }
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "KEY" => {
                operations.push("name")
            }
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), _) => operations.push("get"),
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => operations.push("call"),
            ExprKind::Binary(Binary::Add, left, right) => {
                assert_eq!(
                    (left.ty, right.ty, value.ty),
                    (Type::Int, Type::Int, Type::Int)
                );
                operations.push("add");
            }
            _ => {}
        });
        operations.sort_unstable();
        assert_eq!(operations, ["add", "call", "dict", "get", "name", "name"]);
    }

    #[test]
    fn a_mixed_definition_keeps_its_original_list_write_diagnostic() {
        let graph = SourceGraph::single(concat!(
            "def publish(send):\n",
            "    previous: int = 0\n",
            "    previous = send(1)\n",
            "    values: list[int] = [3]\n",
            "    values.append(True)\n",
            "    print(locals()['previous'])\n",
            "def echo(value):\n    return 41\n",
            "entries = {'publish': publish}\n",
            "entries[input()]({'send': echo}[input()])\n",
        ))
        .unwrap();
        let diagnostic = super::super::lower(&graph).err().unwrap();
        assert!(diagnostic.contains(":5:19:"), "{diagnostic}");
        assert!(
            diagnostic.contains("expected int, got bool"),
            "{diagnostic}"
        );
    }

    #[test]
    fn an_annotated_noncallable_local_keeps_its_checked_call_error() {
        let graph = SourceGraph::single("def bad(x: int, arg):\n    x(arg)\n").unwrap();
        assert!(super::super::lower(&graph).is_err());
        assert!(graph.registered_functions().is_empty());
    }

    #[test]
    fn region_boundaries_preserve_specialized_and_packed_operands() {
        let many = std::iter::repeat_n("1", 65).collect::<Vec<_>>().join(",");
        for source in [
            "s.partition('-')[1]",
            "s[:2]",
            "f(*items)",
            "f(k=v)",
            "{'key': recv.method()}",
            &format!("f({many})"),
        ] {
            let graph = SourceGraph::single(source).unwrap();
            let ast::Stmt::Expr(statement) = &graph.modules[graph.entry].parsed.syntax().body[0]
            else {
                panic!("expected an expression")
            };
            assert!(
                values::registered_value(&statement.value, &HashMap::new(), false).is_none(),
                "{source}"
            );
        }
    }

    #[test]
    fn fixed_layout_folds_cannot_erase_object_operations() {
        // A runtime int subclass can supply a distinct implementation for each operation.
        for operation in ["x += y", "x = +x", "x = int(x)", "x = x.real"] {
            let program =
                format!("def report(x: int, sink, y: int):\n    {operation}\n    sink(x)\n");
            let graph = SourceGraph::single(&program).unwrap();
            assert!(super::super::lower(&graph).is_err());
            assert!(graph.registered_functions().is_empty(), "{operation}");
        }
    }

    #[test]
    fn an_imported_initializer_keeps_its_priority_over_a_registered_body_error() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app.py");
        std::fs::write(&app, "import operations\n").unwrap();
        let registered =
            "def apply(send):\n    items: list[int] = [1]\n    items.append(True)\n    send(1)\n";
        let initialize =
            "def report(value):\n    print(value)\nhandlers = {'cb': report}\ncount: int = 'bad'\n";
        for source in [
            format!("{registered}{initialize}apply(handlers[input()])\n"),
            format!("{initialize}{registered}apply(handlers[input()])\n"),
        ] {
            std::fs::write(dir.path().join("operations.py"), source).unwrap();
            let error = crate::check_file(&app, None).unwrap_err();
            assert!(error.contains("expected int, got str"), "{error}");
        }
    }

    #[test]
    fn file_entry_uses_one_native_body_for_arithmetic_and_object_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.py");
        let source = concat!(
            "def report(x: int, sink, y: int):\n",
            "    answer = x + y\n",
            "    sink(answer)\n",
            "def echo(value):\n    print(value)\n",
            "def pair(value):\n    print(value + value, value + 1)\n",
            "choices = {'a': echo, 'b': pair}\n",
            "report(int(input()), choices[input()], int(input()))\n",
        );
        std::fs::write(&path, source.replace('\n', "\r\n")).unwrap();
        crate::check_file(&path, None).unwrap();
        let compilation =
            crate::compile_file(&path, None, &crate::CompileOptions::default()).unwrap();
        let ir = &compilation.llvm_ir;
        let filename = ir
            .lines()
            .find_map(|line| {
                let (symbol, data) = line.split_once(" = private unnamed_addr constant ")?;
                data.ends_with(r#"report.py\00""#).then_some(symbol)
            })
            .expect("report.py source filename");
        let body_at = |line: usize| {
            let location = format!("ptr {filename}, i32 {line}, i64 0)");
            let mut matches = ir
                .split("\ndefine ")
                .filter_map(|definition| definition.split_once("\n}").map(|(body, _)| body))
                .filter(|body| {
                    body.lines().any(|line| {
                        line.contains("call void @imre_dyn_statement_with_exception(")
                            && line.ends_with(&location)
                    })
                });
            let body = matches.next().expect("compiled source body");
            assert!(matches.next().is_none(), "source body emitted twice");
            body
        };
        let report = body_at(2);
        let add = "call i64 @int_add(";
        let callback = "call i64 @imre_dyn_positional_call_at(";
        assert_eq!(report.matches(add).count(), 1);
        assert_eq!(report.matches(callback).count(), 1);
        assert!(report.find(add).unwrap() < report.find(callback).unwrap());
        let pair = body_at(7);
        assert!(
            !pair.contains("call i64 @imre_dyn_exact_int_word("),
            "{pair}"
        );
        assert!(!pair.contains(add));
        let plus = ir
            .lines()
            .find_map(|line| {
                let (symbol, data) = line.split_once(" = private unnamed_addr constant ")?;
                data.ends_with(r#"c"+\00""#).then_some(symbol)
            })
            .expect("Python addition protocol name");
        assert_eq!(
            pair.matches(&format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {plus},"))
                .count(),
            2
        );
    }
}
