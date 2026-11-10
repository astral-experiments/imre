//! Prepare the binding expression in the enclosing scope and the return in its own frame.

use super::*;
use crate::frontend::function_scope::{CallableMetadata, FunctionScope, ScopeFacts};
use imre_ast::code_flags;

pub(crate) struct RegisteredLambda {
    pub(crate) metadata: CallableMetadata,
    pub(crate) body: Option<RegisteredFunction>,
    pub(crate) binding: std::rc::Rc<ExpressionSelection>,
}

impl RegisteredLambda {
    /// Bind lambdas and templates in the activation that evaluates their operands.
    pub(crate) fn collect(
        body: &[ast::Stmt],
        source: values::RegisteredSource<'_>,
        scopes: &mut ScopeFacts,
        expressions: &mut ExpressionSites,
    ) -> HashMap<TextSize, Self> {
        if scopes.lambdas.is_empty() && !source.has_templates {
            return HashMap::new();
        }
        use ast::visitor::{Visitor, walk_body, walk_expr, walk_stmt};
        struct Collector<'a, 't> {
            source: values::RegisteredSource<'a>,
            expressions: &'t mut ExpressionSites,
            scopes: HashMap<TextSize, FunctionScope>,
            sites: HashMap<TextSize, RegisteredLambda>,
            owner: RegisteredScope,
            suite_owner: Option<RegisteredScope>,
        }
        impl<'a> Visitor<'a> for Collector<'_, '_> {
            fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
                // Ruff visits the entire header before calling visit_body for the suite.
                self.suite_owner = match stmt {
                    ast::Stmt::FunctionDef(def) => Some(RegisteredScope::Function {
                        is_async: def.is_async,
                    }),
                    ast::Stmt::ClassDef(_) => Some(RegisteredScope::Class),
                    _ => None,
                };
                walk_stmt(self, stmt);
            }

            fn visit_body(&mut self, stmts: &'a [ast::Stmt]) {
                let parent = self.owner;
                if let Some(owner) = self.suite_owner.take() {
                    self.owner = owner;
                }
                walk_body(self, stmts);
                self.owner = parent;
            }

            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match expr {
                    ast::Expr::Lambda(lambda) => {
                        if let Some(parameters) = &lambda.parameters {
                            self.visit_parameters(parameters);
                        }
                        let parent = std::mem::replace(&mut self.owner, RegisteredScope::Lambda);
                        self.visit_expr(&lambda.body);
                        self.owner = parent;
                    }
                    ast::Expr::Generator(generator) => {
                        let plan = imre_ast::comprehension::Plan::generator(generator);
                        self.visit_expr(plan.outer);
                        let owner = RegisteredScope::Function {
                            is_async: plan.body_suspends(),
                        };
                        let parent = std::mem::replace(&mut self.owner, owner);
                        plan.visit_body(|child| self.visit_expr(child));
                        self.owner = parent;
                    }
                    _ => walk_expr(self, expr),
                });

