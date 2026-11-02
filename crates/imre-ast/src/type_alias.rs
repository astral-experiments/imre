//! Symbol-table restrictions on expressions evaluated by a `type` declaration.
use ast::visitor::{self, Visitor};
use ruff_python_ast as ast;

/// Return the offending expression and Python diagnostic before compiling a suite.
pub fn check(body: &[ast::Stmt]) -> Option<(&ast::Expr, String)> {
    #[derive(Default)]
    struct Aliases<'a>(Option<(&'a ast::Expr, String)>);
    impl<'a> Visitor<'a> for Aliases<'a> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.0.is_none() {
                if let ast::Stmt::TypeAlias(alias) = stmt {
                    self.0 = check_alias(alias);
                } else {
                    visitor::walk_stmt(self, stmt);
                }
            }
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }
    let mut aliases = Aliases::default();
    aliases.visit_body(body);
    aliases.0
}

/// Check the alias binding and lazy expression, including unexecuted declarations.
pub fn check_alias(alias: &ast::StmtTypeAlias) -> Option<(&ast::Expr, String)> {
    if alias
        .name
        .as_name_expr()
        .is_some_and(|name| name.id == "__debug__")
    {
        return Some((&alias.name, "cannot assign to __debug__".into()));
    }
    let mut scan = AliasExpression::default();
    scan.visit_expr(&alias.value);
    scan.error
}

#[derive(Default)]
struct AliasExpression<'a> {
    comprehensions: Vec<(&'static str, &'a [ast::Comprehension])>,
    in_iterable: bool,
    error: Option<(&'a ast::Expr, String)>,
}

impl<'a> AliasExpression<'a> {
    fn comprehension(
        &mut self,
        kind: &'static str,
        clauses: &'a [ast::Comprehension],
        results: impl IntoIterator<Item = &'a ast::Expr>,
    ) {
        let was_iterable = std::mem::replace(&mut self.in_iterable, true);
        self.visit_expr(&clauses[0].iter);
        self.in_iterable = was_iterable;
        self.comprehensions.push((kind, clauses));
        for (index, clause) in clauses.iter().enumerate() {
            if index != 0 {
                self.in_iterable = true;
                self.visit_expr(&clause.iter);
                self.in_iterable = was_iterable;
            }
            for filter in &clause.ifs {
                self.visit_expr(filter);
            }
        }
        for expr in results {
            self.visit_expr(expr);
        }
        self.comprehensions.pop();
    }

    fn named_error(&self, named: &ast::ExprNamed) -> String {
        if self.comprehensions.is_empty() {
            return "named expression cannot be used within a type alias".into();
        }
        if self.in_iterable {
            return "assignment expression cannot be used in a comprehension iterable expression"
                .into();
        }
        let target = named
            .target
            .as_name_expr()
            .expect("named expression target")
            .id
            .as_str();
        if self
            .comprehensions
            .iter()
            .flat_map(|(_, clauses)| *clauses)
            .any(|c| binds(&c.target, target))
        {
            return format!(
                "assignment expression cannot rebind comprehension iteration variable '{target}'"
            );
        }
        "assignment expression within a comprehension cannot be used in a type alias".into()
    }
}

fn binds(expr: &ast::Expr, spelling: &str) -> bool {
    match expr {
        ast::Expr::Name(name) => name.id == spelling,
        ast::Expr::Tuple(t) => t.elts.iter().any(|e| binds(e, spelling)),
        ast::Expr::List(l) => l.elts.iter().any(|e| binds(e, spelling)),
        ast::Expr::Starred(s) => binds(&s.value, spelling),
        _ => false,
    }
}

impl<'a> Visitor<'a> for AliasExpression<'a> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if self.error.is_some() {
            return;
        }
        let message = match expr {
            ast::Expr::Named(named) => self.named_error(named),
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => {
                if let Some((kind, _)) = self.comprehensions.last() {
                    let word = if matches!(expr, ast::Expr::YieldFrom(_)) {
                        "yield from"
                    } else {
                        "yield"
                    };
                    format!("'{word}' inside {kind}")
                } else {
                    "yield expression cannot be used within a type alias".into()
                }
            }
            ast::Expr::Await(_) if self.comprehensions.is_empty() => {
                "await expression cannot be used within a type alias".into()
            }
            _ => {
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match expr {
                    ast::Expr::Lambda(lambda) => {
                        // Only default expressions belong to the surrounding alias scope.
                        if let Some(params) = &lambda.parameters {
                            self.visit_parameters(params);
                        }
                    }
                    ast::Expr::ListComp(c) => {
                        self.comprehension("list comprehension", &c.generators, [c.elt.as_ref()])
                    }
                    ast::Expr::SetComp(c) => {
                        self.comprehension("set comprehension", &c.generators, [c.elt.as_ref()])
                    }
                    ast::Expr::DictComp(c) => self.comprehension(
                        "dict comprehension",
                        &c.generators,
                        c.key.as_deref().into_iter().chain([c.value.as_ref()]),
                    ),
                    ast::Expr::Generator(c) => {
                        self.comprehension("generator expression", &c.generators, [c.elt.as_ref()])
                    }
                    _ => visitor::walk_expr(self, expr),
                });
                return;
            }
        };
        self.error = Some((expr, message));
    }
}
