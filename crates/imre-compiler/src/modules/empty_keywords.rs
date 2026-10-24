//! Prove a fresh local keyword mapping is used only for call expansion.

use super::{ModuleId, SourceGraph};
use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_keyword, walk_stmt};
use ruff_text_size::{Ranged, TextSize};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct EmptyKeywords {
    /// Fresh displays and the variable reads that expand their contents.
    positions: HashSet<(ModuleId, TextSize)>,
}

impl EmptyKeywords {
    pub(crate) fn contains(&self, module: ModuleId, expr: &impl Ranged) -> bool {
        self.positions.contains(&(module, expr.start()))
    }

    pub(super) fn collect(graph: &SourceGraph) -> Self {
        let mut result = Self::default();
        for (module, source) in graph.modules.iter().enumerate() {
            for stmt in &source.parsed.syntax().body {
                let ast::Stmt::FunctionDef(f) = stmt else {
                    continue;
                };
                let mut scan = Uses {
                    graph,
                    module,
                    names: HashMap::new(),
                    escaped: HashSet::new(),
                };
                for p in f.parameters.iter_non_variadic_params() {
                    scan.escaped.insert(p.name().to_string());
                }
                for statement in &f.body {
                    scan.visit_stmt(statement);
                }
                for (name, (displays, expansions)) in scan.names {
                    if displays.len() == 1
                        && !expansions.is_empty()
                        && !scan.escaped.contains(&name)
                    {
                        result.positions.insert((module, displays[0]));
                        result
                            .positions
                            .extend(expansions.into_iter().map(|start| (module, start)));
                    }
                }
            }
        }
        result
    }
}

struct Uses<'a> {
    graph: &'a SourceGraph,
    module: ModuleId,
    names: HashMap<String, (Vec<TextSize>, Vec<TextSize>)>,
    escaped: HashSet<String>,
}

impl<'a> Uses<'_> {
    fn assignment(&mut self, target: &'a ast::Expr, value: &'a ast::Expr) {
        match (target, value) {
            (ast::Expr::Name(n), ast::Expr::Dict(d)) if d.items.is_empty() => {
                self.names
                    .entry(n.id.to_string())
                    .or_default()
                    .0
                    .push(d.start());
            }
            (ast::Expr::Tuple(t), ast::Expr::Tuple(v)) if t.elts.len() == v.elts.len() => {
                for (target, value) in t.elts.iter().zip(&v.elts) {
                    self.assignment(target, value);
                }
            }
            _ => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
        }
    }
}

impl<'a> Visitor<'a> for Uses<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if self
            .graph
            .entry_profile
            .ranges(self.module)
            .any(|r| r.contains_range(stmt.range()))
        {
            return;
        }
        if matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
            struct Captures<'b>(&'b mut HashSet<String>);
            impl<'a> Visitor<'a> for Captures<'_> {
                fn visit_expr(&mut self, expr: &'a ast::Expr) {
                    if let ast::Expr::Name(n) = expr {
                        self.0.insert(n.id.to_string());
                    }
                    walk_expr(self, expr);
                }
            }
            Captures(&mut self.escaped).visit_stmt(stmt);
            return;
        }
        if let ast::Stmt::Assign(a) = stmt
            && let [target] = a.targets.as_slice()
        {
            self.assignment(target, &a.value);
        } else {
            walk_stmt(self, stmt);
        }
    }
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let ast::Expr::Name(n) = expr {
            self.escaped.insert(n.id.to_string());
        }
        walk_expr(self, expr);
    }
    fn visit_keyword(&mut self, keyword: &'a ast::Keyword) {
        if keyword.arg.is_none()
            && let ast::Expr::Name(n) = &keyword.value
        {
            self.names
                .entry(n.id.to_string())
                .or_default()
                .1
                .push(n.start());
        } else {
            walk_keyword(self, keyword);
        }
    }
}
