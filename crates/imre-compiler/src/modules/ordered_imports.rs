//! Preserve the source binding behind statically resolved import names during initialization.
use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

pub(super) struct Bindings(Vec<HashMap<String, usize>>);

impl Bindings {
    pub(super) fn new(modules: &[SourceModule]) -> Self {
        Self(
            modules
                .iter()
                .map(|module| {
                    let mut names = HashMap::new();
                    for (index, stmt) in module.parsed.syntax().body.iter().enumerate() {
                        let (aliases, dotted) = match stmt {
                            ast::Stmt::Import(import) => (&import.names, true),
                            ast::Stmt::ImportFrom(import) => (&import.names, false),
                            _ => continue,
                        };
                        for alias in aliases {
                            let spelling = alias.name.as_str();
                            let name = if let Some(alias) = &alias.asname {
                                alias.as_str()
                            } else if dotted {
                                spelling.split('.').next().unwrap()
                            } else {
                                spelling
                            };
                            names.entry(name.to_owned()).or_insert(index);
                        }
                    }
                    names
                })
                .collect(),
        )
    }
}

impl SourceGraph {
    pub(crate) fn has_ordered_imports(&self) -> bool {
        self.ordered_bindings.is_some()
    }

    /// Add name dependencies that disappear when closed imports or class references are folded.
    pub(crate) fn ordered_reads(
        &self,
        owner: ModuleId,
        body: &[ast::Stmt],
        function: bool,
        out: &mut HashSet<DefinitionId>,
    ) {
        if let Some(bindings) = &self.ordered_bindings {
            Reads {
                graph: self,
                bindings,
                owner,
                function,
                out,
            }
            .visit_body(body);
        }
    }
}

struct Reads<'a> {
    graph: &'a SourceGraph,
    bindings: &'a Bindings,
    owner: ModuleId,
    function: bool,
    out: &'a mut HashSet<DefinitionId>,
}

impl Reads<'_> {
    fn name(&mut self, module: ModuleId, name: &str) {
        if let Some(&statement) = self.bindings.0[module].get(name) {
            self.out.insert(DefinitionId { module, statement });
        }
        match self.graph.modules[module].bindings.get(name) {
            Some(Binding::Definition(def)) => {
                self.out.insert(*def);
            }
            Some(Binding::Module(child))
                if !self.bindings.0[module].contains_key(name)
                    && self.graph.modules[*child].name.rsplit_once('.')
                        == Some((self.graph.modules[module].name.as_str(), name)) =>
            {
                // A qualified package.child lookup requires publication on the parent.
                // A local alias obtained from the sys.modules fallback has no such requirement.
                self.out.insert(DefinitionId {
                    module: *child,
                    statement: self.graph.modules[*child].parsed.syntax().body.len(),
                });
            }
            _ => {}
        }
    }
}

impl<'ast> Visitor<'ast> for Reads<'_> {
    fn visit_expr(&mut self, expr: &'ast ast::Expr) {
        match expr {
            ast::Expr::Name(name) if name.ctx == ast::ExprContext::Load => {
                self.name(self.owner, name.id.as_str())
            }
            ast::Expr::Attribute(attr) if attr.ctx == ast::ExprContext::Load => {
                if let Some(Binding::Module(target)) = self.graph.resolve(self.owner, &attr.value) {
                    self.name(target, attr.attr.as_str());
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }

    fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
        match stmt {
            ast::Stmt::If(guard)
                if !self.function
                    && guard.elif_else_clauses.is_empty()
                    && self.graph.resolve(self.owner, &guard.test)
                        == Some(Binding::Intrinsic(Intrinsic::TypeChecking)) =>
            {
                // Postponed type imports do not execute the guarded body.
                self.visit_expr(&guard.test);
            }
            ast::Stmt::FunctionDef(def) if !self.function => {
                for decorator in &def.decorator_list {
                    self.visit_expr(&decorator.expression);
                }
                for param in def.parameters.iter_non_variadic_params() {
                    if let Some(default) = param.default() {
                        self.visit_expr(default);
                    }
                }
            }
            ast::Stmt::AnnAssign(assign) if !self.function => {
                if let Some(value) = &assign.value {
                    self.visit_expr(value);
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }
}
