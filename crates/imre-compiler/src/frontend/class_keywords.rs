use super::{ast, error};
use ast::visitor::{Visitor, walk_stmt};
use std::collections::HashSet;

/// Repeated literal class keywords are a syntax error even if the class never executes.
pub(crate) fn check_class_keywords(source: &str, body: &[ast::Stmt]) -> Result<(), String> {
    struct Check<'a> {
        source: &'a str,
        error: Option<String>,
    }
    impl<'a> Visitor<'a> for Check<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.error.is_some() {
                return;
            }
            if let ast::Stmt::ClassDef(class) = stmt
                && let Some(args) = &class.arguments
            {
                let mut seen = HashSet::new();
                for keyword in &args.keywords {
                    if let Some(name) = &keyword.arg
                        && !seen.insert(name.as_str())
                    {
                        self.error = Some(error(
                            self.source,
                            keyword,
                            &format!("keyword argument repeated: {name}"),
                        ));
                        return;
                    }
                }
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }
    let mut check = Check {
        source,
        error: None,
    };
    check.visit_body(body);
    check.error.map_or(Ok(()), Err)
}
