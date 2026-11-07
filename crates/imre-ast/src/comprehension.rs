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

impl<'a> Plan<'a> {
    pub fn from_expr(expr: &'a ast::Expr) -> Option<Self> {
        let (output, clauses) = Output::from_expr(expr)?;
        Some(Self::new(clauses, output))
    }

    pub fn generator(expr: &'a ast::ExprGenerator) -> Self {
        Self::new(&expr.generators, Output::Generator(&expr.elt))
    }

    pub fn new(clauses: &'a [ast::Comprehension], output: Output<'a>) -> Self {
        let outer = clauses
            .first()
            .expect("comprehension has an outer iterable");
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
        let mut names = HashSet::new();
        for step in &self.steps {
            if let Step::Next { target, .. } = step {
                collect_target_names(target, &mut names);
            }
        }
        names
    }

    /// Visit the expressions in one successful pass through the body. The
    /// acquisition expression belongs to the enclosing lexical scope.
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

/// Recognize explicit awaits and implicit asynchronous pulls in this scope.
/// Nested lazy bodies begin executing only when their generator is advanced.
pub fn expression_suspends(expression: &ast::Expr) -> bool {
    let mut scan = Suspension::default();
    scan.visit_expr(expression);
    scan.any()
}

/// Only names in a binding pattern become local. Attribute and subscript
/// assignment evaluate their receiver in the comprehension scope.
pub fn collect_target_names(target: &ast::Expr, names: &mut HashSet<String>) {
    let mut pending = vec![target];
    while let Some(target) = pending.pop() {
        match target {
            ast::Expr::Name(name) => {
                names.insert(name.id.to_string());
            }
            ast::Expr::Starred(star) => pending.push(&star.value),
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => pending.extend(elts),
            _ => {}
        }
    }
}

#[derive(Default)]
struct Suspension<'a> {
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
                ast::Expr::Lambda(lambda) => {
                    if let Some(params) = &lambda.parameters {
                        self.visit_parameters(params);
                    }
                }
                ast::Expr::Generator(generator) => self.visit_expr(&generator.generators[0].iter),
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
