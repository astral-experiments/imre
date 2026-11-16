//! Source spelling of annotation expressions shared by native and runtime compilation.
use ruff_python_ast as ast;
use ruff_python_codegen::{Generator, Mode};

/// Annotation providers publish this positional-only argument as `format`.
/// Type-parameter and alias evaluators publish their separate `.format` argument verbatim.
pub const FORMAT_PARAMETER: &str = ".__annotate__.format";

/// A postponed-annotation dictionary is initialized if the suite contains an
/// annotated assignment, even on an unreachable or non-name target.
pub fn has_variable_annotation(body: &[ast::Stmt]) -> bool {
    use ast::statement_visitor::{StatementVisitor, walk_stmt};
    struct Finder(bool);
    impl<'a> StatementVisitor<'a> for Finder {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::AnnAssign(_) => self.0 = true,
                ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_) => {}
                _ if !self.0 => walk_stmt(self, stmt),
                _ => {}
            }
        }
    }
    let mut finder = Finder(false);
    finder.visit_body(body);
    finder.0
}

/// Return the expression spelling used in Python annotation dictionaries.
pub fn spelling(annotation: &ast::Expr) -> String {
    fn items(items: &[ast::Expr]) -> String {
        items.iter().map(spelling).collect::<Vec<_>>().join(", ")
    }
    match annotation {
        ast::Expr::Subscript(s) => {
            let slice = match &*s.slice {
                ast::Expr::Tuple(t) if !t.elts.is_empty() => {
                    let mut args = items(&t.elts);
                    if t.elts.len() == 1 {
                        args.push(',');
                    }
                    args
                }
                expr => spelling(expr),
            };
            let mut base = spelling(&s.value);
            if matches!(&*s.value, ast::Expr::BinOp(_)) {
                base = format!("({base})");
            }
            return format!("{base}[{slice}]");
        }
        ast::Expr::List(l) => return format!("[{}]", items(&l.elts)),
        ast::Expr::Tuple(t) => {
            return format!(
                "({}{})",
                items(&t.elts),
                if t.elts.len() == 1 { "," } else { "" }
            );
        }
        ast::Expr::BinOp(b) if b.op == ast::Operator::BitOr && symbolic(annotation) => {
            let mut right = spelling(&b.right);
            if matches!(&*b.right, ast::Expr::BinOp(_)) {
                right = format!("({right})");
            }
            return format!("{} | {right}", spelling(&b.left));
        }
        _ => {}
    }
    Generator::new(&Default::default(), Default::default())
        .with_mode(Mode::AstUnparse)
        .expr(annotation)
}

// The STRING protocol evaluates expressions in a symbolic namespace. Only
// forms whose spelling survives that evaluation can use the AST unparser.
pub fn symbolic(value: &ast::Expr) -> bool {
    match value {
        ast::Expr::Name(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::NoneLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::EllipsisLiteral(_) => true,
        ast::Expr::Attribute(a) => symbolic(&a.value),
        ast::Expr::Subscript(s) => symbolic(&s.value) && symbolic(&s.slice),
        ast::Expr::Tuple(t) => t.elts.iter().all(symbolic),
        ast::Expr::List(l) => l.elts.iter().all(symbolic),
        ast::Expr::BinOp(b) if b.op == ast::Operator::BitOr => {
            symbolic(&b.left)
                && symbolic(&b.right)
                && (matches!(
                    &*b.left,
                    ast::Expr::Name(_)
                        | ast::Expr::Subscript(_)
                        | ast::Expr::Attribute(_)
                        | ast::Expr::BinOp(_)
                ) || matches!(
                    &*b.right,
                    ast::Expr::Name(_) | ast::Expr::Subscript(_) | ast::Expr::Attribute(_)
                ))
        }
        _ => false,
    }
}
