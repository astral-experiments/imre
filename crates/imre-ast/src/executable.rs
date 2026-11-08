//! Binding restrictions checked when a parsed Python module becomes executable.
use ruff_python_ast::{
    self as ast,
    visitor::{self, Visitor},
};
use ruff_text_size::{Ranged, TextRange};

#[derive(Default)]
pub struct BindingErrors {
    /// Symbol-table errors precede pattern code generation.
    pub symbol: Option<(TextRange, &'static str)>,
    pub pattern: Option<(TextRange, String)>,
}

/// An unguarded case can make subsequent cases unreachable.
pub fn unreachable_case(case: &ast::MatchCase, has_next: bool) -> Option<ast::IrrefutablePattern> {
    unreachable_alternative(&case.pattern, has_next && case.guard.is_none())
}

/// An irrefutable OR arm always consumes the subject before a later arm can run.
pub fn unreachable_alternative(
    pattern: &ast::Pattern,
    has_next: bool,
) -> Option<ast::IrrefutablePattern> {
    if has_next {
        pattern.irrefutable_pattern()
    } else {
        None
    }
}

impl BindingErrors {
    pub fn check(body: &[ast::Stmt]) -> Self {
        let mut errors = Self::default();
        errors.visit_body(body);
        errors
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

    fn record_unreachable(&mut self, binding: Option<ast::IrrefutablePattern>) {
        if self.pattern.is_none()
            && let Some(binding) = binding
        {
            let kind = match binding.kind {
                ast::IrrefutablePatternKind::Wildcard => "wildcard".to_owned(),
                ast::IrrefutablePatternKind::Name(name) => format!("name capture '{name}'"),
            };
            self.pattern = Some((
                binding.range,
                format!("{kind} makes remaining patterns unreachable"),
            ));
        }
    }
}

impl<'a> Visitor<'a> for BindingErrors {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        if let ast::Stmt::Match(statement) = stmt {
            for (index, case) in statement.cases.iter().enumerate() {
                self.visit_pattern(&case.pattern);
                self.record_unreachable(unreachable_case(case, index + 1 < statement.cases.len()));
                self.visit_body(&case.body);
            }
        } else {
            visitor::walk_stmt(self, stmt);
        }
    }

    fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
        if let ast::Pattern::MatchOr(or) = pattern {
            for (index, arm) in or.patterns.iter().enumerate() {
                self.visit_pattern(arm);
                self.record_unreachable(unreachable_alternative(
                    arm,
                    index + 1 < or.patterns.len(),
                ));
            }
        } else {
            visitor::walk_pattern(self, pattern);
        }
        let name = match pattern {
            ast::Pattern::MatchAs(p) => &p.name,
            ast::Pattern::MatchStar(p) => &p.name,
            ast::Pattern::MatchMapping(p) => &p.rest,
            _ => return,
        };
        if self.symbol.is_none() && name.as_ref().is_some_and(|name| name == "__debug__") {
            self.symbol = Some((pattern.range(), "cannot assign to __debug__"));
        }
    }

    fn visit_type_param(&mut self, param: &'a ast::TypeParam) {
        if self.symbol.is_none() {
            let message = match param.name().as_str() {
                "__classdict__" => {
                    "reserved name '__classdict__' cannot be used for type parameter"
                }
                "__debug__" => "cannot assign to __debug__",
                _ => return,
            };
            self.symbol = Some((param.range(), message));
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
        let (range, reason) = BindingErrors::check(&parsed.syntax().body).pattern.unwrap();
        assert_eq!(&text[range], "_");
        assert_eq!(reason, "wildcard makes remaining patterns unreachable");
    }
}
