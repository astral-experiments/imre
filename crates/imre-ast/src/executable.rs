//! Binding restrictions checked when a parsed Python module becomes executable.
mod mapping_keys;

use indexmap::IndexMap;
use num_bigint::BigInt;
use ruff_python_ast::{
    self as ast,
    visitor::{self, Visitor},
};
use ruff_text_size::{Ranged, TextRange};
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Default)]
pub struct BindingErrors {
    /// Symbol-table errors precede pattern code generation.
    pub symbol: Option<(TextRange, &'static str)>,
    /// Reserved stores and deletions are checked at the selected file origin.
    pub target: Option<(TextRange, &'static str)>,
    pub pattern: Option<BindingError>,
    /// First invalid handler transfer or default clause, if it precedes the first pattern error.
    pub handler: Option<HandlerError>,
}

#[derive(Clone, Copy, Default)]
struct GroupHandler {
    local_loop: bool,
    cleanup: bool,
}

#[derive(Clone, Copy)]
pub struct HandlerError {
    /// The statement or clause used for build diagnostics.
    pub range: TextRange,
    /// Unwinding a context manager or finally suite emits an artificial instruction.
    pub location: Option<TextRange>,
    pub message: &'static str,
}

/// The identifier stored by this pattern after its children have matched.
pub fn capture_name(pattern: &ast::Pattern) -> Option<&ast::Identifier> {
    match pattern {
        ast::Pattern::MatchAs(p) => p.name.as_ref(),
        ast::Pattern::MatchStar(p) => p.name.as_ref(),
        ast::Pattern::MatchMapping(p) => p.rest.as_ref(),
        _ => None,
    }
}

type BindingError = (TextRange, PatternMessage);

#[derive(Debug)]
pub enum PatternMessage {
    Syntax(String),
    DuplicateInteger(BigInt),
}

/// Ruff retains interpolated strings as value-pattern expressions. Python only
/// accepts literal constants (including foldable complex numbers) and dotted names.
pub fn value_pattern_error(value: &ast::Expr, source: &str) -> Option<&'static str> {
    let valid = match value {
        ast::Expr::Attribute(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_) => true,
        ast::Expr::UnaryOp(op) => {
            matches!(op.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                && op.operand.is_number_literal_expr()
        }
        ast::Expr::BinOp(bin) => {
            let real = match bin.left.as_ref() {
                ast::Expr::UnaryOp(op) if op.op == ast::UnaryOp::UAdd => op.operand.as_ref(),
                other => other,
            };
            matches!(bin.op, ast::Operator::Add | ast::Operator::Sub)
                && mapping_keys::real_part(real, source).is_some()
                && matches!(
                    bin.right.as_number_literal_expr().map(|n| &n.value),
                    Some(ast::Number::Complex { .. })
                )
        }
        _ => false,
    };
    (!valid).then_some("patterns may only match literals and attribute lookups")
}

fn duplicate_key(value: impl std::fmt::Display) -> String {
    format!("mapping pattern checks duplicate key ({value})")
}

impl PatternMessage {
    // Explicit admission has no executing interpreter. Bound decimal output
    // using Python's default and still identify the duplicate at its source span.
    pub fn checked(&self) -> Cow<'_, str> {
        self.render(|i| {
            crate::numbers::decimal_fits(i, crate::numbers::DEFAULT_DIGITS)
                .then_some(())
                .ok_or(())
        })
        .unwrap_or(Cow::Borrowed(
            "mapping pattern checks duplicate integer key",
        ))
    }

    pub fn render<E>(
        &self,
        check_decimal: impl FnOnce(&BigInt) -> Result<(), E>,
    ) -> Result<Cow<'_, str>, E> {
        match self {
            Self::Syntax(s) => Ok(Cow::Borrowed(s)),
            Self::DuplicateInteger(i) => {
                check_decimal(i)?;
                Ok(Cow::Owned(duplicate_key(i)))
            }
        }
    }
}
impl From<String> for PatternMessage {
    fn from(s: String) -> Self {
        Self::Syntax(s)
    }
}
type Names<'a> = IndexMap<&'a str, &'a str>;

/// Check captures in store order, comparing private names as written even
/// when two spellings have the same storage name.
struct CapturePlan<'a> {
    spellings: Option<&'a crate::Names>,
    source: Option<&'a str>,
}

