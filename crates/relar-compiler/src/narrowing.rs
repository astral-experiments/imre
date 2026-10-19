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
            let value = match compare.operands.as_ref() {
                [value, ast::Expr::NoneLiteral(_)] | [ast::Expr::NoneLiteral(_), value] => value,
                _ => return HashMap::new(),
            };
            let name = match value {
                ast::Expr::Name(name) => Some(name),
                ast::Expr::Named(named) => match &*named.target {
                    ast::Expr::Name(name) => Some(name),
                    _ => None,
                },
                _ => None,
            };
            name.map_or_else(HashMap::new, |name| {
                HashMap::from([(name.id.to_string(), present)])
            })
        }
        ast::Expr::UnaryOp(unary) if unary.op == ast::UnaryOp::Not => facts(&unary.operand, !truth),
        ast::Expr::BoolOp(boolean)
            if (boolean.op == ast::BoolOp::And && truth)
                || (boolean.op == ast::BoolOp::Or && !truth) =>
        {
            let mut result = HashMap::new();
            let mut conflicting = HashSet::new();
            for value in &boolean.values {
                // A later operand can replace the value observed by an earlier
                // test, even when that operand contributes no None fact itself.
                for name in expression_assignments(value) {
                    result.remove(&name);
                    conflicting.remove(&name);
                }
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

/// Finds writes in an expression, including writes escaping eager displays.
pub(crate) fn expression_assignments(expression: &ast::Expr) -> HashSet<String> {
    struct Writes(HashSet<String>);
    impl<'a> Visitor<'a> for Writes {
        fn visit_expr(&mut self, expression: &'a ast::Expr) {
            if let ast::Expr::Named(value) = expression
                && let ast::Expr::Name(name) = &*value.target
            {
                self.0.insert(name.id.to_string());
            }
            walk_expr(self, expression);
        }
    }
    let mut writes = Writes(HashSet::new());
    writes.visit_expr(expression);
    writes.0
}

/// Names written on every normally completed path, optionally restricted to a
/// boolean result. Later short-circuit operands and empty displays do not run
/// on every path; a true `and` or false `or` proves all of its operands ran.
pub(crate) fn guaranteed_assignments(
    expression: &ast::Expr,
    truth: Option<bool>,
) -> HashSet<String> {
    match expression {
        ast::Expr::Named(value) => {
            let mut names = guaranteed_assignments(&value.value, truth);
            if let ast::Expr::Name(name) = &*value.target {
                names.insert(name.id.to_string());
            }
            names
        }
        ast::Expr::UnaryOp(value) if value.op == ast::UnaryOp::Not => {
            guaranteed_assignments(&value.operand, truth.map(|truth| !truth))
        }
        ast::Expr::BoolOp(value) => {
            if truth == Some(value.op == ast::BoolOp::And) {
                value
                    .values
                    .iter()
                    .flat_map(|value| guaranteed_assignments(value, truth))
                    .collect()
            } else {
                guaranteed_assignments(&value.values[0], None)
            }
        }
        ast::Expr::Compare(value) => value
            .operands
            .iter()
            .take(if truth == Some(true) {
                value.operands.len()
            } else {
                2
            })
            .flat_map(|value| guaranteed_assignments(value, None))
            .collect(),
        ast::Expr::If(value) => {
            let mut names = guaranteed_assignments(&value.test, None);
            let yes = guaranteed_assignments(&value.body, truth);
            let no = guaranteed_assignments(&value.orelse, truth);
            names.extend(yes.intersection(&no).cloned());
            names
        }
        ast::Expr::ListComp(value) => guaranteed_assignments(&value.generators[0].iter, None),
        ast::Expr::SetComp(value) => guaranteed_assignments(&value.generators[0].iter, None),
        ast::Expr::DictComp(value) => guaranteed_assignments(&value.generators[0].iter, None),
        _ => {
            struct Children(HashSet<String>);
            impl<'a> Visitor<'a> for Children {
                fn visit_expr(&mut self, expression: &'a ast::Expr) {
                    self.0.extend(guaranteed_assignments(expression, None));
                }
            }
            let mut children = Children(HashSet::new());
            walk_expr(&mut children, expression);
            children.0
        }
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
        // Eager displays own iteration targets, but assignment expressions
        // write in the enclosing non-comprehension scope.
        let clauses = match expression {
            ast::Expr::ListComp(value) => Some(value.generators.as_slice()),
            ast::Expr::SetComp(value) => Some(value.generators.as_slice()),
            ast::Expr::DictComp(value) => Some(value.generators.as_ref()),
            _ => None,
        };
        if let Some(clauses) = clauses {
            self.0.extend(expression_assignments(expression));
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
