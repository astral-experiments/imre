//! Runtime assignment flags for explicitly typed locals whose initializer may raise.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_except_handler, walk_expr, walk_stmt};

/// Only ordinary one-name stores participate. Other binding forms keep the
/// existing definite-assignment requirement; we must never miss a write or a
/// deletion that could change the value observed by a guarded load.
pub(super) fn candidates(statements: &[ast::Stmt], interactive: bool) -> HashSet<String> {
    #[derive(Default)]
    struct Scan {
        inside: bool,
        interactive: bool,
        guarded: HashSet<String>,
        other: HashSet<String>,
    }
    impl<'a> Visitor<'a> for Scan {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::Assign(s) if matches!(s.targets.as_slice(), [ast::Expr::Name(_)]) => {
                    if self.interactive && self.inside {
                        self.guarded
                            .insert(s.targets[0].as_name_expr().unwrap().id.to_string());
                    }
                    self.visit_expr(&s.value);
                }
                ast::Stmt::AnnAssign(s)
                    if s.target.as_name_expr().is_some() && s.value.is_some() =>
                {
                    if self.inside {
                        self.guarded
                            .insert(s.target.as_name_expr().unwrap().id.to_string());
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
        ..Scan::default()
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
        let (kind, message) = if self.interactive_names {
            ("NameError", format!("name '{name}' is not defined"))
        } else {
            (
                "UnboundLocalError",
                format!(
                    "cannot access local variable '{name}' where it is not associated with a value"
                ),
            )
        };
        let failure = Statement::Raise(Some(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind(kind).unwrap(),
                message: Some(Box::new(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(message),
                })),
            },
        }));
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
