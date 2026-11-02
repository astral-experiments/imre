//! Validate nonlocal declarations against the lexical scope of type parameters.
use super::{ClosureNames, Visitor, ast, visitor};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum Binding {
    Parameter,
    Global,
}

/// Return a source-level error before any effects from the module can execute.
pub(in super::super) fn check_nonlocals(source: &str, body: &[ast::Stmt]) -> Result<(), String> {
    struct Check<'s> {
        source: &'s str,
        bindings: BTreeMap<String, Binding>,
        error: Option<String>,
    }

    impl<'a> Visitor<'a> for Check<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.error.is_some() {
                return;
            }
            match stmt {
                ast::Stmt::ClassDef(class) => {
                    let outer = self.bindings.clone();
                    if let Some(params) = &class.type_params {
                        self.bindings.extend(
                            params
                                .iter()
                                .map(|p| (p.name().to_string(), Binding::Parameter)),
                        );
                    }
                    if !self.bindings.is_empty() {
                        // CPython removes ordinary class-local bindings from its type-parameter
                        // set for nested scopes, but a class still doesn't bind a closure cell.
                        let locals = super::class_locals(&class.body).locals;
                        self.bindings.retain(|name, binding| {
                            !matches!(binding, Binding::Parameter) || !locals.contains(name)
                        });
                    }
                    self.visit_body(&class.body);
                    self.bindings = outer;
                }
                ast::Stmt::FunctionDef(function)
                    if function.type_params.is_some() || !self.bindings.is_empty() =>
                {
                    // An intervening ordinary local hides the type parameter. A
                    // nonlocal declaration forwards the lookup to its outer scope.
                    let outer = self.bindings.clone();
                    if let Some(params) = &function.type_params {
                        self.bindings.extend(
                            params
                                .iter()
                                .map(|p| (p.name().to_string(), Binding::Parameter)),
                        );
                    }
                    let mut names = ClosureNames::default();
                    names.visit_body(&function.body);
                    names.locals.extend(
                        function
                            .parameters
                            .iter()
                            .map(|p| p.as_parameter().name.to_string()),
                    );
                    self.bindings.retain(|name, binding| {
                        if names.globals.contains(name) {
                            *binding = Binding::Global;
                            true
                        } else {
                            names.nonlocals.contains(name) || !names.locals.contains(name)
                        }
                    });
                    self.visit_body(&function.body);
                    self.bindings = outer;
                }
                ast::Stmt::Nonlocal(statement) => {
                    for name in &statement.names {
                        let message = match self.bindings.get(name.as_str()) {
                            Some(Binding::Parameter) => format!(
                                "SyntaxError: nonlocal binding not allowed for type parameter '{name}'"
                            ),
                            Some(Binding::Global) => {
                                format!("SyntaxError: no binding for nonlocal '{name}' found")
                            }
                            None => continue,
                        };
                        self.error = Some(crate::frontend::error(self.source, statement, &message));
                        break;
                    }
                }
                _ => visitor::walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }

    let mut check = Check {
        source,
        bindings: BTreeMap::new(),
        error: None,
    };
    check.visit_body(body);
    check.error.map_or(Ok(()), Err)
}
