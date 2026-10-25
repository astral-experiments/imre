//! Build the native default unittest suite from resolved TestCase class identities.
use super::*;

impl Checker<'_> {
    pub(super) fn unittest_main(
        &mut self,
        call: &ast::ExprCall,
        module: ModuleId,
    ) -> Result<Expr, String> {
        if !call.arguments.is_empty() {
            return Err(error(
                self.source,
                call,
                "unittest.main currently takes no explicit arguments",
            ));
        }
        let unit = &self.graph.modules[module];
        let class_id = |name: &str| match unit.bindings.get(name) {
            Some(Binding::Definition(id)) => self.classes.declarations.ids.get(id).copied(),
            _ => None,
        };
        let case_base = class_id("TestCase").ok_or("missing bundled TestCase")?;
        let runner_class = class_id("_MainRunner").ok_or("missing bundled unittest runner")?;
        let entry = &self.graph.modules[self.graph.entry];
        if ["load_tests", "setUpModule", "tearDownModule"]
            .iter()
            .any(|name| entry.bindings.contains_key(*name))
        {
            return Err(error(
                self.source,
                call,
                "native unittest.main does not yet support module hooks",
            ));
        }
        let mut cases = Vec::new();
        for (binding, value) in &entry.bindings {
            let Binding::Definition(def) = value else {
                continue;
            };
            let Some(&id) = self.classes.declarations.ids.get(def) else {
                continue;
            };
            let class = &self.classes.declarations.definitions[id];
            if !class.bases.contains(&case_base) {
                continue;
            }
            let mut names: Vec<_> = class
                .methods
                .iter()
                .filter(|m| m.name.starts_with("test"))
                .map(|m| m.name.to_string())
                .collect();
            if names.is_empty() && class.methods.iter().any(|m| m.name.as_str() == "runTest") {
                names.push("runTest".into());
            }
            if names.is_empty() {
                continue;
            }
            // These hooks change which callbacks run or how their results are
            // reported. Reject them until the runner implements their contracts.
            if class.methods.iter().any(|m| {
                matches!(
                    m.name.as_str(),
                    "setUpClass"
                        | "tearDownClass"
                        | "run"
                        | "__call__"
                        | "doCleanups"
                        | "doClassCleanups"
                        | "shortDescription"
                        | "defaultTestResult"
                )
            }) || !class.node.decorator_list.is_empty()
                || class
                    .methods
                    .iter()
                    .any(|m| m.name.starts_with("test") && !m.decorator_list.is_empty())
                || class.static_bindings.keys().any(|k| {
                    matches!(
                        k.as_str(),
                        "__unittest_skip__" | "__unittest_expecting_failure__" | "failureException"
                    )
                })
            {
                return Err(error(
                    self.source,
                    call,
                    "native unittest.main does not yet support custom result hooks or decorated tests",
                ));
            }
            if let Some(init) = class.initializer
                && class.method_origins[init]
                    != self.classes.declarations.definitions[case_base].definition
            {
                return Err(error(
                    self.source,
                    call,
                    "native unittest.main requires the inherited TestCase initializer",
                ));
            }
            names.sort();
            for method in names {
                cases.push((binding.clone(), method, id));
            }
        }
        cases.sort();
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
        let run = self.classes.methods[runner_class]
            .get("_run")
            .cloned()
            .ok_or("unittest runner _run is unavailable")?;
        let finish = self.classes.methods[runner_class]
            .get("_finish")
            .cloned()
            .ok_or("unittest runner _finish is unavailable")?;
        self.calls.extend([start.id, run.id, finish.id]);
        let callback = self.types.callable(Vec::new(), Type::None);
        for (_, name, id) in cases {
            let case = self.hidden(Type::Class(id));
            let mut invocation = vec![Statement::Assign(case, self.construct(call, id)?)];
            let class = &self.classes.declarations.definitions[id];
            let method_slot = class
                .fields
                .iter()
                .position(|field| field.name == "_testMethodName")
                .ok_or("missing TestCase method name")?;
            invocation.push(Statement::SetField(
                self.local(case),
                method_slot,
                unpacking::string(&name),
            ));
            let source = &self.graph.modules[class.definition.module];
            let origin = if class.definition.module == self.graph.entry {
                "__main__"
            } else {
                &source.name
            };
            let str_lit = |s: String| Expr {
                ty: Type::Str,
                kind: ExprKind::Str(s),
            };
            let method = class
                .methods
                .iter()
                .find(|m| m.name.as_str() == name)
                .unwrap();
            let description = match method.body.first() {
                Some(ast::Stmt::Expr(stmt)) => match stmt.value.as_ref() {
                    ast::Expr::StringLiteral(doc) => doc
                        .value
                        .to_str()
                        .trim()
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim(),
                    _ => "",
                },
                _ => "",
            };
            let display = format!("{name} ({origin}.{}.{name})", class.name);
            let mut args = vec![
                self.local(runner),
                str_lit(if description.is_empty() {
                    display
                } else {
                    format!("{display}\n{description}")
                }),
            ];
            for method in ["setUp", name.as_str(), "tearDown"] {
                let signature = self.classes.methods[id]
                    .get(method)
                    .cloned()
                    .ok_or_else(|| format!("unittest method {method} is unavailable"))?;
                let value =
                    self.callable_value(&signature, Some(self.local(case)), Some(callback), call)?;
                self.expect(callback, value.ty, call)?;
                self.calls.insert(signature.id);
                args.push(value);
            }
            invocation.push(Statement::Expr(self.call_value(
                &run,
                Type::None,
                ir::CallArgs::positional(args),
            )));
            body.push(Statement::Scope {
                body: invocation,
                clear: vec![case],
            });
        }
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
}
