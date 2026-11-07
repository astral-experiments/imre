//! Python comprehension control flow, independent of iterator and value representation.
//!
//! The caller acquires the outer iterator in the enclosing scope. All steps then
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
    /// Inspect syntax without building a plan, for scope and validation passes.
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

#[derive(Clone, Copy, Debug)]
pub enum Step<'a> {
    /// Evaluate an inner iterable after its enclosing target and filters.
    Open {
        depth: usize,
        iterable: &'a ast::Expr,
        is_async: bool,
    },
    /// Pull one item and bind it. Only exhaustion from the pull follows this edge.
    Next {
        depth: usize,
        target: &'a ast::Expr,
        is_async: bool,
        exhausted: usize,
    },
    Filter {
        test: &'a ast::Expr,
        reject: usize,
    },
    /// Insert or yield. The next pull resumes at the same innermost iterator.
    Emit {
        output: Output<'a>,
        resume: usize,
    },
    Close {
        depth: usize,
        next: usize,
    },
    End,
}

pub struct Plan<'a> {
    pub outer: &'a ast::Expr,
    pub outer_async: bool,
    pub output: Output<'a>,
    pub depths: usize,
    pub steps: Vec<Step<'a>>,
    /// Original clause nodes for syntax diagnostics and typed specialization.
    pub clauses: &'a [ast::Comprehension],
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
        let mut heads = Vec::with_capacity(clauses.len());
        let mut steps = Vec::new();
        for (depth, clause) in clauses.iter().enumerate() {
            if depth > 0 {
                steps.push(Step::Open {
                    depth,
                    iterable: &clause.iter,
                    is_async: clause.is_async,
                });
            }
            let head = steps.len();
            heads.push(head);
            steps.push(Step::Next {
                depth,
                target: &clause.target,
                is_async: clause.is_async,
                exhausted: 0,
            });
            steps.extend(
                clause
                    .ifs
                    .iter()
                    .map(|test| Step::Filter { test, reject: head }),
            );
        }
        steps.push(Step::Emit {
            output,
            resume: *heads.last().unwrap(),
        });
        while let Some(head) = heads.pop() {
            let close = steps.len();
            let Step::Next { exhausted, .. } = &mut steps[head] else {
                unreachable!()
            };
            *exhausted = close;
            steps.push(Step::Close {
                depth: heads.len(),
                next: heads.last().copied().unwrap_or(close + 1),
            });
        }
        steps.push(Step::End);
        Self {
            outer: &outer.iter,
            outer_async: outer.is_async,
            output,
            depths: clauses.len(),
            steps,
            clauses,
        }
    }

    /// The filters accepted by one structured loop. Rejection and emission must
    /// return to this pull, and exhaustion must close it and rejoin its caller.
    /// The step at the end of the range opens the next loop or emits its item.
    pub fn loop_filters(
        &self,
        head: usize,
        parent: Option<usize>,
    ) -> Option<std::ops::Range<usize>> {
        let Step::Next {
            depth, exhausted, ..
        } = self.steps.get(head)?
        else {
            return None;
        };
        let Step::Close {
            depth: closing,
            next,
        } = self.steps.get(*exhausted)?
        else {
            return None;
        };
        let rejoin = match self.steps.get(*next)? {
            Step::Next { .. } => Some(*next),
            Step::End => None,
            _ => return None,
        };
        if depth != closing || rejoin != parent {
            return None;
        }
        for (pc, step) in self.steps.iter().enumerate().skip(head + 1) {
            match step {
                Step::Filter { reject, .. } if *reject == head => continue,
                Step::Emit { resume, .. } if *resume == head => {}
                Step::Open {
                    depth: inner,
                    is_async,
                    ..
                } if *inner == depth + 1
                    && matches!(self.steps.get(pc + 1),
                        Some(Step::Next { depth, is_async: pull_async, .. }) if depth == inner && pull_async == is_async) =>
                    {}
                _ => return None,
            }
            return Some(head + 1..pc);
        }
        None
    }

    pub fn target_names(&self) -> HashSet<String> {
        self.collect_target_names()
    }

    /// Collect binding names in clause and unpacking order using the caller's collection.
    pub fn collect_target_names<T: Default + Extend<String>>(&self) -> T {
        self.target_names_with(|_| Ok(()))
            .expect("collecting names without validation")
    }

    pub fn try_target_names(&self) -> Result<HashSet<String>, InvalidTarget> {
        self.target_names_with(Err)
    }

    /// Declare the targets of all pulls, including an inner clause that may
    /// never run. Each consumer chooses when to report assignment syntax errors.
    fn target_names_with<T: Default + Extend<String>>(
        &self,
        invalid: impl Fn(InvalidTarget) -> Result<(), InvalidTarget>,
    ) -> Result<T, InvalidTarget> {
        let mut names = T::default();
        for step in &self.steps {
            if let Step::Next { target, .. } = step {
                visit_target_names(target, &mut names, &invalid)?;
            }
        }
        Ok(names)
    }

    /// Visit the operands of a successful body pass in execution order, including
    /// binding targets. The caller evaluates `outer` separately. A name used
    /// there may also occur in a body operand. Capture analysis resolves these
    /// occurrences against their lexical bindings.
    pub fn visit_body(&self, mut visit: impl FnMut(&'a ast::Expr)) {
        for step in &self.steps {
            match *step {
                Step::Next { target, .. } => visit(target),
                Step::Open { iterable, .. } => visit(iterable),
                Step::Filter { test, .. } => visit(test),
                Step::Emit { output, .. } => output.elements().for_each(&mut visit),
                Step::Close { .. } | Step::End => {}
            }
        }
    }

    /// Includes implicit awaits in async iteration, and excludes the acquisition
    /// expression which runs in the caller when constructing a generator.
    pub fn body_suspends(&self) -> bool {
        if self
            .steps
            .iter()
            .any(|s| matches!(s, Step::Next { is_async: true, .. }))
        {
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
    let _ = visit_target_names(target, names, |_| Ok(()));
}

/// Declare bindings left to right. Attribute and subscript receivers are
/// expressions, and only an unpacking pattern may contain a starred target.
pub fn try_collect_target_names(
    target: &ast::Expr,
    names: &mut impl Extend<String>,
) -> Result<(), InvalidTarget> {
    visit_target_names(target, names, Err)
}

fn visit_target_names(
    mut target: &ast::Expr,
    names: &mut impl Extend<String>,
    invalid: impl Fn(InvalidTarget) -> Result<(), InvalidTarget>,
) -> Result<(), InvalidTarget> {
    let mut pending = Vec::new();
    loop {
        match target {
            ast::Expr::Name(name) => names.extend([name.id.to_string()]),
            ast::Expr::Attribute(_) | ast::Expr::Subscript(_) => {}
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
            _ => invalid(InvalidTarget::Unsupported)?,
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
    fn loop_filters_follow_their_pull_and_enclosing_loop() {
        let expression =
            ruff_python_parser::parse_expression("(y for x in xs if x if f(x) for y in x if y)")
                .unwrap();
        let mut plan = Plan::from_expr(expression.expr()).unwrap();
        assert_eq!(plan.loop_filters(0, None), Some(1..3));
        assert_eq!(plan.loop_filters(4, Some(0)), Some(5..6));
        assert_eq!(plan.loop_filters(4, None), None);
        let Step::Filter { reject, .. } = &mut plan.steps[5] else {
            unreachable!()
        };
        *reject = 0;
        assert_eq!(plan.loop_filters(4, Some(0)), None);
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
        let parsed = ruff_python_parser::parse_expression(
            "[item async for (first, *rest), [items[index], box.value] in rows for item, first in rest]",
        )
        .unwrap();
        let plan = Plan::from_expr(parsed.expr()).unwrap();
        let expected = HashSet::from(["first".to_owned(), "rest".to_owned(), "item".to_owned()]);
        assert_eq!(plan.target_names(), expected);
        assert_eq!(plan.try_target_names(), Ok(expected));
        assert_eq!(
            plan.collect_target_names::<Vec<_>>(),
            ["first", "rest", "item", "first"]
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
