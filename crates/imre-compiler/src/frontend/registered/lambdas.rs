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

pub(crate) struct RegisteredLambda {
    pub(crate) metadata: LambdaMetadata,
    pub(crate) body: Option<RegisteredFunction>,
    pub(crate) defaults: Vec<Option<Expr>>,
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
        use ast::visitor::{Visitor, walk_expr};
        struct Collector<'a> {
            source: values::RegisteredSource<'a>,
            scopes: HashMap<TextSize, FunctionScope>,
            sites: HashMap<TextSize, RegisteredLambda>,
        }
        impl<'a> Visitor<'a> for Collector<'_> {
            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                // Nested callables own their bodies before the enclosing operand binds them.
                walk_expr(self, expr);
                if let ast::Expr::Lambda(lambda) = expr {
                    let plan = RegisteredLambda::prepare(
                        lambda,
                        values::RegisteredSource {
                            lambdas: Some(&self.sites),
                            ..self.source
                        },
                        &self.scopes[&lambda.start()],
                    );
                    self.sites.insert(lambda.start(), plan);
                }
            }
        }
        let mut collector = Collector {
            source,
            scopes,
            sites: HashMap::new(),
        };
        collector.visit_body(body);
        collector.sites
    }

    fn prepare(
        lambda: &ast::ExprLambda,
        source: values::RegisteredSource<'_>,
        scope: &FunctionScope,
    ) -> Self {
        let operand = |node: &ast::Expr| values::Region::Lambda.operand(node, source);
        let defaults = values::lambda_defaults(lambda).map(operand).collect();
        let body = operand(&lambda.body)
            .map(|value| RegisteredFunction::expression(lambda.body.range(), value));
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
        let plan = RegisteredLambda::prepare(lambda, source, &FunctionScope::lambda(lambda, false));
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
        let plan = RegisteredLambda::prepare(lambda, source, &FunctionScope::lambda(lambda, false));
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
                .map(Option::is_some)
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
