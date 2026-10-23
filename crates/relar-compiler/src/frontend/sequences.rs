//! A Sequence parameter borrows an exact typed list through a read-only interface.

use ruff_python_ast::visitor::{Visitor, walk_comprehension, walk_expr, walk_stmt};

use super::*;

pub(super) fn check_reads(
    function: &ast::StmtFunctionDef,
    parameter: &str,
    source: &str,
) -> Result<(), String> {
    let mut reads = Reads {
        parameter,
        allowed: HashSet::new(),
        invalid: None,
    };
    reads.visit_body(&function.body);
    if let Some(expr) = reads.invalid {
        Err(error(
            source,
            expr,
            "a Sequence parameter supports iteration, len, and read-only indexing",
        ))
    } else {
        Ok(())
    }
}

struct Reads<'a, 'ast> {
    parameter: &'a str,
    allowed: HashSet<TextRange>,
    invalid: Option<&'ast ast::Expr>,
}

impl Reads<'_, '_> {
    fn allow(&mut self, expr: &ast::Expr) {
        if matches!(expr, ast::Expr::Name(name) if name.id == self.parameter) {
            self.allowed.insert(expr.range());
        }
    }
}

impl<'ast> Visitor<'ast> for Reads<'_, 'ast> {
    fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
        if let ast::Stmt::For(loop_) = stmt {
            self.allow(&loop_.iter);
        }
        walk_stmt(self, stmt);
    }

    fn visit_comprehension(&mut self, clause: &'ast ast::Comprehension) {
        self.allow(&clause.iter);
        walk_comprehension(self, clause);
    }

    fn visit_expr(&mut self, expr: &'ast ast::Expr) {
        match expr {
            ast::Expr::Name(name)
                if name.id == self.parameter && !self.allowed.remove(&expr.range()) =>
            {
                self.invalid.get_or_insert(expr);
            }
            ast::Expr::Subscript(subscript) if subscript.ctx == ast::ExprContext::Load => {
                self.allow(&subscript.value);
            }
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "len")
                    && let [arg] = call.arguments.args.as_ref() =>
            {
                self.allow(arg);
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}
