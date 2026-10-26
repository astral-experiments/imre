//! Runtime assignment flags for explicitly typed locals whose initializer may raise.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_except_handler, walk_expr, walk_stmt};

/// Only ordinary one-name stores participate. Other binding forms keep the
/// existing definite-assignment requirement; we must never miss a write or a
/// deletion that could change the value observed by a guarded load.
pub(super) fn candidates(
    statements: &[ast::Stmt],
    interactive: bool,
    guard_ordinary: &dyn Fn(&str) -> bool,
) -> HashSet<String> {
    struct Scan<'b> {
        guard_ordinary: &'b dyn Fn(&str) -> bool,
        inside: bool,
        interactive: bool,
        guarded: HashSet<String>,
        other: HashSet<String>,
    }
    impl<'a> Visitor<'a> for Scan<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::Assign(s) if matches!(s.targets.as_slice(), [ast::Expr::Name(_)]) => {
                    let name = &s.targets[0].as_name_expr().unwrap().id;
                    if (self.interactive && self.inside) || (self.guard_ordinary)(name.as_str()) {
                        self.guarded.insert(name.to_string());
                    }
                    self.visit_expr(&s.value);
                }
                ast::Stmt::AnnAssign(s)
                    if s.target.as_name_expr().is_some() && s.value.is_some() =>
                {
                    let name = &s.target.as_name_expr().unwrap().id;
                    if self.inside || (self.guard_ordinary)(name.as_str()) {
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
                ast::Stmt::FunctionDef(f) => {
                    self.other.insert(f.name.to_string());
                }
                ast::Stmt::ClassDef(c) => {
                    self.other.insert(c.name.to_string());
                }
                // Pattern captures are identifiers, not ordinary store expressions.
                ast::Stmt::Match(_) => self
                    .other
                    .extend(narrowing::assigned_names(std::slice::from_ref(stmt))),
                _ => walk_stmt(self, stmt),
            }
        }

        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            if let ast::Expr::Name(n) = expr
                && n.ctx != ast::ExprContext::Load
            {
                self.other.insert(n.id.to_string());
            }
            walk_expr(self, expr);
        }

        fn visit_except_handler(&mut self, handler: &'a ast::ExceptHandler) {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            if let Some(name) = &h.name {
                self.other.insert(name.to_string());
            }
            walk_except_handler(self, handler);
        }
    }
    let mut scan = Scan {
        interactive,
        guard_ordinary,
        inside: false,
        guarded: HashSet::new(),
        other: HashSet::new(),
    };
    scan.visit_body(statements);
    scan.guarded.retain(|n| !scan.other.contains(n));
    scan.guarded
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
    Statement::Raise(Some(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind(kind).unwrap(),
            message: Some(Box::new(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(message),
            })),
        },
    }))
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
