//! Scalar replacement for fresh, nonescaping keyword dictionaries.

use super::*;

/// A fresh dictionary used only as **kwargs is a fixed tuple of evaluated
/// values. Preserve its construction point even when later assignments rebind
/// the original value names.
#[derive(Default)]
struct KeywordUses {
    assigned: HashMap<String, Vec<String>>,
    expanded: HashSet<String>,
    escaped: HashSet<String>,
    excluded: Vec<TextRange>,
}

impl KeywordUses {
    fn assign(&mut self, target: &ast::Expr, value: &ast::Expr) {
        match (target, value) {
            (ast::Expr::Name(n), ast::Expr::Dict(d)) if let Some(keys) = keys(d) => {
                if self.assigned.insert(n.id.to_string(), keys).is_some() {
                    self.escaped.insert(n.id.to_string());
                }
                self.visit_expr(value);
            }
            (ast::Expr::Tuple(t), ast::Expr::Tuple(v)) if t.elts.len() == v.elts.len() => {
                for (t, v) in t.elts.iter().zip(&v.elts) {
                    self.assign(t, v);
                }
            }
            _ => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
        }
    }
}

impl<'a> Visitor<'a> for KeywordUses {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if matches!(
            expr,
            ast::Expr::Lambda(_)
                | ast::Expr::ListComp(_)
                | ast::Expr::SetComp(_)
                | ast::Expr::DictComp(_)
                | ast::Expr::Generator(_)
        ) {
            let mut names = Mentions::default();
            names.visit_expr(expr);
            self.escaped.extend(names.0);
            return;
        }
        if let ast::Expr::Name(n) = expr {
            self.escaped.insert(n.id.to_string());
        }
        walk_expr(self, expr);
    }
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if self.excluded.iter().any(|r| r.contains_range(stmt.range())) {
            return;
        }
        if let ast::Stmt::ClassDef(class) = stmt {
            self.escaped.insert(class.name.to_string());
        }
        if let ast::Stmt::FunctionDef(function) = stmt {
            self.escaped.insert(function.name.to_string());
        }
        if matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
            let mut names = Mentions::default();
            names.visit_stmt(stmt);
            self.escaped.extend(names.0);
            return;
        }
        if let ast::Stmt::Assign(a) = stmt
            && let [target] = a.targets.as_slice()
        {
            self.assign(target, &a.value);
        } else {
            walk_stmt(self, stmt);
        }
    }
    fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(h) = handler;
        if let Some(n) = &h.name {
            self.escaped.insert(n.to_string());
        }
        walk_except_handler(self, handler);
    }
    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        let binding = match pattern {
            ast::Pattern::MatchAs(p) => p.name.as_ref(),
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
            _ => None,
        };
        if let Some(n) = binding {
            self.escaped.insert(n.to_string());
        }
        walk_pattern(self, pattern);
    }
    fn visit_keyword(&mut self, kw: &'a ast::Keyword) {
        if kw.arg.is_none()
            && let ast::Expr::Name(n) = &kw.value
        {
            self.expanded.insert(n.id.to_string());
        } else {
            walk_keyword(self, kw);
        }
    }
}

struct KeywordTuples(HashMap<String, Vec<String>>);
impl KeywordTuples {
    fn assign(&self, target: &mut ast::Expr, value: &mut ast::Expr) {
        match (target, &mut *value) {
            (ast::Expr::Name(n), ast::Expr::Dict(d)) if self.0.contains_key(n.id.as_str()) => {
                packed_name(n);
                *value = ast::Expr::Tuple(ast::ExprTuple {
                    node_index: ast::AtomicNodeIndex::NONE,
                    range: d.range(),
                    ctx: ast::ExprContext::Load,
                    parenthesized: true,
                    elts: std::mem::take(&mut d.items)
                        .into_iter()
                        .map(|item| item.value)
                        .collect(),
                });
            }
            (ast::Expr::Tuple(t), ast::Expr::Tuple(v)) => {
                for (t, v) in t.elts.iter_mut().zip(&mut v.elts) {
                    self.assign(t, v);
                }
            }
            _ => {}
        }
    }
}
impl Transformer for KeywordTuples {
    fn visit_stmt(&self, stmt: &mut ast::Stmt) {
        if matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
            return;
        }
        if let ast::Stmt::Assign(a) = stmt
            && let [target] = a.targets.as_mut_slice()
        {
            self.assign(target, &mut a.value);
        }
        transformer::walk_stmt(self, stmt);
    }
    fn visit_expr(&self, expr: &mut ast::Expr) {
        if let ast::Expr::Call(call) = expr
            && call.arguments.keywords.iter().any(|k| k.arg.is_none())
        {
            let mut result = Vec::new();
            for kw in std::mem::take(&mut call.arguments.keywords) {
                if kw.arg.is_none()
                    && let ast::Expr::Name(n) = &kw.value
                    && let Some(names) = self.0.get(n.id.as_str())
                {
                    let mut n = n.clone();
                    packed_name(&mut n);
                    let reference = ast::Expr::Name(n);
                    if names.is_empty() {
                        // Keep a real use of the empty pack: if its assignment did
                        // not dominate this call, ordinary definite-assignment
                        // checks must still reject the uninitialized read.
                        call.arguments
                            .args
                            .push(ast::Expr::Starred(ast::ExprStarred {
                                node_index: ast::AtomicNodeIndex::NONE,
                                range: kw.range,
                                value: Box::new(reference),
                                ctx: ast::ExprContext::Load,
                            }));
                    } else {
                        result.extend(names.iter().enumerate().map(|(i, name)| {
                            keyword(name, project(reference.clone(), i), kw.range)
                        }));
                    }
                } else {
                    result.push(kw);
                }
            }
            call.arguments.keywords = result.into();
        }
        transformer::walk_expr(self, expr);
    }
}

pub(super) fn scalarize(
    function: &mut ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
) -> bool {
    let mut scan = KeywordUses {
        excluded: graph.entry_profile.ranges(module).collect(),
        ..Default::default()
    };
    for p in parameters(&function.parameters) {
        scan.escaped.insert(p.name.to_string());
    }
    scan.visit_body(&function.body);
    scan.assigned
        .retain(|name, _| scan.expanded.contains(name) && !scan.escaped.contains(name));
    if scan.assigned.is_empty() {
        return false;
    }
    KeywordTuples(scan.assigned).visit_body(&mut function.body);
    true
}
