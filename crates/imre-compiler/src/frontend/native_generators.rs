//! Direct consumers can suspend and resume a source-defined, annotated producer.
//! Requiring a terminal yield outside handlers makes the close edge an ordinary
//! early return: no producer can catch GeneratorExit or yield during cleanup.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use std::result::Result;

struct Yield(bool);
impl<'a> Visitor<'a> for Yield {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if !matches!(stmt, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_)) {
            walk_stmt(self, stmt);
        }
    }
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        match expr {
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => self.0 = true,
            ast::Expr::Lambda(_) => {}
            _ => walk_expr(self, expr),
        }
    }
}

pub(super) fn item_type(
    function: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: usize,
    classes: &ClassInfo,
    types: &Types,
) -> Result<Option<Type>, String> {
    let Some(ast::Expr::Subscript(result)) = function.returns.as_deref() else {
        return Ok(None);
    };
    if graph.resolve(module, &result.value) != Some(Binding::Intrinsic(Intrinsic::Iterator)) {
        return Ok(None);
    }
    let unsupported = || {
        graph.modules[module].error(
            function,
            "a generator requires a single terminal for loop with a final yield of a value",
        )
    };
    let without_terminal_yield = || {
        // An Iterator annotation can also describe an ordinary function.
        let mut yields = Yield(false);
        yields.visit_body(&function.body);
        if yields.0 {
            Err(unsupported())
        } else {
            Ok(None)
        }
    };
    let Some((ast::Stmt::For(loop_), prelude)) = function.body.split_last() else {
        return without_terminal_yield();
    };
    let Some((ast::Stmt::Expr(tail), prefix)) = loop_.body.split_last() else {
        return without_terminal_yield();
    };
    let ast::Expr::Yield(y) = &*tail.value else {
        return without_terminal_yield();
    };
    let Some(value) = &y.value else {
        return Err(unsupported());
    };
    let mut other = Yield(false);
    other.visit_body(prelude);
    other.visit_expr(&loop_.iter);
    other.visit_body(prefix);
    other.visit_expr(value);
    if function.is_async
        || !function.decorator_list.is_empty()
        || loop_.is_async
        || !loop_.orelse.is_empty()
        || other.0
    {
        return Err(unsupported());
    }
    let element = annotation(&result.slice, graph, module, classes.declarations, types)?;
    if types.may_own_file(element, &classes.fields, true) {
        return Err(graph.modules[module].error(
            &**value,
            "a yielded element cannot own a finalizable resource",
        ));
    }
    Ok(Some(element))
}

impl Checker<'_> {
    pub(super) fn generator_signature(
        &self,
        binding: Option<Binding>,
    ) -> Option<(Signature, Type)> {
        let Binding::Definition(id) = binding? else {
            return None;
        };
        let signature = self.functions.get(&id)?;
        let element = *self.classes.producer_items.get(&signature.id)?;
        Some((signature.clone(), element))
    }
}
