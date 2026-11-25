//! Runtime assignment flags for locals whose stores may not execute or may raise.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_except_handler, walk_expr, walk_stmt};

fn nullable_return(checker: &Checker<'_>, rhs: &ast::Expr) -> Option<Type> {
    let call = rhs.as_call_expr()?;
    let crate::modules::Binding::Definition(id) = checker.resolve(&call.func)? else {
        return None;
    };
    let ty = checker.functions.get(&id)?.returns;
    checker.types.payload(ty).map(|_| ty)
}

/// A plain-store cache must not remove the runtime flags required by conditional
/// loop stores. Nested function and class bodies belong to another scope.
fn contains_loop(statements: &[ast::Stmt]) -> bool {
    #[derive(Default)]
    struct Scan(bool);
    impl<'a> Visitor<'a> for Scan {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::For(_) | ast::Stmt::While(_) => self.0 = true,
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {}
                _ if !self.0 => walk_stmt(self, stmt),
                _ => {}
            }
        }

        fn visit_expr(&mut self, _expr: &'a ast::Expr) {}
    }
    let mut scan = Scan::default();
    scan.visit_body(statements);
    scan.0
}

/// Collect guarded local stores and the optional layouts needed by earlier assignments.
pub(super) fn local_assignments(
    checker: &Checker<'_>,
    statements: &[ast::Stmt],
    function: Option<&ast::StmtFunctionDef>,
) -> (HashSet<String>, HashMap<String, Type>) {
    if let Some(function) = function {
        let mut nullable = HashMap::new();
        let plain =
            checker
                .graph
                .session
                .plain_function_stores(checker.module, function, |name, rhs| {
                    if checker.interactive_names || checker.mapping_class_import(name) {
                        return None;
                    }
                    if let Some(ty) = nullable_return(checker, rhs)
                        && nullable
                            .insert(name.to_owned(), ty)
                            .is_some_and(|prior| prior != ty)
                    {
                        // Differing layouts follow the original visitor's assignment order.
                        return None;
                    }
                    Some(())
                });
        if plain.is_some() && !contains_loop(statements) {
            return (HashSet::new(), nullable);
        }
    }

    struct Scan<'a, 'b> {
        checker: &'a Checker<'b>,
        inside: bool,
        inside_loop: bool,
        inside_match: bool,
        guarded: HashSet<String>,
        other: HashSet<String>,
        nullable: HashMap<String, Type>,
    }
    impl<'a> Visitor<'a> for Scan<'_, '_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::Assign(s) if matches!(s.targets.as_slice(), [ast::Expr::Name(_)]) => {
                    let name = &s.targets[0].as_name_expr().unwrap().id;
                    if let Some(ty) = nullable_return(self.checker, &s.value) {
                        self.nullable.insert(name.to_string(), ty);
                    }
                    if !self.inside_match
                        && (self.inside_loop
                            || (self.checker.interactive_names && self.inside)
                            || self.checker.mapping_class_import(name.as_str()))
                    {
                        self.guarded.insert(name.to_string());
                    }
                    self.visit_expr(&s.value);
                }
                _ if self.inside_match => {
                    if !matches!(stmt, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_)) {
                        walk_stmt(self, stmt);
                    }
                }
                ast::Stmt::AnnAssign(s)
                    if s.target.as_name_expr().is_some() && s.value.is_some() =>
                {
                    let name = &s.target.as_name_expr().unwrap().id;
                    if self.inside
                        || self.inside_loop
                        || self.checker.mapping_class_import(name.as_str())
                    {
                        self.guarded.insert(name.to_string());
                    }
                    self.visit_expr(s.value.as_deref().unwrap());
                }
                ast::Stmt::Try(s) => {
                    let before = self.inside;
                    self.inside |= !s.handlers.is_empty();
                    self.visit_body(&s.body);
                    self.inside = before;
                    for handler in &s.handlers {
                        self.visit_except_handler(handler);
                    }
                    self.visit_body(&s.orelse);
                    self.visit_body(&s.finalbody);
                }
                ast::Stmt::For(_) | ast::Stmt::While(_) => {
                    let before = self.inside_loop;
                    self.inside_loop = true;
                    walk_stmt(self, stmt);
                    self.inside_loop = before;
                }
                ast::Stmt::FunctionDef(f) => {
                    self.other.insert(f.name.to_string());
                }
                ast::Stmt::ClassDef(c) => {
                    self.other.insert(c.name.to_string());
                }
                ast::Stmt::Match(matched) => {
                    self.other
                        .extend(narrowing::assigned_names(std::slice::from_ref(stmt)));
                    // Matches exclude their writes from the guarded locals. Their
                    // assignments can still reserve an optional layout for an earlier store.
                    self.inside_match = true;
                    for case in &matched.cases {
                        self.visit_body(&case.body);
                    }
                    self.inside_match = false;
                }
                _ => walk_stmt(self, stmt),
            }
        }

        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if self.inside_match {
                return;
            }
            if let ast::Expr::Name(n) = expr
                && n.ctx != ast::ExprContext::Load
            {
                self.other.insert(n.id.to_string());
            }
            walk_expr(self, expr);
        }

        fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            if !self.inside_match
                && let Some(name) = &h.name
            {
                self.other.insert(name.to_string());
            }
            walk_except_handler(self, handler);
        }
    }
    let mut scan = Scan {
        checker,
        inside: false,
        inside_loop: false,
        inside_match: false,
        guarded: HashSet::new(),
        other: HashSet::new(),
        nullable: HashMap::new(),
    };
    scan.visit_body(statements);
    scan.guarded.retain(|n| !scan.other.contains(n));
    (scan.guarded, scan.nullable)
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn conditional_loop_store_uses_a_checked_native_read() {
        let llvm = compile(
            "def find(values: list[int]) -> int:\n    for value in values:\n        if value > 0:\n            found = value\n            break\n    return found\n",
            &CompileOptions::default(),
        )
        .unwrap();
        let encoded =
            "cannot access local variable 'found' where it is not associated with a value"
                .bytes()
                .map(|byte| format!("\\{byte:02X}"))
                .collect::<String>();
        assert!(llvm.contains(&encoded));
        assert!(llvm.contains("call void @imre_exception_raise("));
        assert!(!llvm.contains("@imre_dyn_register"));
    }
}

