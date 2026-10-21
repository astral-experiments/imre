//! Conservative local None facts, independent of checked types and IR lowering.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_except_handler, walk_expr, walk_pattern};

/// Returns facts guaranteed when `test` has the requested truth value.
/// A true fact means non-None. Only local names are eligible: attribute and
/// subscript values can change through aliases without rebinding the receiver.
pub(crate) fn facts(test: &ast::Expr, truth: bool) -> HashMap<String, bool> {
    match test {
        ast::Expr::Compare(compare) if compare.ops.len() == 1 => {
            let present = match compare.ops[0] {
                ast::CmpOp::Is => !truth,
                ast::CmpOp::IsNot => truth,
                _ => return HashMap::new(),
            };
            match compare.operands.as_ref() {
                [ast::Expr::Name(name), ast::Expr::NoneLiteral(_)]
                | [ast::Expr::NoneLiteral(_), ast::Expr::Name(name)] => {
                    HashMap::from([(name.id.to_string(), present)])
                }
                _ => HashMap::new(),
            }
        }
        ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => facts(&unary.operand, !truth),
        ast::Expr::BoolOp(boolean)
            if (boolean.op == ast::BoolOp::And && truth)
                || (boolean.op == ast::BoolOp::Or && !truth) =>
        {
            let mut result = HashMap::new();
            let mut conflicting = HashSet::new();
            for value in &boolean.values {
                for (name, present) in facts(value, truth) {
                    if let Some(previous) = result.insert(name.clone(), present)
                        && previous != present
                    {
                        conflicting.insert(name);
                    }
                }
            }
            for name in conflicting {
                result.remove(&name);
            }
            result
        }
        _ => HashMap::new(),
    }
}

/// Finds local stores anywhere in a loop body, including its branches and loops.
/// Callers must invalidate these names' incoming facts before checking a loop:
/// later iterations can observe a value written on an earlier backedge.
pub(crate) fn assigned_names(body: &[ast::Stmt]) -> HashSet<String> {
    let mut visitor = Assignments(HashSet::new());
    for statement in body {
        visitor.visit_stmt(statement);
    }
    visitor.0
}

struct Assignments(HashSet<String>);

impl<'a> Visitor<'a> for Assignments {
    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        let name = match pattern {
            ast::Pattern::MatchAs(pattern) => pattern.name.as_ref(),
            ast::Pattern::MatchStar(pattern) => pattern.name.as_ref(),
            ast::Pattern::MatchMapping(pattern) => pattern.rest.as_ref(),
            _ => None,
        };
        if let Some(name) = name {
            self.0.insert(name.to_string());
        }
        walk_pattern(self, pattern);
    }

    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        // Eager displays own their target bindings. Only the leftmost iterable
        // executes in this scope; walrus expressions remain unsupported.
        let clauses = match expression {
            ast::Expr::ListComp(value) => Some(value.generators.as_slice()),
            ast::Expr::SetComp(value) => Some(value.generators.as_slice()),
            ast::Expr::DictComp(value) => Some(value.generators.as_ref()),
            _ => None,
        };
        if let Some(clauses) = clauses {
            self.visit_expr(&clauses[0].iter);
            return;
        }
        if let ast::Expr::Name(name) = expression
            && name.ctx == ast::ExprContext::Store
        {
            self.0.insert(name.id.to_string());
        }
        walk_expr(self, expression);
    }

    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(handler_) = handler;
        if let Some(name) = &handler_.name {
            self.0.insert(name.to_string());
        }
        walk_except_handler(self, handler);
    }
}

/// Handler targets are deleted on exit, even when the name previously existed.
/// An outer handler cannot assume such a local survived the protected body.
pub(crate) fn deleted_names(body: &[ast::Stmt]) -> HashSet<String> {
    struct Deletions(HashSet<String>);
    impl<'a> Visitor<'a> for Deletions {
        fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
            let ast::ExceptHandler::ExceptHandler(handler_) = handler;
            if let Some(name) = &handler_.name {
                self.0.insert(name.to_string());
            }
            walk_except_handler(self, handler);
        }
    }
    let mut visitor = Deletions(HashSet::new());
    for statement in body {
        visitor.visit_stmt(statement);
    }
    visitor.0
}
