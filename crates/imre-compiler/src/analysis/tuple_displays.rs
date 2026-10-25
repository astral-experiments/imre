//! Prove fixed display widths before ty can materialize an exponentially wide tuple.

use std::collections::HashMap;

use ruff_python_ast::visitor::source_order::{self, SourceOrderVisitor};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{self as ast, Expr, ExprContext, Stmt};
use ruff_text_size::{Ranged, TextSize};

const MAX_FIELDS: usize = 256;
const UNPROVEN: &str = "starred elements in tuple displays require a bounded, unmodified sequence";
type Widths = HashMap<TextSize, usize>;

/// A fresh list or tuple has as many fields as its literal. An unfiltered
/// comprehension over such a literal produces exactly one item per input.
fn fresh_width(expr: &Expr) -> Option<usize> {
    let elements = match expr {
        Expr::List(seq) => &seq.elts,
        Expr::Tuple(seq) => &seq.elts,
        Expr::ListComp(comp) => {
            let [clause] = comp.generators.as_slice() else {
                return None;
            };
            if clause.is_async
                || !clause.ifs.is_empty()
                || !matches!(&clause.iter, Expr::List(_) | Expr::Tuple(_))
            {
                return None;
            }
            return fresh_width(&clause.iter);
        }
        _ => return None,
    };
    (!elements.iter().any(|e| matches!(e, Expr::Starred(_)))).then_some(elements.len())
}

/// Recognize an explicitly fixed tuple parameter; the frontend independently
/// checks the concrete operand layout against this bound.
fn parameter_width(annotation: &Expr) -> Option<usize> {
    let Expr::Subscript(sub) = annotation else {
        return None;
    };
    if !matches!(&*sub.value, Expr::Name(n) if n.id == "tuple") {
        return None;
    }
    let fields = match &*sub.slice {
        Expr::Tuple(seq) => seq.elts.as_slice(),
        field => std::slice::from_ref(field),
    };
    fields
        .iter()
        .all(|e| !matches!(e, Expr::EllipsisLiteral(_) | Expr::Starred(_)))
        .then_some(fields.len())
}

struct Binding {
    declaration: TextSize,
    width: usize,
    spreads: Vec<TextSize>,
}

struct Uses<'a> {
    bindings: HashMap<&'a str, Binding>,
    nested: bool,
}

impl<'a> SourceOrderVisitor<'a> for Uses<'a> {
    fn visit_alias(&mut self, alias: &'a ast::Alias) {
        if alias.asname.is_none() {
            self.bindings
                .remove(alias.name.as_str().split('.').next().unwrap());
        }
        source_order::walk_alias(self, alias);
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        let previous = self.nested;
        self.nested |= matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_));
        source_order::walk_stmt(self, stmt);
        self.nested = previous;
    }

    fn visit_identifier(&mut self, id: &'a ast::Identifier) {
        // Includes handler targets, captures, global/nonlocal declarations,
        // nested definitions and imports that can replace a binding.
        self.bindings.remove(id.as_str());
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Tuple(tuple) if tuple.ctx == ExprContext::Load && !self.nested => {
                for field in &tuple.elts {
                    if let Expr::Starred(star) = field
                        && let Expr::Name(name) = &*star.value
                        && let Some(binding) = self.bindings.get_mut(name.id.as_str())
                        && binding.declaration < name.start()
                    {
                        binding.spreads.push(star.start());
                    } else {
                        self.visit_expr(field);
                    }
                }
            }
            Expr::Name(name) => {
                if self
                    .bindings
                    .get(name.id.as_str())
                    .is_some_and(|b| b.declaration != name.start())
                {
                    // An ordinary read can publish a mutable list; a store or
                    // deletion invalidates the width even for an immutable tuple.
                    self.bindings.remove(name.id.as_str());
                }
            }
            _ => source_order::walk_expr(self, expr),
        }
    }
}