pub fn case_bindings<'a>(
    case: &'a ast::MatchCase,
    source: &'a str,
    has_next: bool,
) -> Result<impl Iterator<Item = &'a str>, BindingError> {
    let plan = CapturePlan {
        spellings: None,
        source: Some(source),
    };
    Ok(plan.case(case, has_next)?.into_keys())
}

pub fn source_case_bindings<'a>(
    case: &'a ast::MatchCase,
    has_next: bool,
    spellings: Option<&'a crate::Names>,
) -> Result<Vec<String>, BindingError> {
    let plan = CapturePlan {
        spellings,
        source: None,
    };
    Ok(plan
        .case(case, has_next)?
        .into_values()
        .map(str::to_owned)
        .collect())
}

impl<'a> CapturePlan<'a> {
    fn case(&self, case: &'a ast::MatchCase, has_next: bool) -> Result<Names<'a>, BindingError> {
        let mut names = IndexMap::new();
        self.collect(&case.pattern, case.guard.is_some() || !has_next, &mut names)?;
        Ok(names)
    }

    fn collect(
        &self,
        pattern: &'a ast::Pattern,
        allow_irrefutable: bool,
        names: &mut Names<'a>,
    ) -> Result<(), BindingError> {
        let store = |names: &mut Names<'a>, name: &'a str, storage: &'a str| {
            if names.insert(name, storage).is_none() {
                Ok(())
            } else {
                Err((
                    pattern.range(),
                    format!("multiple assignments to name '{name}' in pattern").into(),
                ))
            }
        };
        match pattern {
            ast::Pattern::MatchAs(p) => {
                if let Some(inner) = &p.pattern {
                    self.collect(inner, allow_irrefutable, names)?;
                } else if !allow_irrefutable {
                    let kind = match &p.name {
                        Some(name) => format!("name capture '{name}'"),
                        None => "wildcard".to_owned(),
                    };
                    return Err((
                        p.range(),
                        format!("{kind} makes remaining patterns unreachable").into(),
                    ));
                }
            }
            ast::Pattern::MatchOr(or) => {
                let mut first: Option<Names<'a>> = None;
                for (i, arm) in or.patterns.iter().enumerate() {
                    let mut next = Names::new();
                    self.collect(
                        arm,
                        allow_irrefutable && i + 1 == or.patterns.len(),
                        &mut next,
                    )?;
                    if let Some(expected) = &first {
                        if next.len() != expected.len()
                            || !expected.keys().all(|name| next.contains_key(name))
                        {
                            return Err((
                                or.range(),
                                "alternative patterns bind different names"
                                    .to_owned()
                                    .into(),
                            ));
                        }
                    } else {
                        first = Some(next);
                    }
                }
                for (name, storage) in first.into_iter().flatten() {
                    store(names, name, storage)?;
                }
            }
            ast::Pattern::MatchSequence(p) => {
                if p.patterns
                    .iter()
                    .filter(|child| child.is_match_star())
                    .nth(1)
                    .is_some()
                {
                    return Err((
                        p.range(),
                        "multiple starred names in sequence pattern"
                            .to_owned()
                            .into(),
                    ));
                }
                for child in &p.patterns {
                    self.collect(child, true, names)?;
                }
            }
            ast::Pattern::MatchMapping(p) => {
                if let Some(source) = self.source {
                    mapping_keys::check(p, source)?;
                }
                for child in &p.patterns {
                    self.collect(child, true, names)?;
                }
            }
            ast::Pattern::MatchClass(p) => {
                // CPython chooses the first repeated name, then its next occurrence.
                let mut later = HashMap::new();
                let mut duplicate = None;
                for field in p.arguments.keywords.iter().rev() {
                    if let Some(next) = later.insert(field.attr.as_str(), field) {
                        duplicate = Some(next);
                    }
                }
                if let Some(field) = duplicate {
                    return Err((
                        field.pattern.range(),
                        format!("attribute name repeated in class pattern: {}", field.attr).into(),
                    ));
                }
                for child in p
                    .arguments
                    .patterns
                    .iter()
                    .chain(p.arguments.keywords.iter().map(|k| &k.pattern))
                {
                    self.collect(child, true, names)?;
                }
            }
            ast::Pattern::MatchValue(value) => {
                if let Some(source) = self.source
                    && let Some(reason) = value_pattern_error(&value.value, source)
                {
                    return Err((value.value.range(), reason.to_owned().into()));
                }
            }
            ast::Pattern::MatchSingleton(_) | ast::Pattern::MatchStar(_) => {}
        }
        if let Some(id) = capture_name(pattern) {
            let spelling = self
                .spellings
                .map_or(id.as_str(), |names| names.definition(id));
            store(names, spelling, id.as_str())?;
        }
        Ok(())
    }
}