                let source = values::RegisteredSource {
                    lambdas: Some(&self.sites),
                    ..self.source
                };
                match expr {
                    ast::Expr::Lambda(lambda) => {
                        let plan = RegisteredLambda::prepare(
                            lambda,
                            source,
                            &self.scopes[&lambda.start()],
                            self.owner,
                        );
                        self.expressions
                            .register_lexical(expr, std::rc::Rc::clone(&plan.binding));
                        self.sites.insert(lambda.start(), plan);
                    }
                    ast::Expr::TString(_) => {
                        self.expressions.prepare_template(expr, source, self.owner)
                    }
                    _ => {}
                }
            }
        }
        let mut collector = Collector {
            source,
            expressions,
            scopes: std::mem::take(&mut scopes.lambdas),
            sites: HashMap::new(),
            owner: RegisteredScope::Module,
            suite_owner: None,
        };
        collector.visit_body(body);
        collector.sites
    }

    pub(super) fn body(
        lambda: &ast::ExprLambda,
        source: values::RegisteredSource<'_>,
    ) -> Option<RegisteredFunction> {
        let expr = &lambda.body;
        let operations = HashMap::new();
        let sites = StatementSites::default();
        let mut scope = SourceScope::new(RegisteredScope::Lambda, source, &operations, &sites);
        let value = scope.operand(expr)?;
        let mut function = RegisteredFunction::expression(expr.range(), value);
        (function.function.locals, function.local_names) = scope.bindings();
        Some(function)
    }

    fn prepare(
        lambda: &ast::ExprLambda,
        source: values::RegisteredSource<'_>,
        scope: &FunctionScope,
        owner: RegisteredScope,
    ) -> Self {
        let body = Self::body(lambda, source);
        let binding = if body.is_some() {
            let operations = HashMap::new();
            let sites = StatementSites::default();
            let mut scope = SourceScope::new(owner, source, &operations, &sites);
            scope.report_child_refusals = true;
            match scope.with_suspending_operands(|scope| scope.lambda_binding(lambda)) {
                Ok(value) if scope.locals.is_empty() => ExpressionSelection::Checked(value),
                Ok(value) => ExpressionSelection::Operand(Box::new(RegisteredOperand::from_scope(
                    value, scope,
                ))),
                Err((range, reason)) => ExpressionSelection::Refused(range, reason),
            }
        } else {
            ExpressionSelection::Refused(
                lambda.range(),
                "cannot compile this lambda body ahead of time",
            )
        };
        Self {
            metadata: CallableMetadata::lambda(
                lambda.parameters.as_deref(),
                scope,
                code_flags::yields_in_expression(&lambda.body),
            ),
            binding: std::rc::Rc::new(binding),
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mixed_defaults_own_comprehension_slots_in_the_binding_scope() {
        let text = "\
async def factory(seed, rows, mark):
    checked = lambda first=mark(seed), snap=[mark(row[0]) for row in rows], *, last=mark(seed + 1): (first, snap, last, seed)
    residual = lambda first=mark(seed), snap=[row[0] for row in rows], stream=(row for row in rows), /, *, skip, deferred=(row async for row in rows), thunk=lambda: [item for item in rows], last=mark(seed): stream
";
        let mut body = ruff_python_parser::parse_module(text)
            .unwrap()
            .into_syntax()
            .body;
        let names = imre_ast::lower(&mut body);
        let mut scopes =
            crate::frontend::function_scope::ScopeFacts::prepare(&body, &names, |_, _| {});
        let source = values::RegisteredSource {
            text,
            method_imports: &Default::default(),
            replacements: &Default::default(),
            has_templates: false,
            template_fields: None,
            generators: None,
            lambdas: None,
            private_names: Some(&names),
        };
        let generators = RegisteredGenerator::collect(&body, source);
        let source = values::RegisteredSource {
            generators: Some(&generators),
            ..source
        };
        let mut sites =
            RegisteredLambda::collect(&body, source, &mut scopes, &mut ExpressionSites::default());
        let factory = body[0].as_function_def_stmt().unwrap();
        let lambda = |index: usize| {
            factory.body[index]
                .as_assign_stmt()
                .unwrap()
                .value
                .as_lambda_expr()
                .unwrap()
        };

        let checked = &sites[&lambda(0).start()];
        let ExpressionSelection::Operand(display) = checked.binding.as_ref() else {
            panic!("the complete binding should own the comprehension's private slots");
        };
        let row = display
            .local_names
            .values()
            .find(|binding| binding.name == "row")
            .unwrap();
        assert!(
            display
                .region()
                .private_slots()
                .contains(&row.namespace.unwrap())
        );
        let child = checked.body.as_ref().unwrap();
        assert!(child.region().private_slots().is_empty() && child.local_names.is_empty());
        assert_eq!(
            checked
                .metadata
                .signature(imre_ast::code_flags::NESTED, false)["scope_reads"],
            json!(["seed"])
        );

        let residual = lambda(1);
        let parameters = residual.parameters.as_ref().unwrap();
        let thunk = parameters.kwonlyargs[2].default.as_deref().unwrap();
        let binding = |index: usize| &*factory.body[index].as_assign_stmt().unwrap().value;
        let owner = RegisteredScope::Function { is_async: true };
        let complete = values::RegisteredSource {
            lambdas: Some(&sites),
            ..source
        };
        assert!(RegisteredOperand::prepare(binding(1), complete, owner).is_some());

        let ExpressionSelection::Operand(plan) = sites[&residual.start()].binding.as_ref() else {
            panic!("expected an owning binding");
        };
        let ExprKind::Builtin(Builtin::RegisteredLambda(site, false), defaults) = &plan.value.kind
        else {
            panic!("expected the ordinary callable binding");
        };
        assert_eq!(*site, residual.start());
        assert_eq!(defaults.len(), 6);
        let (sync_generator, async_generator) = (&defaults[2], &defaults[3]);
        let stream = parameters.posonlyargs[2].default.as_deref().unwrap();
        let deferred = parameters.kwonlyargs[1].default.as_deref().unwrap();
        for (value, site, is_async) in [
            (sync_generator, stream.range(), false),
            (async_generator, deferred.range(), true),
        ] {
            let mut children = Vec::new();
            ir::walk_operand_expressions(value, &mut |node| {
                if let ExprKind::Builtin(Builtin::RegisteredGenerator(range, false), _) = &node.kind
                {
                    children.push(*range);
                }
            });
            assert_eq!(children, [site]);
            assert_eq!(generators[&site].is_async, is_async);
        }
        // A missing child is diagnosed at its original site before LLVM.
        assert!(sites.get_mut(&thunk.start()).unwrap().body.take().is_some());
        let mixed = RegisteredLambda::prepare(
            residual,
            values::RegisteredSource {
                lambdas: Some(&sites),
                ..source
            },
            &FunctionScope::lambda(residual, false),
            owner,
        );
        assert!(
            matches!(mixed.binding.as_ref(), ExpressionSelection::Refused(range, "cannot compile this lambda body ahead of time") if *range == thunk.range())
        );
        sites.insert(residual.start(), mixed);

        let source = values::RegisteredSource {
            lambdas: Some(&sites),
            ..source
        };
        assert!(RegisteredOperand::prepare(binding(0), source, owner).is_some());
        assert!(RegisteredOperand::prepare(binding(1), source, owner).is_none());
    }

    #[test]
    fn lambda_binding_owns_await_and_comprehension_in_one_default() {
        let text = "async def factory(rows):\n    return lambda n=consume(await one(), [x for x in rows]): n\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let literals = values::ModuleSource::new(module);
        let factory = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let lambda = factory.body[0]
            .as_return_stmt()
            .unwrap()
            .value
            .as_deref()
            .unwrap()
            .as_lambda_expr()
            .unwrap();
        let plan = RegisteredLambda::prepare(
            lambda,
            literals.context(&module.source),
            &FunctionScope::lambda(lambda, false),
            RegisteredScope::Function { is_async: true },
        );

        let ExpressionSelection::Operand(binding) = plan.binding.as_ref() else {
            panic!("the awaited default should be part of the prepared binding");
        };
        let iteration = binding
            .local_names
            .values()
            .find(|local| local.name == "x")
            .unwrap();
        assert!(
            binding
                .region()
                .private_slots()
                .contains(&iteration.namespace.unwrap())
        );
        let mut bindings = Vec::new();
        let mut awaits = Vec::new();
        ir::walk_operand_expressions(&binding.value, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredLambda(site, false), args) => {
                bindings.push((*site, args.len()));
            }
            ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Await, range), _) => {
                awaits.push(&text[*range]);
            }
            _ => {}
        });
        assert_eq!(bindings, [(lambda.start(), 1)]);
        assert_eq!(awaits, ["await one()"]);
        let child = plan.body.as_ref().unwrap();
        assert!(child.local_names.is_empty() && child.region().private_slots().is_empty());
    }

    #[test]
    fn generator_lambda_owns_its_unpack_and_named_expression_slots() {
        let text = "lambda: ((yield [(last := value) for _, value in row]) or last)";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let literals = values::ModuleSource::new(module);
        let source = literals.context(&module.source);
        let expr = &module.parsed.syntax().body[0].as_expr_stmt().unwrap().value;
        let lambda = expr.as_lambda_expr().unwrap();
        assert!(values::has_checked_lambda_body(lambda, source));
        let plan = RegisteredLambda::prepare(
            lambda,
            source,
            &FunctionScope::lambda(lambda, false),
            RegisteredScope::Module,
        );
        assert_eq!(plan.metadata.suspension_flags(), code_flags::GENERATOR);
        let checked = plan.body.unwrap();
        assert!(
            checked
                .local_names
                .values()
                .any(|b| b.name == "last" && b.namespace.is_none())
        );
        let mut pulls = 0;
        let mut yields = 0;
        ir::walk_nodes(&checked.function.body, &mut |node, _| match node {
            ir::Node::Statement(Statement::IterNext { .. }) => pulls += 1,
            ir::Node::Expression(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _), _),
                ..
            }) => yields += 1,
            _ => {}
        });
        assert_eq!((pulls, yields), (1, 1));
    }

    #[test]
    fn binding_and_invocation_have_separate_checked_operands() {
        let text = "lambda x=early(), /, *, y=later(): reader(*x, closed=closed + y)";
        let expression = ruff_python_parser::parse_expression(text)
            .unwrap()
            .into_syntax()
            .body;
        let ast::Expr::Lambda(lambda) = expression.as_ref() else {
            unreachable!()
        };
        let source = values::RegisteredSource {
            text,
            method_imports: &Default::default(),
            replacements: &Default::default(),
            has_templates: false,
            template_fields: None,
            generators: None,
            lambdas: None,
            private_names: None,
        };
        let plan = RegisteredLambda::prepare(
            lambda,
            source,
            &FunctionScope::lambda(lambda, false),
            RegisteredScope::Module,
        );
        let sites = HashMap::from([(lambda.start(), plan)]);
        let source = values::RegisteredSource {
            lambdas: Some(&sites),
            ..source
        };
        let plan = &sites[&lambda.start()];
        assert_eq!(plan.metadata.defaults.as_ref(), ["x", "y"]);
        let binding = values::checked_binding(lambda, source).unwrap();
        let ExprKind::Builtin(Builtin::RegisteredLambda(site, false), defaults) = &binding.kind
        else {
            panic!("expected a checked lambda binding")
        };
        assert_eq!(*site, lambda.start());
        let names = |expr: &Expr| {
            let mut names = Vec::new();
            ir::walk_operand_expressions(expr, &mut |node| {
                if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &node.kind {
                    names.push(name.clone());
                }
            });
            names
        };
        assert_eq!(
            defaults.iter().flat_map(names).collect::<Vec<_>>(),
            ["early", "later"]
        );
        let [Statement::Expr(checkpoint), Statement::Return(value)] =
            plan.body.as_ref().unwrap().function.body.as_slice()
        else {
            panic!("expected a checked return")
        };
        assert!(
            matches!(checkpoint.kind, ExprKind::Builtin(Builtin::RegisteredStatement(range, None), _) if range == lambda.body.range())
        );
        assert_eq!(names(value), ["reader", "x", "closed", "y"]);
        let mut calls = Vec::new();
        ir::walk_operand_expressions(value, &mut |node| {
            if let ExprKind::Builtin(Builtin::RegisteredCall(site), _) = &node.kind {
                calls.push(&site.arguments);
            }
        });
        assert_eq!(
            calls,
            [&ir::RegisteredArguments::Packed(ir::PackedArguments {
                starred: vec![true],
                keywords: vec![Some("closed".into())],
            })]
        );
    }

    #[test]
    fn residual_operands_keep_their_neighbors_checked() {
        let text = "lambda a=first(), b=(lambda: x == y), c=last(): consume(*a)";
        let expression = ruff_python_parser::parse_expression(text)
            .unwrap()
            .into_syntax()
            .body;
        let lambda = expression.as_lambda_expr().unwrap();
        let source = values::RegisteredSource {
            text,
            method_imports: &Default::default(),
            replacements: &Default::default(),
            has_templates: false,
            template_fields: None,
            generators: None,
            lambdas: None,
            private_names: None,
        };
        let plan = RegisteredLambda::prepare(
            lambda,
            source,
            &FunctionScope::lambda(lambda, false),
            RegisteredScope::Module,
        );
        let nested = values::lambda_defaults(lambda)
            .nth(1)
            .unwrap()
            .as_lambda_expr()
            .unwrap();
        let child_checked = values::Region::Lambda
            .operand(&nested.body, source)
            .is_some();
        match plan.binding.as_ref() {
            ExpressionSelection::Checked(binding) => {
                let ExprKind::Builtin(Builtin::RegisteredLambda(_, false), defaults) =
                    &binding.kind
                else {
                    panic!("expected the ordinary callable binding");
                };
                assert!(child_checked);
                assert_eq!(defaults.len(), 3);
                let mut children = Vec::new();
                ir::walk_operand_expressions(&defaults[1], &mut |expr| {
                    if let ExprKind::Builtin(Builtin::RegisteredLambda(site, false), _) = &expr.kind
                    {
                        children.push(*site);
                    }
                });
                assert_eq!(children, [nested.start()]);
            }
            ExpressionSelection::Refused(range, _) => {
                assert!(!child_checked);
                assert_eq!(*range, nested.range());
            }
            _ => panic!("expected a checked binding or a located refusal"),
        }
        assert!(plan.body.is_some());
        assert_eq!(
            values::checked_binding(lambda, source).is_some(),
            child_checked
        );
    }
}
