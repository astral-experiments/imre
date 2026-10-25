//! Closed call shapes: fixed tuple fields and fresh keyword displays need no dynamic ABI.

mod dispatch;
mod keywords;

use super::*;
use ruff_python_ast::visitor::transformer::{self, Transformer};
use ruff_python_ast::visitor::{
    Visitor, walk_except_handler, walk_expr, walk_keyword, walk_pattern, walk_stmt,
};

/// Match the ABI order: ordinary parameters, keyword-only parameters, then the private pack.
pub(super) fn parameters(params: &ast::Parameters) -> impl Iterator<Item = &ast::Parameter> {
    params
        .iter_non_variadic_params()
        .map(|p| &p.parameter)
        .chain(params.vararg.as_deref())
}

fn project(value: ast::Expr, index: usize) -> ast::Expr {
    let range = value.range();
    ast::Expr::Subscript(ast::ExprSubscript {
        node_index: ast::AtomicNodeIndex::NONE,
        range,
        value: Box::new(value),
        slice: Box::new(ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
            node_index: ast::AtomicNodeIndex::NONE,
            range,
            value: ast::Number::Int((index as u64).into()),
        })),
        ctx: ast::ExprContext::Load,
    })
}

/// A repeated key would discard an already evaluated expression. Leave it to the
/// ordinary unsupported-unpacking diagnostic rather than changing its effects.
fn keys(dict: &ast::ExprDict) -> Option<Vec<String>> {
    let mut names = Vec::with_capacity(dict.items.len());
    for item in &dict.items {
        let ast::Expr::StringLiteral(key) = item.key.as_ref()? else {
            return None;
        };
        let key = key.value.to_string();
        if names.contains(&key) {
            return None;
        }
        names.push(key);
    }
    Some(names)
}

fn keyword(name: &str, value: ast::Expr, range: TextRange) -> ast::Keyword {
    ast::Keyword {
        node_index: ast::AtomicNodeIndex::NONE,
        range,
        arg: Some(ast::Identifier::new(name, range)),
        value,
    }
}

impl Checker<'_> {
    /// Expand fresh keyword displays. Positional spreads use the ordinary binder.
    pub(super) fn expand_closed_call(&self, call: &ast::ExprCall) -> Option<ast::ExprCall> {
        if !call
            .arguments
            .keywords
            .iter()
            .any(|kw| kw.arg.is_none() && kw.value.is_dict_expr())
        {
            return None;
        }
        let mut changed = false;
        let mut keywords = Vec::new();
        for kw in &call.arguments.keywords {
            if kw.arg.is_none()
                && let ast::Expr::Dict(dict) = &kw.value
                && let Some(names) = keys(dict)
            {
                keywords.extend(
                    names
                        .iter()
                        .zip(&dict.items)
                        .map(|(name, item)| keyword(name, item.value.clone(), item.value.range())),
                );
                changed = true;
            } else {
                keywords.push(kw.clone());
            }
        }
        changed.then(|| {
            let mut result = call.clone();
            result.arguments.keywords = keywords.into();
            result
        })
    }
}

/// A literal keyword display can carry a wider value into an annotated callee.
pub(super) fn has_keyword_displays(graph: &SourceGraph) -> bool {
    #[derive(Default)]
    struct Displays(bool);
    impl<'a> Visitor<'a> for Displays {
        fn visit_keyword(&mut self, kw: &'a ast::Keyword) {
            if kw.arg.is_none() && kw.value.is_dict_expr() {
                self.0 = true;
            } else if !self.0 {
                walk_keyword(self, kw);
            }
        }
    }
    graph.modules.iter().any(|module| {
        module.source.contains("**") && {
            let mut found = Displays::default();
            module.parsed.syntax().body.iter().any(|stmt| {
                found.visit_stmt(stmt);
                found.0
            })
        }
    })
}

const KEYWORD_PACK: &str = "@keyword-tuple:";

fn packed_name(n: &mut ast::ExprName) {
    n.id = format!("{KEYWORD_PACK}{}", n.id).into();
}

/// Only the projections inserted for a proven fresh keyword mapping may carry
/// their observed dynamic argument type into inference.
pub(super) fn keyword_field(expr: &ast::Expr) -> bool {
    matches!(expr, ast::Expr::Subscript(s) if matches!(&*s.value,
        ast::Expr::Name(n) if n.id.as_str().starts_with(KEYWORD_PACK)))
}

/// Specialize the continuation of an exhaustive match that binds a function
/// and its arguments together. Only one arm or its continuation runs. Keeping
/// their native locals separate preserves correlations a tuple of unions loses.
pub(super) fn prepare(
    function: &mut ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
) -> bool {
    dispatch::specialize(function, graph, module);
    keywords::scalarize(function, graph, module)
}

#[derive(Default)]
struct Mentions(HashSet<String>, bool);
impl<'a> Visitor<'a> for Mentions {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        self.1 |= matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_));
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        self.1 |= matches!(
            expr,
            ast::Expr::Lambda(_)
                | ast::Expr::Generator(_)
                | ast::Expr::ListComp(_)
                | ast::Expr::SetComp(_)
                | ast::Expr::DictComp(_)
        );
        if let ast::Expr::Name(n) = expr {
            self.0.insert(n.id.to_string());
        }
        walk_expr(self, expr);
    }
}
