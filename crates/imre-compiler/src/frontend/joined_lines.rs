//! A string list used only to assemble a final join does not expose its item boundaries.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

fn name(expr: &ast::Expr) -> Option<&str> {
    if let ast::Expr::Name(n) = expr {
        Some(n.id.as_str())
    } else {
        None
    }
}

fn skip_header(expr: &ast::Expr) -> Option<&str> {
    let ast::Expr::Subscript(index) = expr else {
        return None;
    };
    let ast::Expr::Slice(slice) = &*index.slice else {
        return None;
    };
    let ast::Expr::NumberLiteral(n) = slice.lower.as_deref()? else {
        return None;
    };
    (slice.upper.is_none()
        && slice.step.is_none()
        && matches!(&n.value, ast::Number::Int(i) if i.as_u64() == Some(1)))
    .then(|| name(&index.value))
    .flatten()
}

fn binding(stmt: &ast::Stmt) -> Option<(&str, &ast::Expr)> {
    match stmt {
        ast::Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                return None;
            };
            Some((name(target)?, &assign.value))
        }
        ast::Stmt::AnnAssign(assign) => Some((name(&assign.target)?, assign.value.as_deref()?)),
        _ => None,
    }
}

struct Uses<'a> {
    out: &'a str,
    escapes: bool,
    initialized: bool,
    writes: HashSet<TextRange>,
    sources: HashMap<String, TextRange>,
    consumed: HashSet<String>,
    observed: HashSet<String>,
}

impl<'ast> Visitor<'ast> for Uses<'_> {
    fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
        // Nested scopes and pattern/exception target names have different binding rules.
        if matches!(
            stmt,
            ast::Stmt::FunctionDef(_)
                | ast::Stmt::ClassDef(_)
                | ast::Stmt::Try(_)
                | ast::Stmt::Match(_)
                | ast::Stmt::Import(_)
                | ast::Stmt::ImportFrom(_)
                | ast::Stmt::TypeAlias(_)
                | ast::Stmt::Global(_)
                | ast::Stmt::Nonlocal(_)
        ) {
            self.escapes = true;
            return;
        }
        if let Some((var, expr @ ast::Expr::Call(call))) = binding(stmt)
            && self.initialized
            && var != self.out
            && call.arguments.is_empty()
            && matches!(&*call.func, ast::Expr::Attribute(attr) if attr.attr.as_str() == "splitlines")
        {
            if self.sources.insert(var.to_owned(), call.range()).is_some() {
                self.observed.insert(var.to_owned());
            }
            self.visit_expr(expr);
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast ast::Expr) {
        if matches!(expr, ast::Expr::Lambda(_))
            || matches!(expr, ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(n) if matches!(n.id.as_str(), "locals" | "vars" | "eval" | "exec")))
        {
            self.escapes = true;
            return;
        }
        if self.initialized
            && let ast::Expr::Call(call) = expr
            && let ast::Expr::Attribute(method) = &*call.func
            && name(&method.value) == Some(self.out)
            && matches!(method.attr.as_str(), "append" | "extend")
            && call.arguments.keywords.is_empty()
            && let [arg] = call.arguments.args.as_ref()
        {
            self.writes.insert(call.range());
            if method.attr.as_str() == "extend"
                && let Some(var) = skip_header(arg)
            {
                if var == self.out {
                    self.escapes = true;
                }
                self.consumed.insert(var.to_owned());
            } else {
                self.visit_expr(arg);
            }
            return;
        }
        if let Some(var) = name(expr) {
            self.escapes |= var == self.out;
            self.observed.insert(var.to_owned());
        }
        walk_expr(self, expr);
    }
}

#[derive(Default)]
pub(super) struct Plan {
    pub(super) splits: HashMap<TextRange, String>,
    buffer: Option<Buffer>,
    active: bool,
}

struct Buffer {
    init: TextRange,
    finish: TextRange,
    writes: HashSet<TextRange>,
    separator: String,
}

