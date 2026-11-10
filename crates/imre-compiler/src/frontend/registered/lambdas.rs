//! Prepare the binding expression in the enclosing scope and the return in its own frame.

use super::*;
use crate::frontend::function_scope::FunctionScope;
use imre_ast::code_flags;
use serde_json::json;

/// Lambda code metadata independent of the frame in which its definition executes.
pub(crate) struct LambdaMetadata {
    layout: serde_json::Value,
    flags: u32,
    captures_class: bool,
    pub(crate) generator: bool,
    /// Keys for the default values evaluated in order by the enclosing expression.
    pub(crate) defaults: Vec<String>,
}

impl LambdaMetadata {
    fn new(lambda: &ast::ExprLambda, scope: &FunctionScope) -> Self {
        let parameters = lambda.parameters.as_deref();
        let mut layout = json!({});
        let mut defaults = Vec::new();
        if let Some(parameters) = parameters {
            for (group, args) in [
                ("posonly", &parameters.posonlyargs),
                ("positional", &parameters.args),
                ("kwonly", &parameters.kwonlyargs),
            ] {
                if !args.is_empty() {
                    layout[group] = json!(
                        args.iter()
                            .map(|p| p.parameter.name.as_str())
                            .collect::<Vec<_>>()
                    );
                    defaults.extend(
                        args.iter()
                            .filter(|p| p.default.is_some())
                            .map(|p| p.parameter.name.to_string()),
                    );
                }
            }
            for (key, param) in [
                ("varargs", &parameters.vararg),
                ("kwargs", &parameters.kwarg),
            ] {
                if let Some(param) = param {
                    layout[key] = json!(param.name.as_str());
                }
            }
        }
        let captures_class = scope.captures_class;
        scope.write_signature(&mut layout);

        let mut flags = code_flags::function_flags(parameters, false, false);
        let generator = code_flags::yields_in_expression(&lambda.body);
        if generator {
            flags |= code_flags::GENERATOR;
            layout["generator_lambda"] = json!(true);
        }
        if captures_class {
            layout["class_cell"] = json!(true);
        }
        LambdaMetadata {
            layout,
            flags,
            generator,
            captures_class,
            defaults,
        }
    }

    pub(crate) fn parameter_names(&self) -> impl Iterator<Item = &str> {
        ["posonly", "positional", "kwonly"]
            .into_iter()
            .flat_map(|group| self.layout[group].as_array().into_iter().flatten())
            .filter_map(serde_json::Value::as_str)
            .chain(
                ["varargs", "kwargs"]
                    .into_iter()
                    .filter_map(|group| self.layout[group].as_str()),
            )
    }

    pub(crate) fn signature(
        &self,
        nested: bool,
        method: bool,
        future_flags: u32,
    ) -> serde_json::Value {
        let mut signature = self.layout.clone();
        let mut flags = self.flags | future_flags;
        if nested {
            flags |= code_flags::NESTED;
        }
        if method {
            flags |= code_flags::METHOD;
            signature["method_class_cell"] = json!(self.captures_class);
        }
        signature["code_flags"] = json!(flags);
        signature
    }
}

/// An inline default can also be embedded in a larger checked expression. Defaults
/// with their own locals must be evaluated as complete regions in the binding frame.
pub(crate) enum LambdaDefault {
    Value(Expr),
    Operand(Box<RegisteredOperand>),
    Source(DefaultParameter),
}

impl LambdaDefault {
    pub(super) fn inline(&self) -> Option<&Expr> {
        match self {
            Self::Value(value) => Some(value),
            _ => None,
        }
    }
}

/// Only a genuinely residual default needs its source operand. Record its physical
/// parameter during preparation so the adapter never scans a signature to find it.
#[derive(Clone, Copy)]
pub(crate) struct DefaultParameter {
    group: usize,
    index: usize,
}

impl DefaultParameter {
    fn groups(parameters: &ast::Parameters) -> [&[ast::ParameterWithDefault]; 3] {
        [
            &parameters.posonlyargs,
            &parameters.args,
            &parameters.kwonlyargs,
        ]
    }

    pub(crate) fn source(self, lambda: &ast::ExprLambda) -> &ast::Expr {
        let parameters = lambda
            .parameters
            .as_deref()
            .expect("lambda default has parameters");
        Self::groups(parameters)[self.group][self.index]
            .default
            .as_deref()
            .expect("prepared default parameter")
    }
}

pub(crate) struct RegisteredLambda {
    pub(crate) metadata: LambdaMetadata,
    pub(crate) body: Option<RegisteredFunction>,
    pub(crate) defaults: Vec<LambdaDefault>,
}

