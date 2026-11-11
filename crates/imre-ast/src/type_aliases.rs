//! Expression restrictions in annotation scopes used by type aliases and generic classes.
use std::collections::HashSet;

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

use crate::comprehension::{Output, Plan, Step};

/// The independently evaluated values on a declared type parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeParameterField {
    Bound,
    Constraints,
    Default,
}

impl TypeParameterField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bound => "bound",
            Self::Constraints => "constraints",
            Self::Default => "default",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Bound, Self::Constraints, Self::Default]
            .into_iter()
            .find(|field| field.as_str() == name)
    }

    pub fn expressions(param: &ast::TypeParam) -> impl Iterator<Item = (Self, &Expr)> {
        let bound = match param {
            ast::TypeParam::TypeVar(var) => var.bound.as_deref().map(|bound| {
                let field = if bound.is_tuple_expr() {
                    Self::Constraints
                } else {
                    Self::Bound
                };
                (field, bound)
            }),
            _ => None,
        };
        bound
            .into_iter()
            .chain(param.default().map(|value| (Self::Default, value)))
    }
}

type Error<'a> = (&'a Expr, &'static str);

/// Validate annotation scopes before execution, including nested and untaken declarations.
pub fn annotation_scope_error(body: &[Stmt]) -> Option<(TextRange, String)> {
    #[derive(Default)]
    struct Module(Option<(TextRange, String)>);
    impl<'a> Visitor<'a> for Module {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.0.is_some() {
                return;
            }
            let params = match stmt {
                Stmt::TypeAlias(alias) => alias.type_params.as_deref(),
                Stmt::ClassDef(class) => class.type_params.as_deref(),
                _ => None,
            };
            if let Some(params) = params {
                let mut names = HashSet::new();
                for param in params.iter() {
                    let name = param.name();
                    if !names.insert(name.as_str()) {
                        self.0 =
                            Some((param.range(), format!("duplicate type parameter '{name}'")));
                        return;
                    }
                }
            }
            let error = match stmt {
                Stmt::TypeAlias(alias) => {
                    let mut rhs = Rhs::default();
                    rhs.visit_expr(&alias.value);
                    rhs.error
                }
                Stmt::ClassDef(class) if params.is_some() => class
                    .arguments
                    .as_deref()
                    .and_then(generic_class_arguments_error),
                _ => None,
            };
            if let Some((expr, reason)) = error {
                self.0 = Some((expr.range(), reason.to_owned()));
            } else {
                visitor::walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, _: &'a Expr) {}
    }

    let mut module = Module::default();
    module.visit_body(body);
    module.0
}

/// Validate the base and keyword expressions executed in a generic class's parameter scope.
pub fn generic_class_arguments_error(arguments: &ast::Arguments) -> Option<Error<'_>> {
    let mut rhs = Rhs {
        generic_class: true,
        ..Default::default()
    };
    rhs.visit_arguments(arguments);
    rhs.error
}

#[derive(Default)]
struct Rhs<'a> {
    generic_class: bool,
    error: Option<Error<'a>>,
    comprehension: Option<&'a Expr>,
    in_iterable: bool,
}

impl<'a> Visitor<'a> for Rhs<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.error.is_some() {
            return;
        }
        let error = match expr {
            Expr::Named(_) if self.comprehension.is_none() => (
                expr,
                if self.generic_class {
                    "named expression cannot be used within the definition of a generic"
                } else {
                    "named expression cannot be used within a type alias"
                },
            ),
            Expr::Named(_) if self.in_iterable => (
                expr,
                "assignment expression cannot be used in a comprehension iterable expression",
            ),
            Expr::Named(n) => (
                &*n.target,
                if self.generic_class {
                    "assignment expression within a comprehension cannot be used within the definition of a generic"
                } else {
                    "assignment expression within a comprehension cannot be used in a type alias"
                },
            ),
            Expr::Yield(_) | Expr::YieldFrom(_) => (
                expr,
                match self.comprehension {
                    Some(Expr::ListComp(_)) => "'yield' inside list comprehension",
                    Some(Expr::SetComp(_)) => "'yield' inside set comprehension",
                    Some(Expr::DictComp(_)) => "'yield' inside dict comprehension",
                    Some(_) => "'yield' inside generator expression",
                    None if self.generic_class => {
                        "yield expression cannot be used within the definition of a generic"
                    }
                    None => "yield expression cannot be used within a type alias",
                },
            ),
            Expr::Await(_) if self.comprehension.is_none() => (
                expr,
                if self.generic_class {
                    "await expression cannot be used within the definition of a generic"
                } else {
                    "await expression cannot be used within a type alias"
                },
            ),
            Expr::Lambda(lambda) => {
                // Defaults run in the enclosing scope. The lambda body has its own scope.
                if let Some(params) = &lambda.parameters {
                    self.visit_parameters(params);
                }
                return;
            }
            expr if let Some(plan) = Plan::from_expr(expr) => {
                return self.comprehension(expr, &plan);
            }
            // A missing key is the parser's invalid dictionary-unpack comprehension.
            Expr::DictComp(_) => return,
            _ => {
                return stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    visitor::walk_expr(self, expr)
                });
            }
        };
        self.error = Some(error);
    }
}

