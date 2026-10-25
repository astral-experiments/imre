//! Discover fixed test classes from a real module value. Aliases in a module's
//! namespace are separate loader occurrences, even when they name one class.

use super::*;

impl Checker<'_> {
    /// Run the concrete override for its effects. Some legitimate Python
    /// overrides return None, and binding must fail before a bad body runs.
    pub(super) fn suite_run_branch(
        &mut self,
        call: &ast::ExprCall,
        attribute: &ast::ExprAttribute,
        class: usize,
        receiver: Expr,
    ) -> Result<Expr, String> {
        let index = self.classes.methods[class]["run"].method.unwrap().index;
        let record = &self.classes.declarations.definitions[class];
        let origin = record.method_origins[index];
        if !self.graph.modules[origin.module].bundled {
            self.graph
                .checked_suite_run_override(origin.module, &record.methods[index]);
        }
        let body = if let Some(message) = self.concrete_method_arity(class, index, true, 1, &[]) {
            vec![
                Statement::Expr(receiver),
                Statement::Expr(self.expr(&call.arguments.args[0])?),
                self.unittest_call_type_error(message),
            ]
        } else {
            vec![Statement::Expr(self.direct_method_call(
                call,
                attribute,
                class,
                Some(receiver),
            )?)]
        };
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
                clear: vec![],
            },
        })
    }

    pub(super) fn suite_class(&self, class: usize, name: &str) -> bool {
        let record = &self.classes.declarations.definitions[class];
        let module = &self.graph.modules[record.definition.module];
        module.bundled && module.name == "unittest" && record.name == name
    }

    fn suite_record(&self, class: usize, field: &str) -> usize {
        self.classes.declarations.definitions[class]
            .fields
            .iter()
            .position(|f| f.name == field)
            .expect("bundled suite field")
    }

    fn suite_list(
        &mut self,
        suite: usize,
        contents: Vec<Expr>,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let init = &self.classes.methods[suite]["__init__"];
        let (initializer, optional) = (init.id, init.params[1].ty);
        let list = self.types.payload(optional).unwrap();
        let item = self.types.element(list).unwrap();
        let contents = contents
            .into_iter()
            .map(|case| self.coerce(case, item, at))
            .collect::<Result<_, _>>()?;
        let list = self.coerce(
            Expr {
                ty: list,
                kind: ExprKind::List(contents),
            },
            optional,
            at,
        )?;
        self.calls.insert(initializer);
        Ok(Expr {
            ty: Type::Class(suite),
            kind: ExprKind::Construct {
                initializer,
                args: ir::CallArgs::positional(vec![list]),
            },
        })
    }

    fn discovered_case(
        &mut self,
        class: usize,
        name: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let record = &self.classes.declarations.definitions[class];
        let module = &self.graph.modules[record.definition.module];
        let owner = format!("{}.{}", module.name, record.name);
        self.class_reads.borrow_mut().insert(record.definition);
        let init = &self.classes.methods[class]["__init__"];
        let initializer = init.id;
        if init.params.len() != 2 || init.params[1].ty != Type::Str {
            return Err(error(
                self.source,
                call,
                &format!("native discovery requires a method-name constructor on {owner}"),
            ));
        }
        self.calls.insert(initializer);
        let ty = Type::Class(class);
        let slot = self.hidden(ty);
        let object = Expr {
            ty,
            kind: ExprKind::Construct {
                initializer,
                args: ir::CallArgs::positional(vec![unpacking::string(name)]),
            },
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(slot, object),
                    Statement::SetField(
                        self.local(slot),
                        self.suite_record(class, "_testClassName"),
                        unpacking::string(owner),
                    ),
                ],
                value: Box::new(self.local(slot)),
                clear: vec![slot],
            },
        })
    }

    pub(super) fn discover_module(
        &mut self,
        call: &ast::ExprCall,
        receiver: Expr,
    ) -> Result<Expr, String> {
        let (operand, rest) = match call.arguments.args.as_ref() {
            [module] => (module, call.arguments.keywords.as_ref()),
            [] if call
                .arguments
                .keywords
                .first()
                .is_some_and(|kw| kw.arg.as_ref().is_some_and(|n| n == "module")) =>
            {
                (
                    &call.arguments.keywords[0].value,
                    &call.arguments.keywords[1..],
                )
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "loadTestsFromModule requires one module value",
                ));
            }
        };
        let value = self.expr(operand)?;
        let Type::Module(module) = value.ty else {
            return Err(error(
                self.source,
                operand,
                "native loadTestsFromModule requires a statically imported module",
            ));
        };
        let source = &self.graph.modules[module];
        for hook in [
            "setUpModule",
            "tearDownModule",
            "load_tests",
            "__getattr__",
            "__dir__",
        ] {
            if source.bindings.contains_key(hook) {
                return Err(error(
                    self.source,
                    operand,
                    &format!("native discovery of {} needs its {hook} hook", source.name),
                ));
            }
        }
        let mut names = source
            .bindings
            .iter()
            .filter_map(|(name, binding)| {
                let Binding::Definition(def) = binding else {
                    return None;
                };
                let class = *self.classes.declarations.ids.get(def)?;
                (self.unittest_case(class) && !self.suite_class(class, "TestCase"))
                    .then_some((name.clone(), class))
            })
            .collect::<Vec<_>>();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let Type::Class(loader) = receiver.ty else {
            unreachable!()
        };
        let unittest = &self.graph.modules[self.classes.declarations.definitions[loader]
            .definition
            .module];
        let Binding::Definition(suite) = unittest.bindings["TestSuite"] else {
            unreachable!()
        };
        let suite = self.classes.declarations.ids[&suite];
        let mut groups = Vec::new();
        let loader_slot = self.hidden(receiver.ty);
        let check = self.classes.methods[loader]["_check_prefix"].id;
        self.calls.insert(check);
        let mut actions = vec![
            Statement::Assign(loader_slot, receiver),
            Statement::Expr(value),
        ];
        for kw in rest {
            if kw.arg.as_ref().is_none_or(|n| n != "pattern") {
                return Err(error(
                    self.source,
                    kw,
                    "unsupported loadTestsFromModule keyword",
                ));
            }
            actions.push(Statement::Expr(self.expr(&kw.value)?));
        }
        actions.push(Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(
                check,
                ir::CallArgs::positional(vec![self.local(loader_slot)]),
            ),
        }));
        for (_, class) in names {
            let record = &self.classes.declarations.definitions[class];
            let containing = &self.graph.modules[record.definition.module];
            if record
                .methods
                .iter()
                .any(|m| matches!(m.name.as_str(), "setUpClass" | "tearDownClass"))
                || ["setUpModule", "tearDownModule"]
                    .iter()
                    .any(|hook| containing.bindings.contains_key(*hook))
            {
                return Err(error(
                    self.source,
                    call,
                    &format!(
                        "native suite needs the class or module fixtures of {}.{}",
                        containing.name, record.name
                    ),
                ));
            }
            let mut methods = record
                .methods
                .iter()
                .zip(&record.method_kinds)
                .filter(|(m, kind)| {
                    m.name.starts_with("test") && **kind != crate::classes::MethodKind::Property
                })
                .map(|(m, _)| m.name.to_string())
                .collect::<Vec<_>>();
            methods.sort();
            if methods.is_empty() && record.methods.iter().any(|m| m.name.as_str() == "runTest") {
                methods.push("runTest".into());
            }
            let instances = methods
                .iter()
                .map(|n| self.discovered_case(class, n, call))
                .collect::<Result<_, _>>()?;
            groups.push(self.suite_list(suite, instances, call)?);
        }
        let value = self.suite_list(suite, groups, call)?;
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: actions,
                value: Box::new(value),
                clear: vec![loader_slot],
            },
        })
    }

    pub(super) fn invoke_test(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
        receiver: Expr,
    ) -> Result<Expr, String> {
        let methods = self.classes.methods[class]
            .iter()
            .filter(|(n, s)| {
                (n.starts_with("test") || n.as_str() == "runTest")
                    && s.method.is_some_and(|m| {
                        m.kind(&self.classes.declarations) != crate::classes::MethodKind::Property
                    })
            })
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect::<Vec<_>>();
        let case = self.hidden(receiver.ty);
        let name = self.hidden(Type::Str);
        let found = self.hidden(Type::Bool);
        let bool_value = |b| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(b),
        };
        let mut actions = vec![
            Statement::Assign(case, receiver),
            Statement::Assign(
                name,
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Field(
                        Box::new(self.local(case)),
                        self.suite_record(class, "_testMethodName"),
                    ),
                },
            ),
            Statement::Assign(found, bool_value(false)),
        ];
        for (method, sig) in methods {
            let instance = sig.method.unwrap().kind(&self.classes.declarations)
                == crate::classes::MethodKind::Instance;
            // An upstream test may run a nested suite. Only this sealed callback
            // edge has a runtime depth guard; ordinary calls to the same method
            // still participate in the source recursion check.
            let unguarded = self.calls.contains(&sig.id);
            let invocation = self.source_call(
                call,
                &sig,
                instance.then(|| self.local(case)),
                usize::from(instance),
            )?;
            if !unguarded {
                self.calls.remove(&sig.id);
            }
            let depth = |enter| Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::UnittestDepth(enter), vec![]),
            };
            let protected = Statement::Try {
                body: vec![Statement::Expr(invocation)],
                handlers: vec![],
                orelse: vec![],
                finalbody: vec![Statement::Expr(depth(false))],
            };
            let limit = Statement::Raise(Some(Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("RuntimeError").unwrap(),
                    message: Some(Box::new(unpacking::string(
                        "maximum native unittest nesting depth exceeded",
                    ))),
                },
            }));
            let matches = Expr {
                ty: Type::Bool,
                kind: ExprKind::Compare(
                    vec![self.local(name), unpacking::string(method)],
                    vec![Compare::Equal],
                ),
            };
            actions.push(Statement::If(
                matches,
                vec![
                    Statement::Assign(found, bool_value(true)),
                    Statement::If(depth(true), vec![protected], vec![limit]),
                ],
                vec![],
            ));
        }
        let missing = self.classes.methods[class]["_missing_method"].id;
        self.calls.insert(missing);
        let fail = Expr {
            ty: Type::None,
            kind: ExprKind::Call(missing, ir::CallArgs::positional(vec![self.local(case)])),
        };
        actions.push(Statement::If(
            self.local(found),
            vec![],
            vec![Statement::Expr(fail)],
        ));
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body: actions,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
                clear: vec![case, name, found],
            },
        })
    }
}
