//! A Sequence parameter borrows an exact typed list through a read-only interface.

use ruff_python_ast::visitor::{Visitor, walk_comprehension, walk_expr, walk_stmt};

use super::*;

pub(super) fn check_reads(
    function: &ast::StmtFunctionDef,
    parameter: &str,
    empty_tuple_default: bool,
    graph: &SourceGraph,
    module: ModuleId,
    source: &str,
) -> Result<(), String> {
    let mut reads = Reads {
        parameter,
        empty_tuple_default,
        pack: false,
        graph,
        module,
        allowed: HashSet::new(),
        invalid: None,
    };
    reads.visit_body(&function.body);
    if let Some(expr) = reads.invalid {
        Err(error(
            source,
            expr,
            if empty_tuple_default {
                "a Sequence parameter with an empty tuple default supports iteration, map(str, ...), and len"
            } else {
                "a Sequence parameter supports iteration, map(str, ...), len, and read-only indexing"
            },
        ))
    } else {
        Ok(())
    }
}

/// A variadic tuple can use homogeneous backend storage when the body only
/// observes its elements. Tuple identity, slicing, mutation, and escape remain
/// unsupported rather than exposing the private list that carries the values.
pub(super) fn check_pack_reads(
    function: &ast::StmtFunctionDef,
    parameter: &str,
    graph: &SourceGraph,
    module: ModuleId,
    source: &str,
) -> Result<(), String> {
    let mut reads = Reads {
        parameter,
        empty_tuple_default: false,
        pack: true,
        graph,
        module,
        allowed: HashSet::new(),
        invalid: None,
    };
    reads.visit_body(&function.body);
    if let Some(expr) = reads.invalid {
        Err(error(
            source,
            expr,
            "a variadic positional parameter supports iteration, expansion, and len",
        ))
    } else {
        Ok(())
    }
}

struct Reads<'a, 'ast> {
    parameter: &'a str,
    empty_tuple_default: bool,
    pack: bool,
    graph: &'a SourceGraph,
    module: ModuleId,
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
        if let ast::Stmt::FunctionDef(nested) = stmt
            && !comprehensions::function_reads(nested).contains(self.parameter)
        {
            // Defaults belong to the enclosing scope even when the body
            // binds its own local with the outer parameter's spelling.
            self.visit_parameters(&nested.parameters);
            return;
        }
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
        let clauses = match expr {
            ast::Expr::ListComp(value) => Some(value.generators.as_slice()),
            ast::Expr::SetComp(value) => Some(value.generators.as_slice()),
            ast::Expr::DictComp(value) => Some(value.generators.as_ref()),
            ast::Expr::Generator(value) => Some(value.generators.as_slice()),
            _ => None,
        };
        if let Some(clauses) = clauses {
            let mut targets = HashSet::new();
            for clause in clauses {
                comprehensions::target_names(&clause.target, &mut targets);
            }
            if targets.contains(self.parameter) {
                // Only the first iterable is evaluated outside the
                // comprehension's private scope.
                self.allow(&clauses[0].iter);
                self.visit_expr(&clauses[0].iter);
                return;
            }
        }
        if let ast::Expr::Lambda(lambda) = expr
            && !comprehensions::free_reads(expr, HashSet::new()).contains(self.parameter)
        {
            if let Some(parameters) = &lambda.parameters {
                self.visit_parameters(parameters);
            }
            return;
        }
        match expr {
            ast::Expr::BinOp(binary) if self.pack && binary.op == ast::Operator::Mod => {
                self.allow(&binary.right)
            }
            ast::Expr::Starred(star) if self.pack && star.ctx == ast::ExprContext::Load => {
                self.allow(&star.value)
            }
            ast::Expr::Name(name)
                if name.id == self.parameter && !self.allowed.remove(&expr.range()) =>
            {
                self.invalid.get_or_insert(expr);
            }
            ast::Expr::Subscript(subscript)
                if subscript.ctx == ast::ExprContext::Load
                    && !self.empty_tuple_default
                    && !self.pack =>
            {
                self.allow(&subscript.value);
            }
            ast::Expr::Call(call)
                if self.graph.resolve(self.module, &call.func)
                    == Some(Binding::Intrinsic(Intrinsic::Ndiff)) =>
            {
                for arg in &call.arguments.args {
                    self.allow(arg);
                }
            }
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "len")
                    && let [arg] = call.arguments.args.as_ref() =>
            {
                self.allow(arg);
            }
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "map")
                    && let [ast::Expr::Name(convert), arg] = call.arguments.args.as_ref()
                    && convert.id == "str" =>
            {
                self.allow(arg);
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use crate::check;

    #[test]
    fn shadowing_still_checks_outer_iterables_and_free_captures() {
        let prefix = "from typing import Sequence\ndef mutate(values: list[int]) -> list[int]:\n    values.append(3)\n    return values\ndef use(xs: Sequence[int]) -> list[int]:\n";
        for body in [
            "    return [xs for xs in mutate(xs)]\n",
            "    def inner() -> list[int]:\n        return mutate(xs)\n    return inner()\n",
        ] {
            assert!(
                check(&format!("{prefix}{body}"))
                    .unwrap_err()
                    .contains("a Sequence parameter supports")
            );
        }
    }
}