/// Only accept a local that is freshly bound at function entry or by an
/// unconditional assignment and whose every other use is a tuple spread.
fn local_widths(function: &ast::StmtFunctionDef, widths: &mut Widths) {
    let mut bindings = HashMap::new();
    for param in function.parameters.iter_non_variadic_params() {
        let param = &param.parameter;
        if let Some(width) = param.annotation.as_deref().and_then(parameter_width) {
            bindings.insert(
                param.name.as_str(),
                Binding {
                    declaration: param.name.start(),
                    width,
                    spreads: Vec::new(),
                },
            );
        }
    }
    for stmt in &function.body {
        let (target, value) = match stmt {
            Stmt::Assign(a) if a.targets.len() == 1 => (&a.targets[0], &*a.value),
            Stmt::AnnAssign(a) if a.value.is_some() => (&*a.target, a.value.as_deref().unwrap()),
            _ => continue,
        };
        if let Expr::Name(name) = target
            && let Some(width) = fresh_width(value)
        {
            bindings.insert(
                name.id.as_str(),
                Binding {
                    declaration: name.start(),
                    width,
                    spreads: Vec::new(),
                },
            );
        }
    }
    if bindings.is_empty() {
        return;
    }
    let mut uses = Uses {
        bindings,
        nested: false,
    };
    uses.visit_body(&function.body);
    for binding in uses.bindings.into_values() {
        widths.extend(
            binding
                .spreads
                .into_iter()
                .map(|site| (site, binding.width)),
        );
    }
}

#[derive(Default)]
struct Displays<'a> {
    widths: Widths,
    function: bool,
    function_body: Option<TextSize>,
    error: Option<(&'a ast::ExprStarred, &'static str)>,
}

impl<'a> Visitor<'a> for Displays<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        let previous = (self.function, self.function_body);
        if let Stmt::FunctionDef(function) = stmt {
            local_widths(function, &mut self.widths);
            self.function = false;
            self.function_body = function.body.first().map(Ranged::start);
        } else if matches!(stmt, Stmt::ClassDef(_)) {
            self.function = false;
        }
        walk_stmt(self, stmt);
        (self.function, self.function_body) = previous;
    }

    fn visit_body(&mut self, body: &'a [Stmt]) {
        if body
            .first()
            .is_some_and(|stmt| Some(stmt.start()) == self.function_body)
        {
            self.function = true;
        }
        ruff_python_ast::visitor::walk_body(self, body);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.error.is_some() {
            return;
        }
        if let Expr::Tuple(tuple) = expr
            && tuple.ctx == ExprContext::Load
        {
            let mut total = tuple
                .elts
                .iter()
                .filter(|e| !matches!(e, Expr::Starred(_)))
                .count();
            for element in &tuple.elts {
                if let Expr::Starred(star) = element {
                    if !self.function {
                        self.error = Some((
                            star,
                            "starred elements in tuple displays require a function body",
                        ));
                        return;
                    }
                    let width = fresh_width(&star.value)
                        .or_else(|| self.widths.get(&star.start()).copied());
                    let Some(width) = width else {
                        self.error = Some((star, UNPROVEN));
                        return;
                    };
                    total = total.saturating_add(width);
                    if total > MAX_FIELDS {
                        self.error =
                            Some((star, "starred elements in tuple displays exceed 256 fields"));
                        return;
                    }
                    self.widths.insert(star.start(), width);
                }
            }
        }
        walk_expr(self, expr);
    }
}

/// Keep unrelated source on the fast path and record a width for every accepted spread.
pub(super) fn check(body: &[Stmt]) -> Result<Widths, (&ast::ExprStarred, &'static str)> {
    if !ast::helpers::any_over_body(body, |e| {
        matches!(e, Expr::Tuple(t) if t.ctx == ExprContext::Load
        && t.elts.iter().any(|e| matches!(e, Expr::Starred(_))))
    }) {
        return Ok(Widths::new());
    }
    let mut displays = Displays::default();
    displays.visit_body(body);
    displays.error.map_or(Ok(displays.widths), Err)
}

#[cfg(test)]
mod tests {
    #[test]
    fn mutable_or_variable_width_sources_do_not_acquire_a_fixed_tuple_layout() {
        for body in [
            "ys = xs\n    ys.append(3)",
            "xs.append(3)",
            "if cond:\n        xs = []",
            "def change() -> None:\n        xs.pop()\n    change()",
            "xs = [x for x in [1, 2] if cond]",
        ] {
            let source = format!(
                "def f(cond: bool) -> int:\n    xs = [1, 2]\n    {body}\n    y = (*xs,)\n    return len(y)\n"
            );
            let error = crate::check(&source).unwrap_err();
            assert!(
                error.contains("starred elements in tuple displays"),
                "{error}"
            );
        }
    }
}
