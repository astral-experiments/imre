//! Lower functions whose live Python arguments are supplied by the registered binder.

use super::*;

pub(crate) struct RegisteredFunction {
    pub(crate) function: ir::Function,
    pub(crate) local_names: HashMap<usize, String>,
}

pub(super) fn capture(
    graph: &SourceGraph,
    signatures: &HashMap<DefinitionId, Signature>,
    classes: &ClassInfo,
    types: &Types,
    inference: &inference::Inference,
    globals: &ModuleValues,
) {
    let mut registered = HashMap::<ModuleId, HashMap<TextSize, RegisteredFunction>>::new();
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
                if let Some(native) =
                    retained::capture(checker, function, &signature, inference.len(), &helpers)
                {
                    graph
                        .native_functions
                        .borrow_mut()
                        .entry(id)
                        .or_default()
                        .insert(function.start(), native);
                }
            }
            continue;
        };
        let helpers = Helpers::new(inference.len());
        let checker = Checker::new(graph, id, signatures, classes, types, &helpers, globals);
        let mut local_names = HashMap::new();
        if let Ok((body, _)) = lower_function(checker, function, &signature, Some(&mut local_names))
            && body.calls.is_empty()
            && helpers.len() == 0
        {
            registered.entry(id).or_default().insert(
                function.start(),
                RegisteredFunction {
                    function: body.function,
                    local_names,
                },
            );
        }
    }
    graph.set_registered_functions(registered);
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

pub(super) fn call(node: &ast::ExprCall, operands: Vec<Expr>) -> Expr {
    operation(
        Builtin::RegisteredCall(node.range(), node.func.end()),
        Type::PythonObject,
        operands,
    )
}

/// Lower a physical suite for an existing Python namespace. Binding indices name entries
/// in the runtime scope; they never cache a global value across executable statements.
pub(crate) fn registered_suite(stmts: &[ast::Stmt], module: bool) -> Option<RegisteredFunction> {
    let mut scope = SourceScope {
        module,
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

/// Lower a deferred annotation in its runtime-provided namespace.
pub(crate) fn registered_operand(node: &ast::Expr) -> Option<(Expr, HashMap<usize, String>)> {
    let mut scope = SourceScope {
        module: false,
        names: HashMap::new(),
    };
    let operand = scope.operand(node)?;
    Some((operand, scope.bindings()))
}

impl statements::StatementLowering for SourceScope {
    type Error = ();

    fn assignment(&mut self, assign: &ast::StmtAssign) -> StdResult<Vec<Statement>, ()> {
        let [ast::Expr::Name(target)] = assign.targets.as_slice() else {
            return Err(());
        };
        let value = self.operand(&assign.value).ok_or(())?;
        Ok(vec![self.store(target.id.as_str(), value)])
    }

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> StdResult<Option<Statement>, ()> {
        if def.is_async || def.type_params.is_some() {
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
            .map(|e| self.operand(e))
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
            ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => Ok((Vec::new(), true)),
            _ => Err(()),
        }
    }
}

struct SourceScope {
    module: bool,
    names: HashMap<String, usize>,
}

impl SourceScope {
    fn bindings(self) -> HashMap<usize, String> {
        self.names
            .into_iter()
            .map(|(name, slot)| (slot, name))
            .collect()
    }

    fn slot(&mut self, name: &str) -> usize {
        let next = self.names.len();
        *self.names.entry(name.to_owned()).or_insert(next)
    }

    fn store(&mut self, name: &str, value: Expr) -> Statement {
        let slot = self.slot(name);
        if self.module {
            Statement::Expr(operation(Builtin::GlobalSet(slot), Type::None, vec![value]))
        } else {
            Statement::Assign(slot, value)
        }
    }

    fn operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        let value = match expr {
            ast::Expr::Name(n) => {
                let slot = self.slot(n.id.as_str());
                if self.module {
                    operation(Builtin::GlobalGet(slot), Type::PythonObject, vec![])
                } else {
                    Expr::local(slot, Type::PythonObject)
                }
            }
            ast::Expr::StringLiteral(text) => Expr {
                ty: Type::Str,
                kind: ExprKind::Str(text.value.to_str().to_owned()),
            },
            ast::Expr::Dict(display) => {
                let mut entries = Vec::with_capacity(display.items.len());
                for item in &display.items {
                    // Inserting these exact keys cannot call Python between value operands.
                    let key = item.key.as_ref()?;
                    if !matches!(
                        key,
                        ast::Expr::StringLiteral(_)
                            | ast::Expr::NumberLiteral(_)
                            | ast::Expr::BooleanLiteral(_)
                            | ast::Expr::NoneLiteral(_)
                    ) {
                        return None;
                    }
                    entries.push((self.operand(key)?, self.operand(&item.value)?));
                }
                Expr {
                    ty: Type::PythonObject,
                    kind: ExprKind::Dict {
                        key: Type::PythonObject,
                        value: Type::PythonObject,
                        entries,
                    },
                }
            }
            ast::Expr::Subscript(item) => operation(
                Builtin::DictGet,
                Type::PythonObject,
                vec![self.operand(&item.value)?, self.operand(&item.slice)?],
            ),
            ast::Expr::BinOp(bin) if bin.op == ast::Operator::Add => {
                let left = self.operand(&bin.left)?;
                let right = self.operand(&bin.right)?;
                Expr {
                    ty: Type::PythonObject,
                    kind: ExprKind::Binary(Binary::Add, Box::new(left), Box::new(right)),
                }
            }
            ast::Expr::NumberLiteral(n) => {
                let ast::Number::Int(value) = &n.value else {
                    return None;
                };
                Expr::integer(value)
            }
            ast::Expr::BooleanLiteral(b) => Expr::boolean(b.value),
            ast::Expr::NoneLiteral(_) => Expr::none(),
            ast::Expr::Call(node)
                if node.arguments.keywords.is_empty()
                    && !node.arguments.args.iter().any(ast::Expr::is_starred_expr) =>
            {
                let operands = std::iter::once(node.func.as_ref())
                    .chain(node.arguments.args.iter())
                    .map(|e| self.operand(e))
                    .collect::<Option<_>>()?;
                call(node, operands)
            }
            _ => return None,
        };
        Some(located(expr, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn file_entry_uses_one_native_body_for_arithmetic_and_object_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.py");
        let source = concat!(
            "def report(x: int, sink, y: int):\n",
            "    answer = x + y\n",
            "    sink(answer)\n",
            "def echo(value):\n    print(value)\n",
            "def pair(value):\n    print(value, value)\n",
            "choices = {'a': echo, 'b': pair}\n",
            "report(int(input()), choices[input()], int(input()))\n",
        );
        std::fs::write(&path, source.replace('\n', "\r\n")).unwrap();
        crate::check_file(&path, None).unwrap();
        let compilation =
            crate::compile_file(&path, None, &crate::CompileOptions::default()).unwrap();
        let mixed = compilation
            .llvm_ir
            .split("define ")
            .filter(|definition| {
                let body = definition.split('}').next().unwrap();
                body.contains("call i64 @int_add(")
                    && body.contains("call i64 @imre_dyn_positional_call_at(")
            })
            .count();
        assert_eq!(mixed, 1);
    }
}