/// The flag is set after the RHS and store succeed; a failure leaves the previous
/// binding untouched. In particular, an assigned Python None is not an unbound local.
pub(super) fn assigned(target: &ast::Stmt) -> Option<&str> {
    match target {
        ast::Stmt::Assign(s) => match s.targets.as_slice() {
            [ast::Expr::Name(n)] => Some(n.id.as_str()),
            _ => None,
        },
        ast::Stmt::AnnAssign(s) if s.value.is_some() => {
            s.target.as_name_expr().map(|n| n.id.as_str())
        }
        _ => None,
    }
}

impl Checker<'_> {
    pub(super) fn checked_bound_read(&self, slot: usize, flag: usize, name: &str) -> Expr {
        let value = self.read_local(slot);
        let failure = unbound_failure(
            name,
            if self.interactive_names {
                Missing::Interactive
            } else {
                Missing::Local
            },
        );
        Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::If(self.local(flag), vec![], vec![failure])],
                value: Box::new(value),
                clear: vec![],
            },
        }
    }
}

#[derive(Copy, Clone)]
pub(super) struct Binding {
    hint: Type,
    may_store: bool,
}

/// Assignment expressions in omitted assertions still bind in the enclosing function.
/// A comprehension binds a walrus target there too; a lambda body has its own scope.
pub(super) fn omitted_assert_bindings(
    body: &[ast::Stmt],
) -> (HashMap<String, Binding>, HashSet<String>) {
    #[derive(Default)]
    struct Bindings {
        names: HashMap<String, Binding>,
        live: HashSet<String>,
        external: HashSet<String>,
        inside: bool,
    }
    impl<'a> Visitor<'a> for Bindings {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::FunctionDef(f) => {
                    self.live.insert(f.name.to_string());
                }
                ast::Stmt::ClassDef(c) => {
                    self.live.insert(c.name.to_string());
                }
                ast::Stmt::Global(g) => self
                    .external
                    .extend(g.names.iter().map(ToString::to_string)),
                ast::Stmt::Nonlocal(g) => self
                    .external
                    .extend(g.names.iter().map(ToString::to_string)),
                ast::Stmt::Assert(_) => {
                    self.inside = true;
                    walk_stmt(self, stmt);
                    self.inside = false;
                }
                _ => walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if !self.inside {
                if let ast::Expr::Name(name) = expr
                    && name.ctx != ast::ExprContext::Load
                {
                    self.live.insert(name.id.to_string());
                }
                if !matches!(expr, ast::Expr::Lambda(_)) {
                    walk_expr(self, expr);
                }
                return;
            }
            if let ast::Expr::Lambda(lambda) = expr {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
                return;
            }
            if let ast::Expr::Named(named) = expr
                && let Some(name) = named.target.as_name_expr()
            {
                let hint = match &*named.value {
                    ast::Expr::BooleanLiteral(_) => Type::Bool,
                    ast::Expr::NumberLiteral(n) => match n.value {
                        ast::Number::Int(_) => Type::Int,
                        ast::Number::Float(_) => Type::Float,
                        _ => Type::None,
                    },
                    ast::Expr::StringLiteral(_) => Type::Str,
                    ast::Expr::BytesLiteral(_) => Type::Bytes,
                    _ => Type::None,
                };
                self.names
                    .entry(name.id.to_string())
                    .and_modify(|b| {
                        if b.hint != hint {
                            b.hint = Type::None;
                        }
                    })
                    .or_insert(Binding {
                        hint,
                        may_store: false,
                    });
            }
            walk_expr(self, expr);
        }
    }
    let mut scan = Bindings::default();
    scan.visit_body(body);
    scan.names.retain(|name, b| {
        b.may_store = scan.live.contains(name);
        !scan.external.contains(name)
    });
    (scan.names, scan.live)
}