impl Plan {
    /// Enable the recorded writes only after the checker confirms this exact list[str] initializer.
    pub(super) fn start_buffer(&mut self, at: TextRange) -> Option<String> {
        let buffer = self.buffer.as_ref().filter(|b| b.init == at)?;
        self.active = true;
        Some(buffer.separator.clone())
    }

    pub(super) fn writes_buffer(&self, at: TextRange) -> bool {
        self.active && self.buffer.as_ref().is_some_and(|b| b.writes.contains(&at))
    }

    pub(super) fn finishes_buffer(&self, at: TextRange) -> bool {
        self.active && self.buffer.as_ref().is_some_and(|b| b.finish == at)
    }
}

/// A split can group its tail only if every use of that local extends the same
/// private output by exactly [1:]. The only read of the output is the final
/// literal join. Any escape or alternative binding keeps the ordinary lists.
pub(super) fn find(stmts: &[ast::Stmt]) -> Plan {
    let Some((ast::Stmt::Return(ret), prefix)) = stmts.split_last() else {
        return Plan::default();
    };
    let Some(ast::Expr::Call(call)) = ret.value.as_deref() else {
        return Plan::default();
    };
    let ast::Expr::Attribute(method) = &*call.func else {
        return Plan::default();
    };
    let ast::Expr::StringLiteral(separator) = &*method.value else {
        return Plan::default();
    };
    let [arg] = call.arguments.args.as_ref() else {
        return Plan::default();
    };
    let Some(out) = name(arg) else {
        return Plan::default();
    };
    if method.attr.as_str() != "join" || !call.arguments.keywords.is_empty() {
        return Plan::default();
    }
    let Some(init) = prefix.iter().position(|stmt|
        matches!(binding(stmt), Some((var, ast::Expr::List(list))) if var == out && list.elts.is_empty())
    ) else { return Plan::default() };
    let mut scan = Uses {
        out,
        escapes: false,
        initialized: false,
        writes: HashSet::new(),
        sources: HashMap::new(),
        consumed: HashSet::new(),
        observed: HashSet::new(),
    };
    scan.visit_body(&prefix[..init]);
    scan.initialized = true;
    scan.visit_body(&prefix[init + 1..]);
    if scan.escapes {
        return Plan::default();
    }
    let separator = separator.value.to_str();
    // The checker enables these sites only after confirming the initializer is an
    // exact list[str]. All of the collector's other uses are writes to this instance.
    let buffer = match &prefix[init] {
        ast::Stmt::AnnAssign(assignment) if !scan.writes.is_empty() => Some(Buffer {
            init: assignment.value.as_ref().unwrap().range(),
            finish: call.range(),
            writes: scan.writes,
            separator: separator.to_owned(),
        }),
        _ => None,
    };
    let splits = scan
        .sources
        .into_iter()
        .filter(|(var, _)| scan.consumed.contains(var) && !scan.observed.contains(var))
        .map(|(_, range)| (range, separator.to_owned()))
        .collect();
    Plan {
        splits,
        buffer,
        active: false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_the_unobserved_collector_can_transfer_its_joined_string() {
        let text = include_str!("../../../../tests/fixtures/containers/joined_strings.py");
        let llvm = crate::compile(text, &crate::CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call ptr @imre_join_buffer_new(").count(), 1);
        assert_eq!(
            llvm.matches("call ptr @imre_join_buffer_finish(").count(),
            1
        );
        // A direct append, a generator extend, and a list extend keep their order.
        assert_eq!(
            llvm.matches("call i32 @imre_join_buffer_append(").count(),
            3
        );
    }

    #[test]
    fn only_private_joined_outputs_can_coalesce_boundaries() {
        let llvm = crate::compile(
            include_str!("../../../../tests/fixtures/containers/joined_splitlines.py"),
            &crate::CompileOptions::default(),
        )
        .unwrap();
        // Two independent split producers in join_two, one in lines_only, none
        // in the alias or rebinding cases that observe individual lines.
        assert_eq!(
            llvm.matches("call ptr @imre_splitlines_joined_tail(")
                .count(),
            3
        );
    }
}
