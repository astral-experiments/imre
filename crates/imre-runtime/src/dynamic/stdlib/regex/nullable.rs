//! Preserve the last empty capture for simple greedy nullable loops.
use regex_syntax::ast::{self, Ast, RepetitionKind, Visitor};
use std::convert::Infallible;

pub(super) fn expand(source: &str) -> Result<String, String> {
    let ast = ast::parse::Parser::new()
        .parse(source)
        .map_err(|e| e.to_string())?;
    struct Rewrite<'a> {
        source: &'a str,
        copied: usize,
        output: String,
    }
    impl Visitor for Rewrite<'_> {
        type Output = String;
        type Err = Infallible;
        fn visit_pre(&mut self, ast: &Ast) -> Result<(), Infallible> {
            let Ast::Repetition(outer) = ast else {
                return Ok(());
            };
            if !outer.greedy
                || !matches!(
                    outer.op.kind,
                    RepetitionKind::ZeroOrMore | RepetitionKind::OneOrMore
                )
            {
                return Ok(());
            }
            let Ast::Group(group) = &*outer.ast else {
                return Ok(());
            };
            if !group.is_capturing() {
                return Ok(());
            }
            let Ast::Repetition(inner) = &*group.ast else {
                return Ok(());
            };
            if !inner.greedy
                || !matches!(
                    inner.op.kind,
                    RepetitionKind::ZeroOrOne | RepetitionKind::ZeroOrMore
                )
                || !matches!(
                    &*inner.ast,
                    Ast::Literal(_)
                        | Ast::Dot(_)
                        | Ast::ClassUnicode(_)
                        | Ast::ClassPerl(_)
                        | Ast::ClassBracketed(_)
                )
            {
                return Ok(());
            }
            let start = outer.span.start.offset;
            let end = outer.span.end.offset;
            self.output.push_str(&self.source[self.copied..start]);
            self.output.push_str("(?:");
            self.output
                .push_str(&self.source[inner.span.start.offset..inner.span.end.offset]);
            self.output.push_str(")*");
            self.output
                .push_str(&self.source[group.span.start.offset..group.span.end.offset]);
            self.copied = end;
            Ok(())
        }
        fn finish(mut self) -> Result<String, Infallible> {
            self.output.push_str(&self.source[self.copied..]);
            Ok(self.output)
        }
    }
    Ok(ast::visit(
        &ast,
        Rewrite {
            source,
            copied: 0,
            output: String::with_capacity(source.len()),
        },
    )
    .unwrap())
}
