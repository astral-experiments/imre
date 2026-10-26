//! Ownership can leave a private accumulator when its only reader is one forward loop.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

struct Candidate {
    init: (usize, TextRange),
    consume: Option<(usize, TextRange)>,
    appended: bool,
    escaped: bool,
}

fn initialized(stmt: &ast::Stmt) -> Option<&ast::ExprName> {
    let (target, value) = match stmt {
        ast::Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                return None;
            };
            (target, &*assign.value)
        }
        ast::Stmt::AnnAssign(assign) => (&*assign.target, assign.value.as_deref()?),
        _ => return None,
    };
    if let ast::Expr::Name(name) = target
        && matches!(value, ast::Expr::List(items) if items.elts.is_empty())
    {
        Some(name)
    } else {
        None
    }
}

struct Uses<'a> {
    names: HashMap<&'a str, Candidate>,
    statement: usize,
    indirect: bool,
}

impl<'ast> Visitor<'ast> for Uses<'_> {
    fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
        // Nested scopes, imports and pattern/exception bindings can name locals
        // without containing a Name expression. Keep their ordinary iteration.
        if matches!(
            stmt,
            ast::Stmt::FunctionDef(_)
                | ast::Stmt::ClassDef(_)
                | ast::Stmt::Try(_)
                | ast::Stmt::Match(_)
                | ast::Stmt::Global(_)
                | ast::Stmt::Nonlocal(_)
                | ast::Stmt::Import(_)
                | ast::Stmt::ImportFrom(_)
                | ast::Stmt::TypeAlias(_)
        ) {
            self.indirect = true;
            return;
        }
        // Only a standalone append before the consuming statement is allowed.
        // An append captured by a generator, for example, could run later.
        if let ast::Stmt::Expr(stmt) = stmt
            && let ast::Expr::Call(call) = &*stmt.value
            && let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "append"
            && let ast::Expr::Name(receiver) = &*method.value
            && let Some(use_) = self.names.get_mut(receiver.id.as_str())
            && use_.init.0 < self.statement
            && use_.consume.is_some_and(|(end, _)| self.statement < end)
            && call.arguments.keywords.is_empty()
            && let [arg] = call.arguments.args.as_ref()
        {
            use_.appended = true;
            self.visit_expr(arg);
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast ast::Expr) {
        if matches!(expr, ast::Expr::Lambda(_)) {
            self.indirect = true;
        } else if let ast::Expr::Name(name) = expr
            && let Some(use_) = self.names.get_mut(name.id.as_str())
            && expr.range() != use_.init.1
            && !use_.consume.is_some_and(|(_, range)| range == expr.range())
        {
            use_.escaped = true;
        } else {
            walk_expr(self, expr);
        }
    }
}

/// The initializer and sole consuming loop are top-level statements: no
/// backedge can restart its cursor over slots whose ownership was transferred.
pub(super) fn find(body: &[ast::Stmt]) -> HashSet<TextRange> {
    let mut uses = Uses {
        names: HashMap::new(),
        statement: 0,
        indirect: false,
    };
    for (index, stmt) in body.iter().enumerate() {
        if let Some(name) = initialized(stmt) {
            uses.names.entry(name.id.as_str()).or_insert(Candidate {
                init: (index, name.range()),
                consume: None,
                appended: false,
                escaped: false,
            });
        }
        if let ast::Stmt::For(for_) = stmt {
            let sources = match &*for_.iter {
                source @ ast::Expr::Name(_) => std::slice::from_ref(source),
                ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(n) if n.id.as_str() == "zip") => {
                    call.arguments.args.as_ref()
                }
                _ => continue,
            };
            for source in sources {
                if let ast::Expr::Name(name) = source
                    && let Some(use_) = uses.names.get_mut(name.id.as_str())
                {
                    use_.escaped |= use_.consume.replace((index, name.range())).is_some();
                }
            }
        }
    }
    uses.names.retain(|_, c| c.consume.is_some() && !c.escaped);
    if uses.names.is_empty() {
        return HashSet::new();
    }
    for (index, stmt) in body.iter().enumerate() {
        uses.statement = index;
        uses.visit_stmt(stmt);
        if uses.indirect {
            return HashSet::new();
        }
    }
    uses.names
        .into_values()
        .filter(|c| c.appended && !c.escaped)
        .filter_map(|c| c.consume.map(|(_, range)| range))
        .collect()
}

impl Checker<'_> {
    /// Only sealed element kinds with unobservable destruction can relinquish
    /// the list's reference early. First-party records can own streams, tasks,
    /// bound callables and other resources indirectly, so retain them normally.
    pub(super) fn consumes_list(&self, ty: Type, range: TextRange) -> bool {
        self.consuming_lists.contains(&range)
            && matches!(ty, Type::List(_))
            && matches!(
                self.types.element(ty),
                Some(Type::Str | Type::Bytes | Type::Path | Type::Patch(_))
            )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_private_once_consumed_objects_transfer_their_owner() {
        let text = include_str!("../../../../tests/fixtures/containers/consuming_lists.py");
        let code = crate::compile(text, &crate::CompileOptions::default()).unwrap();
        let bodies = code
            .split("define internal ")
            .filter(|f| {
                let body = f.split_once("\n}").unwrap_or((f, "")).0;
                body.contains("call i64 @imre_seq_take_index_proven(")
            })
            .count();
        // direct's single traversal and paired's zip can transfer owned values.
        // Aliasing, a surrounding backedge, two consumers and exceptional uses cannot.
        assert_eq!(bodies, 2);
    }

    #[test]
    fn finalizable_fields_keep_their_original_lifetime() {
        let code = crate::compile(
            r#"
from typing import TextIO

class Handle:
    file: TextIO

    def __init__(self, file: TextIO) -> None:
        self.file = file

def observe(items: list[Handle]) -> int:
    private: list[Handle] = []
    for item in items:
        private.append(item)
    count = 0
    for retained in private:
        count += 1
    return count

items: list[Handle] = []
print(observe(items))
"#,
            &crate::CompileOptions::default(),
        )
        .unwrap();
        assert!(!code.contains("call i64 @imre_seq_take_index_proven("));
    }
}
