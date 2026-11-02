//! Expand Python's case equivalents before the engine applies class negation or quantifiers.

use std::convert::Infallible;
use std::fmt::Write;

use regex_syntax::ast::{self, Ast, ClassSetItem, Span, Visitor};

use super::Alphabet;

/// Expand Python's additional case equivalents for an unscoped backend expression.
pub fn expand_case(source: &str, ascii: bool) -> Result<String, String> {
    let alphabet = if ascii {
        Alphabet::AsciiIgnoreCase
    } else {
        Alphabet::UnicodeIgnoreCase
    };
    scoped(source, &[(0, alphabet)], false)
}

/// Expand each literal under its lexical Python alphabet before class negation or repetition.
pub(super) fn scoped(
    source: &str,
    modes: &[(usize, Alphabet)],
    lookahead: bool,
) -> Result<String, String> {
    // regex-syntax cannot parse lookarounds. Substituting same-width group
    // openers lets its AST address literals at their original byte offsets.
    let masked;
    let syntax = if lookahead {
        if source.contains(r"\k<") || source.contains("(?(") {
            return Err("case-insensitive lookahead group references are unsupported".into());
        }
        masked = source
            .replace("(?=", "(?:")
            .replace("(?!", "(?:")
            .replace("(?>", "(?:")
            .replace("(?<=", "(?i:")
            .replace("(?<!", "(?i:");
        &masked
    } else {
        source
    };
    let ast = ast::parse::Parser::new()
        .parse(syntax)
        .map_err(|err| err.to_string())?;
    let visitor = Fold {
        source,
        modes,
        copied: 0,
        output: String::with_capacity(source.len()),
    };
    Ok(ast::visit(&ast, visitor).unwrap())
}

struct Fold<'a> {
    source: &'a str,
    modes: &'a [(usize, Alphabet)],
    copied: usize,
    output: String,
}

impl Fold<'_> {
    fn range(&mut self, span: Span, start: char, end: char, bracketed: bool) {
        let mut extra = String::new();
        let index = self
            .modes
            .partition_point(|(offset, _)| *offset <= span.start.offset);
        let alphabet = self.modes[index - 1].1;
        if !alphabet.ignores_case() {
            return;
        }
        if alphabet == Alphabet::AsciiIgnoreCase {
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