impl RegisteredLambda {
    /// Collect lambda sites in headers and nested activations while source is available.
    pub(crate) fn collect(
        body: &[ast::Stmt],
        source: values::RegisteredSource<'_>,
        scopes: HashMap<TextSize, FunctionScope>,
    ) -> HashMap<TextSize, Self> {
        if scopes.is_empty() {
            return HashMap::new();
        }
        use ast::visitor::{Visitor, walk_body, walk_expr, walk_stmt};
        struct Collector<'a> {
            source: values::RegisteredSource<'a>,
            scopes: HashMap<TextSize, FunctionScope>,
            sites: HashMap<TextSize, RegisteredLambda>,
            owner: RegisteredScope,
            suite_owner: Option<RegisteredScope>,
        }
        impl<'a> Visitor<'a> for Collector<'_> {
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
                let ast::Expr::Lambda(lambda) = expr else {
                    walk_expr(self, expr);
                    return;
                };
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                let parent = std::mem::replace(
                    &mut self.owner,
                    RegisteredScope::Function { is_async: false },
                );
                self.visit_expr(&lambda.body);
                self.owner = parent;

                // Bind only after nested callable sites in the defaults and body are prepared.
                let plan = RegisteredLambda::prepare(
                    lambda,
                    values::RegisteredSource {
                        lambdas: Some(&self.sites),
                        ..self.source
                    },
                    &self.scopes[&lambda.start()],
                    self.owner,
                );
                self.sites.insert(lambda.start(), plan);
            }
        }
        let mut collector = Collector {
            source,
            scopes,
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
        let operand = |node: &ast::Expr| values::Region::Lambda.operand(node, source);
        let mut defaults = Vec::new();
        if let Some(parameters) = lambda.parameters.as_deref() {
            for (group, params) in DefaultParameter::groups(parameters).into_iter().enumerate() {
                for (index, param) in params.iter().enumerate() {
                    if let Some(node) = param.default.as_deref() {
                        defaults.push(if let Some(value) = operand(node) {
                            LambdaDefault::Value(value)
                        } else if let Some(value) = RegisteredOperand::prepare(node, source, owner)
                        {
                            LambdaDefault::Operand(Box::new(value))
                        } else {
                            LambdaDefault::Source(DefaultParameter { group, index })
                        });
                    }
                }
            }
        }
        let body = Self::body(lambda, source);
        Self {
            metadata: LambdaMetadata::new(lambda, scope),
            defaults,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let scopes =
            crate::frontend::function_scope::ScopeFacts::prepare(&body, &names, |_| {}, |_| {});
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
        let (_, generators) = prepare_expression_sites(&body, source);
        let source = values::RegisteredSource {
            generators: Some(&generators),
            ..source
        };
        let mut sites = RegisteredLambda::collect(&body, source, scopes.lambdas);
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
        let [
            LambdaDefault::Value(_),
            LambdaDefault::Operand(display),
            LambdaDefault::Value(_),
        ] = checked.defaults.as_slice()
        else {
            panic!("expected the checked comprehension between two inline defaults");
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
            checked.metadata.signature(true, false, 0)["scope_reads"],
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

        // Withhold this child's body and let ordinary default preparation locate its parameter.
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
        sites.insert(residual.start(), mixed);
        let [
            LambdaDefault::Value(_),
            LambdaDefault::Operand(_),
            LambdaDefault::Value(sync_generator),
            LambdaDefault::Value(async_generator),
            LambdaDefault::Source(parameter),
            LambdaDefault::Value(_),
        ] = sites[&residual.start()].defaults.as_slice()
        else {
            panic!("only the lambda with an unprepared body should need its physical parameter");
        };
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
        assert!(std::ptr::eq(parameter.source(residual), thunk));

        let source = values::RegisteredSource {
            lambdas: Some(&sites),
            ..source
        };
        assert!(RegisteredOperand::prepare(binding(0), source, owner).is_some());
        assert!(RegisteredOperand::prepare(binding(1), source, owner).is_none());
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
        assert!(plan.metadata.generator);
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
        assert_eq!(plan.metadata.defaults, ["x", "y"]);
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
        assert_eq!(
            plan.defaults
                .iter()
                .map(|default| default.inline().is_some())
                .collect::<Vec<_>>(),
            [true, child_checked, true]
        );
        assert!(plan.body.is_some());
        assert_eq!(
            values::checked_binding(lambda, source).is_some(),
            child_checked
        );
    }
}
