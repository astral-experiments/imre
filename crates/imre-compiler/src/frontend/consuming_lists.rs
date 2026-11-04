//! Ownership can leave a private accumulator when its only reader is one forward loop.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

struct Candidate {
    init: (usize, TextRange),
    consume: Option<(usize, TextRange)>,
    split: bool,
    sorted: bool,
    appended: bool,
    escaped: bool,
}

/// Accumulator lists stay at the top level. A split may also capture text
/// inside a context manager before the consuming loop.
fn record<'a>(
    stmt: &'a ast::Stmt,
    index: usize,
    nested: bool,
    names: &mut HashMap<&'a str, Candidate>,
) {
    let (target, value, plain) = match stmt {
        ast::Stmt::With(with) => {
            for stmt in &with.body {
                record(stmt, index, true, names);
            }
            return;
        }
        ast::Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                return;
            };
            (target, &*assign.value, true)
        }
        ast::Stmt::AnnAssign(assign) => {
            let Some(value) = assign.value.as_deref() else {
                return;
            };
            (&*assign.target, value, false)
        }
        _ => return,
    };
    let ast::Expr::Name(name) = target else {
        return;
    };
    let split = plain
        && matches!(value, ast::Expr::Call(call)
        if matches!(&*call.func, ast::Expr::Attribute(attr) if attr.attr.as_str() == "split"));
    if split || (!nested && matches!(value, ast::Expr::List(items) if items.elts.is_empty())) {
        names.entry(name.id.as_str()).or_insert(Candidate {
            init: (index, name.range()),
            consume: None,
            split,
            sorted: false,
            appended: false,
            escaped: false,
        });
    }
}

struct Uses<'a> {
    names: HashMap<&'a str, Candidate>,
    statement: usize,
    indirect: bool,
    shadows_sorted: bool,
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
        self.shadows_sorted |= matches!(expr, ast::Expr::Name(name) if name.id.as_str() == "sorted" && name.ctx != ast::ExprContext::Load);

        if matches!(expr, ast::Expr::Lambda(_))
            || matches!(expr, ast::Expr::Name(n) if matches!(n.id.as_str(), "locals" | "vars" | "eval" | "exec"))
            || matches!(expr, ast::Expr::Attribute(a) if matches!(a.attr.as_str(), "_getframe" | "currentframe" | "f_locals"))
        {
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

#[derive(Default)]
pub(super) struct Plan {
    takes: HashSet<TextRange>,
    split_sites: HashMap<TextRange, TextRange>,
    pub(super) split_consumers: HashMap<TextRange, u8>,
}

impl Plan {
    /// Preserve evaluation at the original assignment and activate the loop
    /// only after the checker identifies the built-in, unlimited string split.
    pub(super) fn capture_split(&mut self, target: TextRange, value: Expr) -> Expr {
        if let Some(&consumer) = self.split_sites.get(&target)
            && let Some((receiver, byte)) = pipelines::byte_split(&value)
        {
            self.split_consumers.insert(consumer, byte);
            receiver.clone()
        } else {
            value
        }
    }
}

/// A top-level consuming loop has no enclosing backedge that could reuse its input.
pub(super) fn find(body: &[ast::Stmt], is_sorted: impl Fn(&ast::Expr) -> bool) -> Plan {
    let mut uses = Uses {
        names: HashMap::new(),
        statement: 0,
        indirect: false,
        shadows_sorted: false,
    };
    for (index, stmt) in body.iter().enumerate() {
        record(stmt, index, false, &mut uses.names);
        if let ast::Stmt::For(for_) = stmt {
            let (sources, sorted) = match &*for_.iter {
                source @ ast::Expr::Name(_) => (std::slice::from_ref(source), false),
                ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(n) if n.id.as_str()=="zip") => {
                    (call.arguments.args.as_ref(), false)
                }
                ast::Expr::Call(call)
                    if is_sorted(&call.func)
                        && call.arguments.keywords.is_empty()
                        && call.arguments.args.len() == 1 =>
                {
                    (call.arguments.args.as_ref(), true)
                }
                _ => continue,
            };
            for source in sources {
                if let ast::Expr::Name(name) = source
                    && let Some(use_) = uses.names.get_mut(name.id.as_str())
                    && if sorted {
                        use_.split
                    } else {
                        !use_.split || matches!(&*for_.iter, ast::Expr::Name(_))
                    }
                {
                    use_.sorted = sorted;
                    use_.escaped |= use_.consume.replace((index, name.range())).is_some();
                }
            }
        }
    }
    uses.names.retain(|_, c| c.consume.is_some() && !c.escaped);
    if uses.names.is_empty() {
        return Plan::default();
    }
    for (index, stmt) in body.iter().enumerate() {
        uses.statement = index;
        uses.visit_stmt(stmt);
        if uses.indirect {
            return Plan::default();
        }
    }
    let mut plan = Plan::default();
    for c in uses.names.into_values().filter(|c| !c.escaped) {
        if let Some((_, at)) = c.consume {
            if c.split && !c.appended && !(c.sorted && uses.shadows_sorted) {
                plan.split_sites.insert(c.init.1, at);
            } else if !c.split && c.appended {
                plan.takes.insert(at);
            }
        }
    }
    plan
}

impl Checker<'_> {
    /// Only sealed element kinds with unobservable destruction can relinquish
    /// the list's reference early. First-party records can own streams, tasks,
    /// bound callables and other resources indirectly, so retain them normally.
    pub(super) fn consumes_list(&self, ty: Type, range: TextRange) -> bool {
        self.consuming_lists.takes.contains(&range)
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
    fn sorted_split_only_streams_private_fields_in_lexical_order() {
        let source = include_str!("../../../../tests/fixtures/containers/sorted_split_fields.py");
        let code = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(
            code.matches("call ptr @imre_str_sorted_split_byte_iter(")
                .count(),
            3
        );
        assert_eq!(code.matches("call ptr @imre_str_split(").count(), 4);
    }

    #[test]
    fn only_a_private_single_pass_split_streams_its_fields() {
        let source = include_str!("../../../../tests/fixtures/containers/assigned_split_fields.py");
        let code = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert_eq!(
            code.matches("call ptr @imre_str_split_byte_iter(").count(),
            2
        );
        assert_eq!(code.matches("call ptr @imre_str_split(").count(), 3);
    }

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
