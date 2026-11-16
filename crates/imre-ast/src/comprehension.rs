//! Python comprehension control flow, independent of iterator and value representation.
//!
//! The caller acquires the outer iterator in the enclosing scope. Clauses then
//! execute with every target local to a single comprehension scope. A generator
//! retains that scope and its active iterators between emissions.

use std::collections::HashSet;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{self, Visitor};

/// Ruff parses PEP 798 comprehensions, but CPython 3.14 rejects a double star
/// here before considering the enclosing expression's symbol-table errors.
pub fn dict_unpacking_error<T>(
    parsed: &ruff_python_parser::Parsed<T>,
) -> Option<(ruff_text_size::TextRange, &'static str)> {
    use ruff_python_ast::token::TokenKind;
    use ruff_text_size::Ranged;

    parsed.unsupported_syntax_errors().iter().find_map(|error| {
        if !matches!(
            error.kind,
            ruff_python_parser::UnsupportedSyntaxErrorKind::UnpackingInComprehension(_)
        ) {
            return None;
        }
        let star = parsed.tokens().in_range(error.range).first()?;
        (star.kind() == TokenKind::DoubleStar).then(|| {
            (
                star.range(),
                "dict unpacking cannot be used in dict comprehension",
            )
        })
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Output<'a> {
    List(&'a ast::Expr),
    Set(&'a ast::Expr),
    Dict(&'a ast::Expr, &'a ast::Expr),
    Generator(&'a ast::Expr),
}

impl<'a> Output<'a> {
    /// Read the result and clauses of a comprehension expression.
    pub fn from_expr(expr: &'a ast::Expr) -> Option<(Self, &'a [ast::Comprehension])> {
        Some(match expr {
            ast::Expr::ListComp(c) => (Self::List(&c.elt), &c.generators),
            ast::Expr::SetComp(c) => (Self::Set(&c.elt), &c.generators),
            ast::Expr::DictComp(c) => (Self::Dict(c.key.as_deref()?, &c.value), &c.generators),
            ast::Expr::Generator(c) => (Self::Generator(&c.elt), &c.generators),
            _ => return None,
        })
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::List(_) => "list",
            Self::Set(_) => "set",
            Self::Dict(..) => "dict",
            Self::Generator(_) => "generator",
        }
    }

    pub fn key(self) -> Option<&'a ast::Expr> {
        match self {
            Self::Dict(key, _) => Some(key),
            _ => None,
        }
    }

    pub fn value(self) -> &'a ast::Expr {
        match self {
            Self::List(value)
            | Self::Set(value)
            | Self::Dict(_, value)
            | Self::Generator(value) => value,
        }
    }

    /// Insertion evaluates the key before the value for a dictionary display.
    pub fn elements(self) -> impl Iterator<Item = &'a ast::Expr> {
        self.key().into_iter().chain(std::iter::once(self.value()))
    }
}

/// An operand evaluated inside the comprehension, including its assignment targets.
#[derive(Clone, Copy)]
pub enum BodyOperand<'a> {
    Target(&'a ast::Expr),
    Value(&'a ast::Expr),
}

pub struct Plan<'a> {
    pub outer: &'a ast::Expr,
    pub clauses: &'a [ast::Comprehension],
    pub output: Output<'a>,
}

/// The only clause whose iterable expression is evaluated by the caller.
pub fn outer_clause(clauses: &[ast::Comprehension]) -> &ast::Comprehension {
    clauses
        .first()
        .expect("comprehension has an outer iterable")
}

impl<'a> Plan<'a> {
    pub fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let (output, clauses) = Output::from_expr(expr)?;
        Some(Self::new(clauses, output))
    }

    pub fn generator(expr: &'a ast::ExprGenerator) -> Self {
        Self::new(&expr.generators, Output::Generator(&expr.elt))
    }

    pub fn new(clauses: &'a [ast::Comprehension], output: Output<'a>) -> Self {
        let outer = outer_clause(clauses);
        Self {
            outer: &outer.iter,
            clauses,
            output,
        }
    }

    pub fn target_names(&self) -> HashSet<String> {
        self.collect_target_names()
    }

    /// A target in any clause shadows this name throughout the private scope.
    /// The first iterable still resolves the name in the enclosing scope.
    pub fn binds_name(&self, name: &str) -> bool {
        self.clauses.iter().any(|clause| {
            let mut bound = false;
            visit_target_leaves(
                &clause.target,
                |_| Ok(()),
                |leaf| {
                    bound |= matches!(leaf, ast::Expr::Name(target) if target.id.as_str() == name);
                },
            )
            .expect("collecting names without validation");
            bound
        })
    }

    /// Collect binding names in clause and unpacking order using the caller's collection.
    pub fn collect_target_names<T: Default + Extend<String>>(&self) -> T {
        self.target_bindings(|_| {})
    }

    /// Collect declarations and visit the expressions evaluated to store into their targets.
    pub fn target_bindings<T: Default + Extend<String>>(
        &self,
        visit: impl FnMut(&'a ast::Expr),
    ) -> T {
        self.target_names_with(|_| Ok(()), visit)
            .expect("collecting names without validation")
    }

    pub fn try_target_names(&self) -> Result<HashSet<String>, InvalidTarget> {
        self.target_names_with(Err, |_| {})
    }

    /// Declare the targets of all pulls, including an inner clause that may
    /// never run. Each consumer chooses when to report assignment syntax errors.
    fn target_names_with<T: Default + Extend<String>>(
        &self,
        invalid: impl Fn(InvalidTarget) -> Result<(), InvalidTarget>,
        mut visit: impl FnMut(&'a ast::Expr),
    ) -> Result<T, InvalidTarget> {
        let mut names = T::default();
        for clause in self.clauses {
            visit_target(&clause.target, &mut names, &invalid, &mut visit)?;
        }
        Ok(names)
    }

    /// The enclosing scope evaluates the outer iterable. The body opens each
    /// subsequent iterable, binds its target, tests its filters and evaluates the output.
    pub fn body_operands(&self) -> impl Iterator<Item = BodyOperand<'a>> {
        self.clauses
            .iter()
            .enumerate()
            .flat_map(|(depth, clause)| {
                (depth > 0)
                    .then_some(BodyOperand::Value(&clause.iter))
                    .into_iter()
                    .chain(std::iter::once(BodyOperand::Target(&clause.target)))
                    .chain(clause.ifs.iter().map(BodyOperand::Value))
            })
            .chain(self.output.elements().map(BodyOperand::Value))
    }

    pub fn visit_body(&self, mut visit: impl FnMut(&'a ast::Expr)) {
        for operand in self.body_operands() {
            match operand {
                BodyOperand::Value(expr) | BodyOperand::Target(expr) => visit(expr),
            }
        }
    }

    /// Collect the private scope's bindings and visit its reads in symbol-table order.
    pub fn scope_bindings<T: Default + Extend<String>>(
        &self,
        visit: impl FnMut(&'a ast::Expr),
    ) -> T {
        struct Reads<F>(F);
        impl<'a, F: FnMut(&'a ast::Expr)> Visitor<'a> for Reads<F> {
            fn visit_expr(&mut self, expr: &'a ast::Expr) {
                (self.0)(expr);
            }
        }

        let mut names = T::default();
        self.visit_symbol_body(
            &mut Reads(visit),
            |reads, target| {
                visit_target(target, &mut names, |_| Ok(()), &mut reads.0)
                    .expect("collecting names without validation");
            },
            Reads::visit_expr,
        );
        names
    }

    /// Visit the comprehension's private scope in symbol-table order. The enclosing
    /// scope already visited the first iterable. An inner target precedes its
    /// iterable, and a dict value precedes its key when selecting a symbol error.
    pub(super) fn visit_symbol_body<V: Visitor<'a>>(
        &self,
        visitor: &mut V,
        mut target: impl FnMut(&mut V, &'a ast::Expr),
        iterable: impl Fn(&mut V, &'a ast::Expr),
    ) {
        for (depth, clause) in self.clauses.iter().enumerate() {
            target(visitor, &clause.target);
            if depth > 0 {
                iterable(visitor, &clause.iter);
            }
            for test in &clause.ifs {
                visitor.visit_expr(test);
            }
        }
        visitor.visit_expr(self.output.value());
        if let Some(key) = self.output.key() {
            visitor.visit_expr(key);
        }
    }

    /// Includes implicit awaits in async iteration, and excludes the acquisition
    /// expression which runs in the caller when constructing a generator.
    pub fn body_suspends(&self) -> bool {
        if self.clauses.iter().any(|clause| clause.is_async) {
            return true;
        }
        let mut scan = Suspension::default();
        self.visit_body(|expr| scan.visit_expr(expr));
        scan.any()
    }

    /// Whether evaluation of the expression itself can suspend its enclosing frame.
    pub fn suspends(&self) -> bool {
        expression_suspends(self.outer)
            || (!matches!(self.output, Output::Generator(_)) && self.body_suspends())
    }
}

/// Recognize yield, await and implicit asynchronous pulls in this scope.
/// Nested lazy bodies begin executing only when their generator is advanced.
pub fn expression_suspends(expression: &ast::Expr) -> bool {
    let mut scan = Suspension {
        include_yield: true,
        ..Suspension::default()
    };
    scan.visit_expr(expression);
    scan.any()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidTarget {
    BareStar,
    MultipleStars,
    Unsupported,
}

/// Only names in a binding pattern become local. Analysis can collect names
/// before its owning compiler reports a malformed assignment pattern.
pub fn collect_target_names(target: &ast::Expr, names: &mut HashSet<String>) {
    let _ = visit_target(target, names, |_| Ok(()), |_| {});
}

/// Declare bindings left to right. Attribute and subscript receivers are
/// expressions, and only an unpacking pattern may contain a starred target.
pub fn try_collect_target_names(
    target: &ast::Expr,
    names: &mut impl Extend<String>,
) -> Result<(), InvalidTarget> {
    visit_target(target, names, Err, |_| {})
}

fn visit_target<'a>(
    target: &'a ast::Expr,
    names: &mut impl Extend<String>,
    invalid: impl Fn(InvalidTarget) -> Result<(), InvalidTarget>,
    mut visit: impl FnMut(&'a ast::Expr),
) -> Result<(), InvalidTarget> {
    visit_target_leaves(target, invalid, |leaf| match leaf {
        ast::Expr::Name(name) => names.extend([name.id.to_string()]),
        expr => visit(expr),
    })
}

/// Visit binding names and evaluated receivers in the order in which they occur in a target.
pub(super) fn visit_target_leaves<'a>(
    mut target: &'a ast::Expr,
    invalid: impl Fn(InvalidTarget) -> Result<(), InvalidTarget>,
    mut visit: impl FnMut(&'a ast::Expr),
) -> Result<(), InvalidTarget> {
    let mut pending = Vec::new();
    loop {
        match target {
            ast::Expr::Name(_) | ast::Expr::Attribute(_) | ast::Expr::Subscript(_) => visit(target),
            ast::Expr::Starred(star) => {
                invalid(InvalidTarget::BareStar)?;
                target = &star.value;
                continue;
            }
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => {
                if elts
                    .iter()
                    .filter(|elt| elt.is_starred_expr())
                    .nth(1)
                    .is_some()
                {
                    invalid(InvalidTarget::MultipleStars)?;
                }
                pending.extend(elts.iter().rev().map(|item| match item {
                    ast::Expr::Starred(star) => star.value.as_ref(),
                    item => item,
                }));
            }
            _ => {
                invalid(InvalidTarget::Unsupported)?;
                visit(target);
            }
        }
        let Some(next) = pending.pop() else {
            return Ok(());
        };
        target = next;
    }
}

#[derive(Default)]
struct Suspension<'a> {
    include_yield: bool,
    pending: Vec<&'a ast::Expr>,
}

impl<'a> Visitor<'a> for Suspension<'a> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        self.pending.push(expr);
    }
}