enum Missing {
    Local,
    Interactive,
    Free,
}
fn unbound_failure(name: &str, missing: Missing) -> Statement {
    let (kind, message) = match missing {
        Missing::Interactive => ("NameError", format!("name '{name}' is not defined")),
        Missing::Local => (
            "UnboundLocalError",
            format!(
                "cannot access local variable '{name}' where it is not associated with a value"
            ),
        ),
        Missing::Free => (
            "NameError",
            format!(
                "cannot access free variable '{name}' where it is not associated with a value in enclosing scope"
            ),
        ),
    };
    Statement::raise(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind(kind).unwrap(),
            message: Some(Box::new(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(message),
            })),
        },
    })
}

impl Checker<'_> {
    /// Read the first operand of a call, subscription or attribute before any arguments.
    pub(super) fn unbound_assert_root<'a>(&self, expr: &'a ast::Expr) -> Option<&'a str> {
        if self.omitted_assert_locals.is_empty() && self.omitted_assert_free.is_empty() {
            return None;
        }
        let mut root = expr;
        loop {
            root = match root {
                ast::Expr::Name(name) => {
                    let name = name.id.as_str();
                    return ((self.omitted_assert_locals.contains_key(name)
                        || self.omitted_assert_free.contains_key(name))
                        && !self.names.contains_key(name))
                    .then_some(name);
                }
                ast::Expr::Attribute(a) => &a.value,
                ast::Expr::Call(c) => &c.func,
                ast::Expr::Subscript(s) => &s.value,
                _ => return None,
            };
        }
    }

    pub(super) fn unbound_assert_captures(&self) -> HashMap<String, Binding> {
        let mut inherited = self.omitted_assert_free.clone();
        inherited.extend(
            self.omitted_assert_locals
                .iter()
                .map(|(name, b)| (name.clone(), *b)),
        );
        inherited.retain(|name, _| !self.names.contains_key(name));
        inherited
    }
    pub(super) fn omitted_assert_read(
        &mut self,
        name: &str,
        expected: Option<Type>,
        node: &ast::Expr,
    ) -> Result<Expr, String> {
        let (binding, missing) = if let Some(b) = self.omitted_assert_locals.get(name) {
            (b, Missing::Local)
        } else {
            let b = &self.omitted_assert_free[name];
            if b.may_store {
                return Err(error(
                    self.source,
                    node,
                    "captured local may be initialized after this definition",
                ));
            }
            (b, Missing::Free)
        };
        let ty = expected.unwrap_or(binding.hint);
        let unreachable_slot = self.hidden(ty);
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![unbound_failure(name, missing)],
                value: Box::new(self.local(unreachable_slot)),
                clear: vec![],
            },
        })
    }
}
