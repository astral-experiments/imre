//! Syntax validation for forward-reference expressions, independent of their evaluation.

pub enum Error {
    Syntax(&'static str),
    InvalidScope(crate::async_generators::Error),
    Scope,
}

/// Turn an unpacked annotation such as `*Ts` into an expression suitable for `eval`.
pub fn starred_expression(spelling: &str) -> String {
    format!("({spelling},)[0]")
}

/// A tokenizer guard or a grammar error in the decoded source.
#[derive(Debug)]
pub enum ParseError {
    Token(&'static str),
    Grammar(ruff_python_parser::ParseError),
    Syntax(ruff_text_size::TextRange, &'static str),
}

/// Parse annotation text without evaluating it or constructing Python AST objects.
pub fn parse(
    source: &str,
) -> Result<ruff_python_parser::Parsed<ruff_python_ast::ModExpression>, ParseError> {
    use ruff_python_ast::token::TokenKind;

    if source.contains('\0') {
        return Err(ParseError::Token(
            "source code string cannot contain null bytes",
        ));
    }
    // CPython's tokenizer caps the combined nesting of (), [] and {} at 200.
    // Keep deeply nested input out of the recursive parser. Short references only lex once.
    const MAX_NESTING: usize = 200;
    if source.len() > MAX_NESTING {
        let mut lexer =
            ruff_python_parser::lexer::lex(source, ruff_python_parser::Mode::Expression);
        let mut depth = 0_usize;
        loop {
            match lexer.next_token() {
                TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                    depth += 1;
                    if depth > MAX_NESTING {
                        return Err(ParseError::Token("too many nested parentheses"));
                    }
                }
                TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => {
                    depth = depth.saturating_sub(1)
                }
                TokenKind::EndOfFile | TokenKind::Unknown => break,
                _ => {}
            }
        }
    }
    let parsed = ruff_python_parser::parse_expression(source).map_err(ParseError::Grammar)?;
    if let Some((range, reason)) = crate::comprehension::dict_unpacking_error(&parsed) {
        return Err(ParseError::Syntax(range, reason));
    }
    Ok(parsed)
}

/// Check a forward reference without evaluating names or converting the tree into Python AST objects.
pub fn validate(
    parsed: &ruff_python_parser::Parsed<ruff_python_ast::ModExpression>,
) -> Result<(), Error> {
    use ruff_python_ast::token::TokenKind;
    use ruff_python_ast::visitor::{self, Visitor};
    use ruff_python_ast::{Expr, ExprContext};
    use ruff_text_size::Ranged;
    use std::collections::HashSet;

    fn invalid_binding<'a>(mut names: impl Iterator<Item = &'a str>) -> bool {
        let mut seen = HashSet::new();
        names.any(|name| name == "__debug__" || !seen.insert(name))
    }

    #[derive(Default)]
    struct Bindings {
        invalid: bool,
    }
    impl<'a> Visitor<'a> for Bindings {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.invalid {
                return;
            }
            self.invalid = match expr {
                Expr::Call(call) => invalid_binding(
                    call.arguments
                        .keywords
                        .iter()
                        .filter_map(|kw| kw.arg.as_deref()),
                ),
                Expr::Lambda(lambda) => lambda.parameters.as_ref().is_some_and(|params| {
                    invalid_binding(params.iter().map(|p| p.name().as_str()))
                }),
                Expr::Name(name) => name.ctx == ExprContext::Store && name.id == "__debug__",
                Expr::Attribute(attr) => {
                    attr.ctx == ExprContext::Store && attr.attr.as_str() == "__debug__"
                }
                _ => false,
            };
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_expr(self, expr)
            });
        }
    }
    let body = parsed.syntax().body.as_ref();
    let invalid = match body {
        Expr::Yield(_) => Some("'yield' outside function"),
        Expr::YieldFrom(_) => Some("'yield from' outside function"),
        Expr::Await(_) => Some("'await' outside function"),
        _ => None,
    };
    if let Some(message) = invalid {
        return Err(Error::Syntax(message));
    }
    // Comprehension targets restrict assignments in their lexical scope.
    let tokens = parsed.tokens();
    let comprehension = tokens.iter().any(|t| t.kind() == TokenKind::For);
    if comprehension
        && tokens.iter().any(|t| t.kind() == TokenKind::ColonEqual)
        && let Some(error) = crate::async_generators::check(parsed.syntax().into(), true)
    {
        return Err(Error::InvalidScope(error));
    }
    let lambda_body = body.as_lambda_expr().map(|lambda| lambda.body.range());
    if tokens.iter().any(|token| match token.kind() {
        TokenKind::Await | TokenKind::Async => true,
        TokenKind::Yield => {
            comprehension || !lambda_body.is_some_and(|r| r.contains_range(token.range()))
        }
        _ => false,
    }) {
        return Err(Error::Scope);
    }
    let mut bindings = Bindings::default();
    bindings.visit_expr(body);
    if bindings.invalid {
        return Err(Error::Syntax("invalid binding in forward reference"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_expressions_use_their_comprehension_scope() {
        for (source, valid) in [
            ("((callee := capture), tuple(callee(i) for i in items))", true),
            ("((lambda: (i := 1))() for i in items)", true),
            ("((i := 1) for i in items)", false),
            ("(i for i in (items := values))", false),
            ("(await f(), tuple(i for i in items))", false),
        ] {
            assert_eq!(validate(&parse(source).unwrap()).is_ok(), valid, "{source}");
        }
    }
}