impl Suspension<'_> {
    fn any(&mut self) -> bool {
        while let Some(expr) = self.pending.pop() {
            match expr {
                ast::Expr::Await(_) => return true,
                ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) if self.include_yield => return true,
                ast::Expr::Lambda(lambda) => {
                    if let Some(params) = &lambda.parameters {
                        self.visit_parameters(params);
                    }
                }
                ast::Expr::Generator(generator) => {
                    self.visit_expr(&outer_clause(&generator.generators).iter)
                }
                expr => {
                    if Output::from_expr(expr)
                        .is_some_and(|(_, clauses)| clauses.iter().any(|c| c.is_async))
                    {
                        return true;
                    }
                    visitor::walk_expr(self, expr);
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_clauses_keep_their_filters_and_targets_in_source_order() {
        let text = "(y for x in xs if x if f(x) for y in x if y)";
        let expression = ruff_python_parser::parse_expression(text).unwrap();
        let plan = Plan::from_expr(expression.expr()).unwrap();
        use ruff_text_size::Ranged;
        let observed = plan
            .body_operands()
            .map(|operand| match operand {
                BodyOperand::Target(expr) => ("bind", &text[expr.range()]),
                BodyOperand::Value(expr) => ("read", &text[expr.range()]),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            observed,
            [
                ("bind", "x"),
                ("read", "x"),
                ("read", "f(x)"),
                ("read", "x"),
                ("bind", "y"),
                ("read", "y"),
                ("read", "y")
            ]
        );
    }

    #[test]
    fn body_operands_exclude_outer_acquisition_but_preserve_later_reads() {
        use ast::ExprContext::{Load, Store};

        for (source, predicate, output) in [
            (
                "(child for item in source if ready for child in children)",
                "ready",
                "child",
            ),
            (
                "(source for item in source if source for child in children)",
                "source",
                "source",
            ),
        ] {
            let parsed = ruff_python_parser::parse_expression(source).unwrap();
            let plan = Plan::from_expr(parsed.expr()).unwrap();
            let outer = plan.outer.as_name_expr().unwrap();
            assert_eq!((outer.id.as_str(), outer.ctx), ("source", Load));

            let mut body = Vec::new();
            plan.visit_body(|operand| {
                assert!(!std::ptr::eq(operand, plan.outer));
                let name = operand.as_name_expr().unwrap();
                body.push((name.id.as_str(), name.ctx));
            });
            assert_eq!(
                body,
                [
                    ("item", Store),
                    (predicate, Load),
                    ("children", Load),
                    ("child", Store),
                    (output, Load),
                ]
            );
        }
    }

    #[test]
    fn target_scopes_declare_unpack_names_in_clause_order() {
        use ruff_text_size::Ranged;

        let text = "[item async for (first, *rest), [items[index], box.value] in rows if first for item, first in rest]";
        let parsed = ruff_python_parser::parse_expression(text).unwrap();
        let plan = Plan::from_expr(parsed.expr()).unwrap();
        let expected = HashSet::from(["first".to_owned(), "rest".to_owned(), "item".to_owned()]);
        for name in ["first", "rest", "item"] {
            assert!(plan.binds_name(name));
        }
        for name in ["items", "index", "box", "value", "rows"] {
            assert!(!plan.binds_name(name));
        }
        assert_eq!(plan.target_names(), expected);
        assert_eq!(plan.try_target_names(), Ok(expected));
        assert_eq!(
            plan.collect_target_names::<Vec<_>>(),
            ["first", "rest", "item", "first"]
        );
        let mut reads = Vec::new();
        let names: Vec<_> = plan.target_bindings(|expr| reads.push(&text[expr.range()]));
        assert_eq!(names, ["first", "rest", "item", "first"]);
        assert_eq!(reads, ["items[index]", "box.value"]);
        reads.clear();
        let names: Vec<_> = plan.scope_bindings(|expr| reads.push(&text[expr.range()]));
        assert_eq!(names, ["first", "rest", "item", "first"]);
        assert_eq!(
            reads,
            ["items[index]", "box.value", "first", "rest", "item"]
        );

        let pattern = ruff_python_parser::parse_expression("(a, [b, *c], d)").unwrap();
        let mut declared = Vec::new();
        try_collect_target_names(pattern.expr(), &mut declared).unwrap();
        assert_eq!(declared, ["a", "b", "c", "d"]);

        let pattern = ruff_python_parser::parse_expression("(*a, *b)").unwrap();
        let mut names = HashSet::new();
        assert_eq!(
            try_collect_target_names(pattern.expr(), &mut names),
            Err(InvalidTarget::MultipleStars)
        );
        let ast::Expr::Tuple(tuple) = pattern.expr() else {
            unreachable!()
        };
        assert_eq!(
            try_collect_target_names(&tuple.elts[0], &mut names),
            Err(InvalidTarget::BareStar)
        );
        collect_target_names(pattern.expr(), &mut names);
        assert_eq!(names, HashSet::from(["a".to_owned(), "b".to_owned()]));
    }

    #[test]
    fn deferred_body_and_enclosing_operand_suspend_independently() {
        for (source, body, enclosing) in [
            ("(x async for x in source)", true, false),
            ("(x for x in await source())", false, true),
            ("[x async for x in source]", true, true),
            ("[(y async for y in x) for x in source]", false, false),
            ("[[y async for y in x] for x in source]", true, true),
            ("[(y for y in await f(x)) for x in source]", true, true),
        ] {
            let parsed = ruff_python_parser::parse_expression(source).unwrap();
            let plan = Plan::from_expr(parsed.expr()).unwrap();
            assert_eq!(
                (plan.body_suspends(), plan.suspends()),
                (body, enclosing),
                "{source}"
            );
        }
    }
}