pub fn debug_binding_error(expr: &ast::Expr) -> Option<&'static str> {
    let context = match expr {
        ast::Expr::Name(name) if name.id == "__debug__" => name.ctx,
        ast::Expr::Attribute(attr) if attr.attr.as_str() == "__debug__" => attr.ctx,
        _ => return None,
    };
    match context {
        ast::ExprContext::Store => Some("cannot assign to __debug__"),
        ast::ExprContext::Del => Some("cannot delete __debug__"),
        _ => None,
    }
}

/// The symbol table still checks an assertion's bindings when Python omits its code.
pub fn assertion_binding_error(assertion: &ast::StmtAssert) -> Option<(TextRange, &'static str)> {
    #[derive(Default)]
    struct Scan(Option<(TextRange, &'static str)>);

    impl<'ast> Visitor<'ast> for Scan {
        fn visit_expr(&mut self, expr: &'ast ast::Expr) {
            if self.0.is_none() {
                if let Some(message) = debug_binding_error(expr) {
                    self.0 = Some((expr.range(), message));
                } else {
                    visitor::walk_expr(self, expr);
                }
            }
        }

        fn visit_parameter(&mut self, parameter: &'ast ast::Parameter) {
            if self.0.is_none() {
                if parameter.name.as_str() == "__debug__" {
                    self.0 = Some((parameter.range(), "cannot assign to __debug__"));
                } else {
                    visitor::walk_parameter(self, parameter);
                }
            }
        }
    }

    let mut scan = Scan::default();
    scan.visit_expr(&assertion.test);
    if let Some(message) = &assertion.msg {
        scan.visit_expr(message);
    }
    scan.0
}

impl BindingErrors {
    pub fn check(body: &[ast::Stmt], source: &str) -> Self {
        let mut check = Check {
            errors: Self::default(),
            source,
            in_function: false,
            group: None,
        };
        check.visit_body(body);
        check.errors
    }

    /// Future directives and reserved bindings are diagnosed during symbol-table construction.
    pub fn first_symbol(
        &self,
        future: Option<(TextRange, &'static str)>,
    ) -> Option<(TextRange, &'static str)> {
        future
            .into_iter()
            .chain(self.symbol)
            .chain(self.target)
            .min_by_key(|(range, _)| range.start())
    }
}

struct Check<'s> {
    errors: BindingErrors,
    source: &'s str,
    in_function: bool,
    group: Option<GroupHandler>,
}

impl Check<'_> {
    fn handler_issue(
        &mut self,
        range: TextRange,
        message: &'static str,
        literal: Option<TextRange>,
        cleanup: bool,
    ) {
        // CPython code generation reports the first of these two kinds of error.
        if self.errors.pattern.is_some() || self.errors.handler.is_some() {
            return;
        }
        let location = literal
            .filter(|value| {
                !self.source[TextRange::new(range.start(), value.start())].contains(['\r', '\n'])
            })
            .unwrap_or(range);
        self.errors.handler = Some(HandlerError {
            range,
            location: (!cleanup).then_some(location),
            message,
        });
    }

    fn body_in(&mut self, body: &[ast::Stmt], group: Option<GroupHandler>) {
        let previous = std::mem::replace(&mut self.group, group);
        self.visit_body(body);
        self.group = previous;
    }

    fn loop_body(&mut self, body: &[ast::Stmt], otherwise: &[ast::Stmt]) {
        self.body_in(
            body,
            self.group.map(|g| GroupHandler {
                local_loop: true,
                ..g
            }),
        );
        // The current loop has been popped before its else suite.
        self.visit_body(otherwise);
    }
}

impl<'a> Visitor<'a> for Check<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::Assert(assertion) if self.errors.target.is_none() => {
                self.errors.target = assertion_binding_error(assertion);
            }
            ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {
                let function =
                    std::mem::replace(&mut self.in_function, stmt.is_function_def_stmt());
                let group = self.group.take();
                visitor::walk_stmt(self, stmt);
                self.group = group;
                self.in_function = function;
            }
            ast::Stmt::Try(t) => {
                let enclosing = self.group;
                if !t.finalbody.is_empty()
                    && let Some(group) = &mut self.group
                {
                    group.cleanup = true;
                }
                self.visit_body(&t.body);
                for (i, ast::ExceptHandler::ExceptHandler(h)) in t.handlers.iter().enumerate() {
                    if h.type_.is_none() && i + 1 < t.handlers.len() {
                        self.handler_issue(
                            h.range(),
                            "default 'except:' must be last",
                            None,
                            false,
                        );
                    }
                    self.body_in(
                        &h.body,
                        if t.is_star {
                            Some(GroupHandler::default())
                        } else {
                            self.group
                        },
                    );
                }
                self.visit_body(&t.orelse);
                self.group = enclosing;
                self.visit_body(&t.finalbody);
            }
            ast::Stmt::With(w) if self.group.is_some() => {
                for item in &w.items {
                    if let Some(target) = &item.optional_vars {
                        self.visit_expr(target);
                    }
                }
                self.body_in(
                    &w.body,
                    self.group.map(|g| GroupHandler { cleanup: true, ..g }),
                );
            }
            ast::Stmt::For(f) => {
                self.visit_expr(&f.target);
                self.loop_body(&f.body, &f.orelse);
            }
            ast::Stmt::While(w) => self.loop_body(&w.body, &w.orelse),
            ast::Stmt::Return(_) | ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {
                let Some(group) = self.group else {
                    return;
                };
                let ret = stmt.as_return_stmt();
                if ret.is_some() && !self.in_function {
                    self.handler_issue(stmt.range(), "'return' outside function", None, false);
                } else if ret.is_some() || !group.local_loop {
                    let value = ret
                        .and_then(|r| r.value.as_deref())
                        .filter(|v| {
                            v.is_literal_expr()
                                || matches!(v, ast::Expr::Name(n) if n.id == "__debug__")
                        })
                        .map(Ranged::range);
                    self.handler_issue(
                        stmt.range(),
                        "'break', 'continue' and 'return' cannot appear in an except* block",
                        value,
                        group.cleanup,
                    );
                }
            }
            ast::Stmt::Match(statement) => {
                for (index, case) in statement.cases.iter().enumerate() {
                    self.visit_pattern(&case.pattern);
                    if self.errors.pattern.is_none() {
                        self.errors.pattern =
                            case_bindings(case, self.source, index + 1 < statement.cases.len())
                                .err();
                    }
                    self.visit_body(&case.body);
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        visitor::walk_pattern(self, pattern);
        if self.errors.symbol.is_none()
            && capture_name(pattern).is_some_and(|name| name == "__debug__")
        {
            self.errors.symbol = Some((pattern.range(), "cannot assign to __debug__"));
        }
    }

    fn visit_type_param(&mut self, param: &'a ast::TypeParam) {
        if self.errors.symbol.is_none() {
            let message = match param.name().as_str() {
                "__classdict__" => {
                    "reserved name '__classdict__' cannot be used for type parameter"
                }
                "__debug__" => "cannot assign to __debug__",
                _ => return,
            };
            self.errors.symbol = Some((param.range(), message));
        }
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if self.errors.target.is_some() {
            return;
        }
        if let Some(message) = debug_binding_error(expr) {
            self.errors.target = Some((expr.range(), message));
        } else if matches!(expr,
            ast::Expr::Tuple(ast::ExprTuple { ctx, .. })
            | ast::Expr::List(ast::ExprList { ctx, .. })
            | ast::Expr::Starred(ast::ExprStarred { ctx, .. })
            if *ctx != ast::ExprContext::Load
        ) {
            visitor::walk_expr(self, expr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_transfers_follow_the_owning_loop_or_function_even_in_dead_code() {
        let text = "def f():\n return\n try: pass\n except* E:\n  for i in xs: break\n  else:\n   def g(): return\n   continue\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let issue = BindingErrors::check(&parsed.syntax().body, text)
            .handler
            .unwrap();
        assert_eq!(&text[issue.location.unwrap()], "continue");
        assert_eq!(
            issue.message,
            "'break', 'continue' and 'return' cannot appear in an except* block"
        );
    }

    #[test]
    fn earlier_pattern_survives_both_legal_and_illegal_handlers() {
        for action in ["pass", "return"] {
            let src = format!(
                "def f():\n match 1:\n  case _ | 2: pass\n try: pass\n except* E: {action}\n"
            );
            let parsed = ruff_python_parser::parse_module(&src).unwrap();
            let errors = BindingErrors::check(&parsed.syntax().body, &src);
            assert!(errors.pattern.is_some());
            assert!(errors.handler.is_none());
        }
    }

    #[test]
    fn unreachable_case_precedes_errors_inside_its_suite() {
        let text = "match 0:\n case _:\n  match 1:\n   case x | 2: pass\n case 3: pass\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let (range, reason) = BindingErrors::check(&parsed.syntax().body, text)
            .pattern
            .unwrap();
        assert_eq!(&text[range], "_");
        assert_eq!(
            reason.checked(),
            "wildcard makes remaining patterns unreachable"
        );
    }

    #[test]
    fn string_interpolations_are_invalid_value_patterns_even_without_fields() {
        for (pattern, invalid) in [
            ("f'plain'", Some("f'plain'")),
            ("t'{side()}'", Some("t'{side()}'")),
            ("{'key': [f'{side()}']}", Some("f'{side()}'")),
            ("names.Row(field=t'plain')", Some("t'plain'")),
            ("[-1-2j, b'a' b'b', 'key', names.deep.kind]", None),
        ] {
            let text = format!("match subject:\n case {pattern}: pass\n");
            let parsed = ruff_python_parser::parse_module(&text).unwrap();
            let error = BindingErrors::check(&parsed.syntax().body, &text).pattern;
            if let Some(invalid) = invalid {
                let (range, reason) = error.expect("expected an invalid value pattern");
                assert_eq!(&text[range], invalid);
                assert_eq!(
                    reason.checked(),
                    "patterns may only match literals and attribute lookups"
                );
            } else {
                assert!(error.is_none());
            }
        }
    }

    #[test]
    fn first_structural_error_precedes_child_bindings() {
        for (pattern, diagnostic) in [
            (
                "C(k=[a] | [b], k=_)",
                "attribute name repeated in class pattern: k",
            ),
            (
                "{1: [a] | [b], True: _}",
                "mapping pattern checks duplicate key (True)",
            ),
        ] {
            let text = format!("match obj:\n case {pattern}: pass\n");
            let parsed = ruff_python_parser::parse_module(&text).unwrap();
            assert_eq!(
                BindingErrors::check(&parsed.syntax().body, &text)
                    .pattern
                    .unwrap()
                    .1
                    .checked(),
                diagnostic
            );
        }
    }

    #[test]
    fn earlier_mapping_keys_do_not_hide_a_later_function_error() {
        for (second, message) in [
            (
                "18446744073709551617",
                "multiple assignments to name 'x' in pattern",
            ),
            (
                "18446744073709551616.0",
                "mapping pattern checks duplicate key (1.8446744073709552e+19)",
            ),
            (
                "0x10000000000000000",
                "mapping pattern checks duplicate key (18446744073709551616)",
            ),
        ] {
            let text = format!(
                "def first(v):\n match v:\n  case {{18446744073709551616: _, {second}: _}}: pass\ndef later(v):\n match v:\n  case [x, x]: pass\n"
            );
            let parsed = ruff_python_parser::parse_module(&text).unwrap();
            assert_eq!(
                BindingErrors::check(&parsed.syntax().body, &text)
                    .pattern
                    .unwrap()
                    .1
                    .checked(),
                message
            );
        }
    }

    #[test]
    fn or_bindings_report_the_first_failing_alternative_at_the_or_span() {
        for (pattern, span, reason) in [
            (
                "[a, [b] | [c] | [d]]",
                "[b] | [c] | [d]",
                "alternative patterns bind different names",
            ),
            (
                "[a] | [b] | [a, a]",
                "[a] | [b] | [a, a]",
                "alternative patterns bind different names",
            ),
            (
                "[a] | b | [c]",
                "b",
                "name capture 'b' makes remaining patterns unreachable",
            ),
            (
                "[a] | [b, b]",
                "b",
                "multiple assignments to name 'b' in pattern",
            ),
        ] {
            let text = format!("match []:\n case {pattern}: pass\nclass C[__classdict__]: pass\n");
            let parsed = ruff_python_parser::parse_module(&text).unwrap();
            let bindings = BindingErrors::check(&parsed.syntax().body, &text);
            let (range, message) = bindings.pattern.unwrap();
            assert_eq!((&text[range], message.checked().as_ref()), (span, reason));
            assert!(bindings.symbol.is_some());
        }

        let parsed =
            ruff_python_parser::parse_module("match []:\n case [a, b] | [b, a]: pass\n").unwrap();
        let ast::Stmt::Match(stmt) = &parsed.syntax().body[0] else {
            unreachable!()
        };
        assert_eq!(
            source_case_bindings(&stmt.cases[0], false, None).unwrap(),
            ["a", "b"]
        );
    }
}
