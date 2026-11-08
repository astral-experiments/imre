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
    pub pattern: Option<BindingError>,
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

/// Check names in store order, optionally recording how each successful OR arm
/// moves its tentative captures into the first arm's slots. Private names are
/// compared as written, even when two spellings have the same storage name.
#[derive(Default)]
struct CapturePlan<'a, 'out> {
    spellings: Option<&'a crate::Names>,
    slots: Option<&'out mut HashMap<TextRange, Vec<usize>>>,
    // Slot allocation runs after the original executable syntax has been checked.
    source: Option<&'a str>,
}

pub fn case_bindings<'a>(
    case: &'a ast::MatchCase,
    source: &'a str,
    has_next: bool,
) -> Result<impl Iterator<Item = &'a str>, BindingError> {
    let mut plan = CapturePlan {
        source: Some(source),
        ..Default::default()
    };
    Ok(plan.case(case, has_next)?.into_keys())
}

pub fn source_case_bindings<'a>(
    case: &'a ast::MatchCase,
    has_next: bool,
    spellings: Option<&'a crate::Names>,
    slots: &mut HashMap<TextRange, Vec<usize>>,
) -> Result<Vec<String>, BindingError> {
    let mut plan = CapturePlan {
        spellings,
        slots: Some(slots),
        source: None,
    };
    Ok(plan
        .case(case, has_next)?
        .into_values()
        .map(str::to_owned)
        .collect())
}

impl<'a> CapturePlan<'a, '_> {
    fn case(
        &mut self,
        case: &'a ast::MatchCase,
        has_next: bool,
    ) -> Result<Names<'a>, BindingError> {
        let mut names = IndexMap::new();
        self.collect(&case.pattern, case.guard.is_some() || !has_next, &mut names)?;
        Ok(names)
    }

    fn collect(
        &mut self,
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
                    let expected = first.as_ref().unwrap_or(&next);
                    let different = || {
                        (
                            or.range(),
                            "alternative patterns bind different names"
                                .to_owned()
                                .into(),
                        )
                    };
                    if next.len() != expected.len() {
                        return Err(different());
                    }
                    let mut order = Vec::new();
                    for name in expected.keys() {
                        let slot = next.get_index_of(name).ok_or_else(different)?;
                        if self.slots.is_some() {
                            order.push(slot);
                        }
                    }
                    if let Some(slots) = self.slots.as_deref_mut()
                        && !order.is_empty()
                    {
                        slots.insert(arm.range(), order);
                    }
                    first.get_or_insert(next);
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
            ast::Pattern::MatchValue(_)
            | ast::Pattern::MatchSingleton(_)
            | ast::Pattern::MatchStar(_) => {}
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

impl BindingErrors {
    pub fn check(body: &[ast::Stmt], source: &str) -> Self {
        let mut check = Check {
            errors: Self::default(),
            source,
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
            .min_by_key(|(range, _)| range.start())
    }
}

struct Check<'a> {
    errors: BindingErrors,
    source: &'a str,
}

impl<'a> Visitor<'a> for Check<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if let ast::Stmt::Match(statement) = stmt {
            for (index, case) in statement.cases.iter().enumerate() {
                self.visit_pattern(&case.pattern);
                if self.errors.pattern.is_none() {
                    self.errors.pattern =
                        case_bindings(case, self.source, index + 1 < statement.cases.len()).err();
                }
                self.visit_body(&case.body);
            }
        } else {
            visitor::walk_stmt(self, stmt);
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

    fn visit_expr(&mut self, _: &'a ast::Expr) {}
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let case = &stmt.cases[0];
        let ast::Pattern::MatchOr(or) = &case.pattern else {
            unreachable!()
        };
        let mut slots = HashMap::new();
        assert_eq!(
            source_case_bindings(case, false, None, &mut slots).unwrap(),
            ["a", "b"]
        );
        assert_eq!(slots[&or.patterns[0].range()], [0, 1]);
        assert_eq!(slots[&or.patterns[1].range()], [1, 0]);
    }
}
