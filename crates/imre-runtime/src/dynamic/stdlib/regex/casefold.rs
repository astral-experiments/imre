//! Expand Python's case equivalents before the engine applies class negation or quantifiers.

use std::convert::Infallible;
use std::fmt::Write;

use regex_syntax::ast::{self, Ast, ClassSetItem, Span, Visitor};

pub(super) fn expand(source: &str, ascii: bool) -> Result<String, String> {
    let ast = ast::parse::Parser::new()
        .parse(source)
        .map_err(|err| err.to_string())?;
    let visitor = Fold {
        source,
        ascii,
        copied: 0,
        output: String::with_capacity(source.len()),
    };
    Ok(ast::visit(&ast, visitor).unwrap())
}

struct Fold<'a> {
    source: &'a str,
    ascii: bool,
    copied: usize,
    output: String,
}

impl Fold<'_> {
    fn range(&mut self, span: Span, start: char, end: char, bracketed: bool) {
        let mut extra = String::new();
        if self.ascii {
            for (low, high) in [('a', 'z'), ('A', 'Z')] {
                let first = low.max(start);
                let last = high.min(end);
                if first <= last {
                    let flip = |c: char| char::from(c as u8 ^ 32);
                    extra.push(flip(first));
                    if first != last {
                        extra.push('-');
                        extra.push(flip(last));
                    }
                }
            }
        } else if ['I', 'i', 'İ', 'ı']
            .into_iter()
            .any(|c| (start..=end).contains(&c))
        {
            // These four share a Python simple lowercase/uppercase equivalence
            // class. The engine's default Unicode simple fold only joins I/i.
            extra.push_str("Iiİı");
        }
        if extra.is_empty() {
            return;
        }
        self.output
            .push_str(&self.source[self.copied..span.start.offset]);
        if !bracketed {
            self.output.push('[');
        }
        // Emit code points explicitly: literals meaningful in a bracketed class
        // (such as a dash at a range endpoint) keep their original value.
        write!(self.output, r"\x{{{:x}}}", start as u32).unwrap();
        if end != start {
            write!(self.output, r"-\x{{{:x}}}", end as u32).unwrap();
        }
        self.output.push_str(&extra);
        if !bracketed {
            self.output.push(']');
        }
        self.copied = span.end.offset;
    }
}

impl Visitor for Fold<'_> {
    type Output = String;
    type Err = Infallible;

    fn visit_pre(&mut self, node: &Ast) -> Result<(), Infallible> {
        if let Ast::Literal(lit) = node {
            self.range(lit.span, lit.c, lit.c, false);
        }
        Ok(())
    }

    fn visit_class_set_item_pre(&mut self, item: &ClassSetItem) -> Result<(), Infallible> {
        match item {
            ClassSetItem::Literal(lit) => self.range(lit.span, lit.c, lit.c, true),
            ClassSetItem::Range(range) => self.range(range.span, range.start.c, range.end.c, true),
            _ => {}
        }
        Ok(())
    }

    fn finish(mut self) -> Result<String, Infallible> {
        self.output.push_str(&self.source[self.copied..]);
        Ok(self.output)
    }
}
