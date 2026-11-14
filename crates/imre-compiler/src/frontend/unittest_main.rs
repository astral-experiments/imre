//! Build the native default unittest suite from resolved TestCase class identities.
use super::*;
use ruff_python_ast::CmpOp;

impl Checker<'_> {
    pub(super) fn unittest_main(
        &mut self,
        call: &ast::ExprCall,
        module: ModuleId,
    ) -> Result<Expr, String> {
        if !call.arguments.is_empty() {
            return Err(self.object_runtime_export("configured unittest.main", call));
        }
        let unit = &self.graph.modules[module];
        let class_id = |name: &str| match unit.bindings.get(name) {
            Some(Binding::Definition(id)) => self.classes.declarations.ids.get(id).copied(),
            _ => None,
        };
        let runner_class = class_id("_MainRunner").ok_or("missing bundled unittest runner")?;
        let entry = &self.graph.modules[self.graph.entry];
        if ["setUpModule", "tearDownModule"]
            .iter()
            .any(|name| entry.bindings.contains_key(*name))
        {
            return Err(self.object_runtime_export("unittest module fixtures", call));
        }
        let mut clear = Vec::new();
        let runner = self.hidden(Type::Class(runner_class));
        clear.push(runner);
        let mut body = vec![Statement::Assign(
            runner,
            self.construct(call, runner_class)?,
        )];
        let start = self.classes.methods[runner_class]
            .get("_start")
            .cloned()
            .ok_or("unittest runner _start is unavailable")?;
        let started = self.call_value(
            &start,
            Type::None,
            ir::CallArgs::positional(vec![self.local(runner)]),
        );
        body.push(Statement::Expr(started));
        let finish = self.classes.methods[runner_class]
            .get("_finish")
            .cloned()
            .ok_or("unittest runner _finish is unavailable")?;
        self.calls.extend([start.id, finish.id]);
        if let Some(Binding::Definition(id)) = entry.bindings.get("load_tests")
            && let ast::Stmt::FunctionDef(f) =
                &self.graph.modules[id.module].parsed.syntax().body[id.statement]
            && f.parameters.is_empty()
        {
            return Err(error(
                self.source,
                call,
                "unittest module hooks must accept loader, tests, and pattern",
            ));
        }
        let Some(Binding::Value(default)) = unit.bindings.get("defaultTestLoader") else {
            return Err(error(self.source, call, "missing unittest default loader"));
        };
        let slot = self
            .globals
            .slot(*default, 0)
            .ok_or("unittest default loader is unavailable")?;
        let suite = self.unittest_suite_from_module(
            call,
            self.graph.entry,
            self.globals.slot_value(slot),
            None,
        )?;
        let run = self.classes.methods[runner_class]
            .get("_runSuite")
            .cloned()
            .ok_or("unittest suite runner is unavailable")?;
        self.calls.insert(run.id);
        body.push(Statement::Expr(self.call_value(
            &run,
            Type::None,
            ir::CallArgs::positional(vec![self.local(runner), suite]),
        )));
        let finished = self.call_value(
            &finish,
            Type::None,
            ir::CallArgs::positional(vec![self.local(runner)]),
        );
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(finished),
            },
        })
    }

    /// Class fixtures share state across nested suites and use the live class
    /// object, including when a case is constructed for direct registration.
    pub(super) fn check_unittest_class_fixtures(
        &self,
        call: &ast::ExprCall,
        id: usize,
    ) -> Result<(), String> {
        if self.unittest_case(id) {
            let class = &self.classes.declarations.definitions[id];
            let bindings = &self.graph.modules[class.definition.module].bindings;
            if bindings.contains_key("setUpModule") || bindings.contains_key("tearDownModule") {
                return Err(self.object_runtime_export("unittest module fixtures", call));
            }
            let attributes = self.fixed_class_attributes(id);
            if attributes.contains_key("setUpClass") || attributes.contains_key("tearDownClass") {
                return Err(self.object_runtime_export("unittest class fixtures", call));
            }
        }
        Ok(())
    }

    fn validate_unittest_case(&self, call: &ast::ExprCall, id: usize) -> Result<(), String> {
        self.check_unittest_class_fixtures(call, id)?;
        let class = &self.classes.declarations.definitions[id];
        let base = class
            .bases
            .iter()
            .rev()
            .find(|&&b| self.unittest_case(b))
            .map(|&b| self.classes.declarations.definitions[b].definition)
            .ok_or("missing TestCase base")?;
        // Fixture lookup uses class declarations even when an inherited
        // classmethod has no explicit call site in the reachable method table.
        let mut seen = HashSet::new();
        for &owner in std::iter::once(&id).chain(&class.bases) {
            let ancestor = &self.classes.declarations.definitions[owner];
            if ancestor.definition == base {
                break;
            }
            let unsupported = ancestor.node.body.iter().any(|stmt| {
                let (name, decorated) = match stmt {
                    ast::Stmt::FunctionDef(f) => (f.name.as_str(), !f.decorator_list.is_empty()),
                    ast::Stmt::Assign(a) if a.targets.len() == 1 => {
                        let Some(name) = a.targets[0].as_name_expr() else {
                            return false;
                        };
                        (name.id.as_str(), false)
                    }
                    ast::Stmt::AnnAssign(a) if a.value.is_some() => {
                        let Some(name) = a.target.as_name_expr() else {
                            return false;
                        };
                        (name.id.as_str(), false)
                    }
                    _ => return false,
                };
                seen.insert(name)
                    && (matches!(
                        name,
                        "setUpClass"
                            | "tearDownClass"
                            | "run"
                            | "__call__"
                            | "doCleanups"
                            | "doClassCleanups"
                            | "shortDescription"
                            | "defaultTestResult"
                            | "__unittest_skip__"
                            | "__unittest_expecting_failure__"
                    ) || (decorated && (name.starts_with("test") || name == "runTest")))
            });
            if unsupported
                || ancestor.static_bindings.contains_key("failureException")
                || (!ancestor.node.decorator_list.is_empty() && ancestor.skip_decorators.is_empty())
            {
                return Err(error(
                    self.source,
                    call,
                    "native unittest does not yet support custom result hooks or decorated tests",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn unittest_default_construct(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
    ) -> Result<Expr, String> {
        let mut constructor = call.clone();
        constructor.arguments.args = Default::default();
        constructor.arguments.keywords = Default::default();
        self.construct(&constructor, class)
    }
    fn unittest_display(
        &self,
        call: &ast::ExprCall,
        id: usize,
        name: &str,
    ) -> Result<Expr, String> {
        let class = &self.classes.declarations.definitions[id];
        let module = self.runtime_module_name(class.definition.module);
        let mut display = format!("{name} ({module}.{}.{name})", class.name);
        let method = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    &format!("cannot register native TestCase method {name}"),
                )
            })?;
        if let Some(ast::Stmt::Expr(stmt)) = method.body.first()
            && let ast::Expr::StringLiteral(doc) = stmt.value.as_ref()
            && let Some(first) = doc.value.to_str().trim().lines().next()
        {
            display.push('\n');
            display.push_str(first.trim());
        }
        Ok(unpacking::string(display))
    }

    fn unittest_invoke(
        &mut self,
        call: &ast::ExprCall,
        runner: &Expr,
        case: Expr,
        id: usize,
        name: &str,
        description: Expr,
    ) -> Result<Expr, String> {
        let Type::Class(runner_class) = runner.ty else {
            unreachable!()
        };
        // Native result records retain the case and diagnostic text; a custom
        // addError/addFailure hook also observes the original exc_info tuple.
        let records = &self.classes.declarations.definitions;
        let custom_errors = std::iter::once(&runner_class).chain(&records[runner_class].bases).any(|&owner| {
            !self.graph.modules[records[owner].definition.module].is_bundled("unittest")
                && records[owner].node.body.iter().any(|stmt| matches!(stmt,
                    ast::Stmt::FunctionDef(method) if matches!(method.name.as_str(), "addFailure" | "addError")))
        });
        if custom_errors {
            return self.record_slot(
                runner_class,
                runner.clone(),
                "_unsupportedCase",
                &[unpacking::string(
                    "custom TestResult error hooks require the original exception information",
                )],
                call,
            );
        }
        let run = self.classes.methods[runner_class]
            .get("_run")
            .cloned()
            .ok_or("unittest runner _run is unavailable")?;
        let mut args = vec![
            runner.clone(),
            self.coerce(case.clone(), run.params[1].ty, call)?,
            description.clone(),
        ];
        let callback = self.types.callable(Vec::new(), Type::None);
        for method in ["setUp", name, "tearDown"] {
            let signature = self.classes.methods[id]
                .get(method)
                .cloned()
                .ok_or_else(|| format!("unittest method {method} is unavailable"))?;
            let value =
                self.callable_value(&signature, Some(case.clone()), Some(callback), call)?;
            self.expect(callback, value.ty, call)?;
            self.calls.insert(signature.id);
            args.push(value);
        }
        self.calls.insert(run.id);
        let mut invoke = self.call_value(&run, Type::None, ir::CallArgs::positional(args));
        // Applying a false decorator leaves inherited skip metadata in effect.
        // Among active decorators in the MRO, the nearest class supplies the reason.
        let class = &self.classes.declarations.definitions[id];
        for &ancestor in std::iter::once(&id).chain(&class.bases).rev() {
            let ancestor = &self.classes.declarations.definitions[ancestor];
            for index in (0..ancestor.skip_decorators.len()).rev() {
                let (flag, reason) = self
                    .globals
                    .unittest_skips
                    .borrow()
                    .get(&(ancestor.definition, index))
                    .copied()
                    .ok_or("uninitialized unittest skip decorator")?;
                let skip = self.classes.methods[runner_class]
                    .get("_skip")
                    .cloned()
                    .ok_or("missing native unittest skip handler")?;
                let args = vec![
                    runner.clone(),
                    self.coerce(case.clone(), skip.params[1].ty, call)?,
                    description.clone(),
                    self.globals.slot_value(reason),
                ];
                self.calls.insert(skip.id);
                let skipped = self.call_value(&skip, Type::None, ir::CallArgs::positional(args));
                invoke = Expr {
                    ty: Type::None,
                    kind: ExprKind::Conditional(
                        Box::new(self.globals.slot_value(flag)),
                        Box::new(skipped),
                        Box::new(invoke),
                    ),
                };
            }
        }
        Ok(invoke)
    }
    /// Every TestCase binding in dir(module) order contributes a suite, including aliases.
    fn unittest_module_classes(&self, module: ModuleId) -> Vec<usize> {
        let mut bindings = Vec::new();
        for (name, binding) in &self.graph.modules[module].bindings {
            if let Binding::Definition(def) = binding
                && let Some(&id) = self.classes.declarations.ids.get(def)
                && self.classes.declarations.definitions[id]
                    .bases
                    .iter()
                    .any(|&base| self.unittest_case(base))
            {
                bindings.push((name, id));
            }
        }
        bindings.sort();
        bindings.into_iter().map(|(_, id)| id).collect()
    }

    fn unittest_method_names(&self, id: usize) -> Vec<String> {
        self.fixed_class_attributes(id)
            .into_iter()
            .filter_map(|(name, callable)| {
                (callable == Some(true) && (name.starts_with("test") || name == "runTest"))
                    .then_some(name)
            })
            .collect()
    }

    pub(super) fn unittest_module_cases(
        &mut self,
        call: &ast::ExprCall,
        target: ModuleId,
        suite: usize,
        loader: usize,
    ) -> Result<Vec<Statement>, String> {
        let Type::Class(suite_class) = self.locals[suite] else {
            unreachable!()
        };
        let mut body = Vec::new();
        for id in self.unittest_module_classes(target) {
            let group = self.hidden(Type::Class(suite_class));
            let mut register = vec![Statement::Assign(
                group,
                self.unittest_default_construct(call, suite_class)?,
            )];
            register.extend(self.unittest_add_class_cases(call, id, group, loader)?);
            let add = self.record_slot(
                suite_class,
                self.local(suite),
                "addTest",
                &[self.local(group)],
                call,
            )?;
            register.push(Statement::Expr(add));
            body.push(Statement::Scope {
                body: register,
                clear: vec![group],
            });
        }
        Ok(body)
    }

    pub(super) fn unittest_add_class_cases(
        &mut self,
        call: &ast::ExprCall,
        id: usize,
        group: usize,
        loader: usize,
    ) -> Result<Vec<Statement>, String> {
        let Type::Class(suite_class) = self.locals[group] else {
            unreachable!()
        };
        if self.unittest_case(id)
            && !self.classes.declarations.definitions[id]
                .bases
                .iter()
                .any(|&parent| self.unittest_case(parent))
        {
            return Ok(Vec::new());
        }
        let mut register = Vec::new();
        let selected = self.unittest_fixed_names(call, &self.local(loader), id)?;
        let names = self.hidden(selected.ty);
        register.push(Statement::Assign(names, selected));
        // CPython tries runTest even when the attribute it finds is not a
        // callable method. Reject an unsupported descriptor rather than
        // reporting an empty suite when that fallback would be selected.
        if self
            .fixed_class_attributes(id)
            .get("runTest")
            .is_some_and(|value| *value != Some(true))
        {
            return Err(error(
                self.source,
                call,
                "native TestCase needs a callable runTest method",
            ));
        }
        if !self.classes.declarations.definitions[id]
            .abstract_methods
            .is_empty()
        {
            return Err(error(
                self.source,
                call,
                "native unittest cannot instantiate an abstract test case",
            ));
        }
        let methods = self.unittest_method_names(id);
        if !methods.is_empty() {
            self.validate_unittest_case(call, id)?;
        }
        let mut constructor = call.clone();
        constructor.arguments.keywords = Default::default();
        for name in methods {
            let contains = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(
                    Builtin::StringListContains,
                    vec![unpacking::string(&name), self.local(names)],
                ),
            };
            let include = if name == "runTest" {
                let nonempty = Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::Bool, vec![self.local(names)]),
                };
                let empty = Expr::not(nonempty);
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Boolean(false, vec![contains, empty]),
                }
            } else {
                contains
            };
            // Bind the selected name through the case's real initializer, including
            // inherited parameter defaults and argument packs.
            constructor.arguments.args = [ast::StringLiteral {
                range: call.range(),
                node_index: ast::AtomicNodeIndex::NONE,
                value: name.as_str().into(),
                flags: ast::StringLiteralFlags::empty(),
            }
            .into()]
            .into();
            let case = self.construct(&constructor, id)?;
            let add = self.record_slot(
                suite_class,
                self.local(group),
                "_addCase",
                &[case, self.unittest_display(call, id, &name)?],
                call,
            )?;
            register.push(Statement::If(include, vec![Statement::Expr(add)], vec![]));
        }
        Ok(vec![Statement::Scope {
            body: register,
            clear: vec![names],
        }])
    }

    /// Suite records hold ordinary case references. Resolve callbacks only when this
    /// occurrence runs, so discovery and repeated registration leave fixtures untouched.
    pub(super) fn unittest_run_registered(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
        receiver: &Expr,
    ) -> Result<Option<Expr>, String> {
        let result = &self.classes.declarations.definitions[class];
        let Some(origin) = result
            .methods
            .iter()
            .position(|m| m.name.as_str() == "_runRegistered")
            .map(|slot| result.method_origins[slot])
        else {
            return Ok(None);
        };
        let module = &self.graph.modules[origin.module];
        if !module.is_bundled("unittest")
            || module.bindings.get("TestResult") != Some(&Binding::Definition(origin))
        {
            return Ok(None);
        }
        let ([case, description], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            return Err(error(
                self.source,
                call,
                "_runRegistered expects a TestCase and its description",
            ));
        };
        let class_id = class;
        let case = self.expr(case)?;
        let Type::Class(base) = case.ty else {
            return Err(error(self.source, call, "expected TestCase"));
        };
        let description = self.expr(description)?;
        let runner_slot = self.hidden(receiver.ty);
        let case_slot = self.hidden(case.ty);
        let description_slot = self.hidden(description.ty);
        let result =
            self.dispatch_record(base, self.local(case_slot), call, |checker, id, case| {
                let class = &checker.classes.declarations.definitions[id];
                let index = class
                    .fields
                    .iter()
                    .position(|f| f.name == "_testMethodName")
                    .ok_or("missing TestCase method name")?;
                let method_name = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Field(Box::new(case.clone()), index),
                };
                let methods = checker.unittest_method_names(id);
                // Explicit TestSuite.addTest(case) can supply a case outside the
                // loader. Do not silently execute one with unsupported hooks.
                if !methods.is_empty()
                    && !checker.graph.modules[class.definition.module].is_bundled("unittest")
                    && let Err(failure) = checker.validate_unittest_case(call, id)
                {
                    let Failure::Diagnostic(reason) = failure else {
                        return Err(failure);
                    };
                    return checker.record_slot(
                        class_id,
                        checker.local(runner_slot),
                        "_unsupportedCase",
                        &[unpacking::string(reason.to_string())],
                        call,
                    );
                }
                let mut run = checker.record_slot(
                    class_id,
                    checker.local(runner_slot),
                    "_missingMethod",
                    std::slice::from_ref(&method_name),
                    call,
                )?;
                for method in methods {
                    let condition = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(
                            vec![method_name.clone(), unpacking::string(&method)],
                            vec![CmpOp::Eq],
                        ),
                    };
                    let explicit = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(
                            vec![checker.local(description_slot), unpacking::string("")],
                            vec![CmpOp::Eq],
                        ),
                    };
                    let display = Expr {
                        ty: Type::Str,
                        kind: ExprKind::Conditional(
                            Box::new(explicit),
                            Box::new(checker.unittest_display(call, id, &method)?),
                            Box::new(checker.local(description_slot)),
                        ),
                    };
                    let invoke = checker.unittest_invoke(
                        call,
                        &checker.local(runner_slot),
                        case.clone(),
                        id,
                        &method,
                        display,
                    )?;
                    run = Expr {
                        ty: Type::None,
                        kind: ExprKind::Conditional(
                            Box::new(condition),
                            Box::new(invoke),
                            Box::new(run),
                        ),
                    };
                }
                Ok(run)
            })?;
        Ok(Some(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(runner_slot, receiver.clone()),
                    Statement::Assign(case_slot, case),
                    Statement::Assign(description_slot, description),
                ],
                clear: vec![runner_slot, case_slot, description_slot],
                value: Box::new(result),
            },
        }))
    }
}
