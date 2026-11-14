use ruff_python_ast as ast;
use std::collections::BTreeSet;

/// Builtin bases whose Python subclass allocator installs separate collection storage.
pub const COLLECTION_STORAGE_BASES: [&str; 3] = ["set", "frozenset", "bytearray"];

/// CPython records self.X stores from code units immediately enclosed by a class.
pub fn static_attributes(body: &[ast::Stmt], spellings: &crate::Names) -> BTreeSet<String> {
    use ast::visitor::{self, Visitor};
    struct Attributes<'a> {
        spellings: &'a crate::Names,
        names: BTreeSet<String>,
        collect: bool,
        in_nested_class: bool,
    }
    impl<'a> Visitor<'a> for Attributes<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            let (body, nested_class) = match stmt {
                // Augmented assignments use their own store instruction; CPython only
                // registers attributes stored by ordinary assignment targets.
                ast::Stmt::AugAssign(update)
                    if matches!(update.target.as_ref(), ast::Expr::Attribute(_)) =>
                {
                    let ast::Expr::Attribute(target) = update.target.as_ref() else {
                        unreachable!()
                    };
                    self.visit_expr(&target.value);
                    self.visit_expr(&update.value);
                    return;
                }
                ast::Stmt::AnnAssign(update) if update.value.is_none() => return,
                ast::Stmt::FunctionDef(def) => (&def.body, false),
                ast::Stmt::ClassDef(def) => (&def.body, true),
                _ => return visitor::walk_stmt(self, stmt),
            };
            if !self.in_nested_class {
                let collect = self.collect;
                self.collect = true;
                self.in_nested_class = nested_class;
                self.visit_body(body);
                self.collect = collect;
                self.in_nested_class = false;
            }
        }
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if self.collect {
                if let ast::Expr::Attribute(a) = expr
                    && a.ctx == ast::ExprContext::Store
                    && matches!(&*a.value, ast::Expr::Name(n) if n.id.as_str() == "self")
                {
                    self.names
                        .insert(self.spellings.definition(&a.attr).to_owned());
                }
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    visitor::walk_expr(self, expr)
                });
            }
        }
    }
    let mut attributes = Attributes {
        spellings,
        names: BTreeSet::new(),
        collect: false,
        in_nested_class: false,
    };
    attributes.visit_body(body);
    attributes.names
}