impl<'a> Rhs<'a> {
    fn comprehension(&mut self, expr: &'a Expr, plan: &Plan<'a>) {
        self.visit_expr(plan.outer);
        let mut child = Self {
            generic_class: self.generic_class,
            error: self.error,
            comprehension: Some(expr),
            in_iterable: self.in_iterable,
        };
        let mut inner_iterable = None;
        for step in plan.steps() {
            match *step {
                Step::Open { iterable, .. } => inner_iterable = Some(iterable),
                Step::Next { target, .. } => {
                    // Symbol analysis checks a binding before its inner iterable.
                    child.visit_expr(target);
                    if let Some(iterable) = inner_iterable.take() {
                        child.in_iterable = true;
                        child.visit_expr(iterable);
                        child.in_iterable = self.in_iterable;
                    }
                }
                Step::Filter { test, .. } => child.visit_expr(test),
                Step::Emit { output, .. } => {
                    // CPython visits a dict's value first when selecting a symbol error.
                    child.visit_expr(output.value());
                    if let Some(key) = output.key() {
                        child.visit_expr(key);
                    }
                }
            }
        }
        self.error = child.error;
        if self.error.is_none()
            && self.comprehension.is_none()
            && !matches!(plan.output, Output::Generator(_))
            && plan.body_suspends()
        {
            self.error = Some((
                expr,
                "asynchronous comprehension outside of an asynchronous function",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_generic_base_errors_in_statement_order() {
        for (source, expected, offender) in [
            (
                "if False:\n class C[T](base([0 for t[(saved:=0)] in rows])): pass",
                "assignment expression within a comprehension cannot be used within the definition of a generic",
                "saved",
            ),
            (
                "class C[T](base([0 for t[await key()] in rows])): pass",
                "asynchronous comprehension outside of an asynchronous function",
                "[0 for t[await key()] in rows]",
            ),
            (
                "class C[T,T](base([0 for t[(saved:=0)] in rows])): pass",
                "duplicate type parameter 'T'",
                "T",
            ),
        ] {
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            let (span, reason) = annotation_scope_error(&parsed.syntax().body).unwrap();
            assert_eq!(reason, expected);
            assert_eq!(&source[span], offender);
        }
    }

    #[test]
    fn parser_rejects_dictionary_unpack_before_symbol_validation() {
        for source in [
            "type T = {**x for x in rows}",
            "type T = {\n   **(saved := x) for x in rows}",
        ] {
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            let (span, message) = crate::comprehension::dict_unpacking_error(&parsed).unwrap();
            assert_eq!(&source[span], "**");
            assert_eq!(
                message,
                "dict unpacking cannot be used in dict comprehension"
            );
        }
    }

    #[test]
    fn comprehension_targets_and_error_order_in_alias_scopes() {
        let walrus = "assignment expression within a comprehension cannot be used in a type alias";
        let eager = "asynchronous comprehension outside of an asynchronous function";
        for (expr, message, offender) in [
            (
                "[0 for t[await key()] in it]",
                eager,
                "[0 for t[await key()] in it]",
            ),
            ("[0 for t[(target := 0)] in it]", walrus, "target"),
            (
                "[0 async for t[await key()] in it if (predicate := 0)]",
                walrus,
                "predicate",
            ),
            (
                "[0 for x in it for t[(target := 0)] in (source := it)]",
                walrus,
                "target",
            ),
            ("{(key := 1): (value := 2) for x in it}", walrus, "value"),
            (
                "[[y async for y in x] for x in it]",
                eager,
                "[[y async for y in x] for x in it]",
            ),
        ] {
            let source = format!("if False:\n    type T = {expr}\n");
            let parsed = ruff_python_parser::parse_module(&source).unwrap();
            let (range, reason) = annotation_scope_error(&parsed.syntax().body).unwrap();
            assert_eq!(reason, message, "{expr}");
            assert_eq!(&source[range], offender, "{expr}");
        }
        for expr in [
            "[0 for t[0] in it]",
            "(0 for t[await key()] in it)",
            "(0 async for x in it)",
        ] {
            let parsed = ruff_python_parser::parse_module(&format!("type T = {expr}")).unwrap();
            assert!(
                annotation_scope_error(&parsed.syntax().body).is_none(),
                "{expr}"
            );
        }
    }
}
