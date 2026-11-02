//! Expand Python's case equivalents before the engine applies class negation or quantifiers.

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::fmt::Write;

use regex_syntax::ast::{self, Ast, ClassSetItem, Span, Visitor};
use regex_syntax::hir::{Class, Hir, HirKind, translate::Translator};

use super::Alphabet;

/// Expand Python's additional case equivalents for the raw backend.
pub fn expand_case(source: &str, ascii: bool) -> Result<String, String> {
    let alphabet = if ascii {
        Alphabet::AsciiIgnoreCase
    } else {
        Alphabet::UnicodeIgnoreCase
    };
    let ast = ast::parse::Parser::new()
        .parse(source)
        .map_err(|e| e.to_string())?;
    let mut modes = vec![(0, alphabet)];
    if source.contains("(?") && source.contains('-') {
        raw_case_scopes(&ast, alphabet, &mut modes);
    }
    Ok(fold(&ast, source, &modes))
}

// The raw backend can accept compound scopes which the Python translator cannot lower.
// Keep the source offsets so Fold leaves case-sensitive literals and classes unchanged.
fn raw_case_scopes(node: &Ast, mode: Alphabet, modes: &mut Vec<(usize, Alphabet)>) {
    match node {
        Ast::Group(group) => {
            let inner = group
                .flags()
                .and_then(|flags| flags.flag_state(ast::Flag::CaseInsensitive))
                .map_or(mode, |enabled| mode.scoped('i', enabled).unwrap());
            if inner != mode {
                modes.push((group.span.start.offset, inner));
            }
            raw_case_scopes(&group.ast, inner, modes);
            if inner != mode {
                modes.push((group.span.end.offset, mode));
            }
        }
        Ast::Repetition(rep) => raw_case_scopes(&rep.ast, mode, modes),
        Ast::Concat(concat) => concat
            .asts
            .iter()
            .for_each(|a| raw_case_scopes(a, mode, modes)),
        Ast::Alternation(alt) => alt
            .asts
            .iter()
            .for_each(|a| raw_case_scopes(a, mode, modes)),
        _ => {}
    }
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
        masked = mask_assertions(source);
        &masked
    } else {
        source
    };
    let ast = ast::parse::Parser::new()
        .parse(syntax)
        .map_err(|err| err.to_string())?;
    Ok(fold(&ast, source, modes))
}

fn mask_assertions(source: &str) -> String {
    source
        .replace("(?=", "(?:")
        .replace("(?!", "(?:")
        .replace("(?>", "(?:")
        .replace("(?<=", "(?i:")
        .replace("(?<!", "(?i:")
}

fn fold(ast: &Ast, source: &str, modes: &[(usize, Alphabet)]) -> String {
    ast::visit(
        ast,
        Fold {
            source,
            modes,
            copied: 0,
            output: String::with_capacity(source.len()),
        },
    )
    .unwrap()
}

/// A case-insensitive VM reference compares a fixed number of bytes. CPython compares
/// characters by their simple lowercase mapping; these comparisons agree on an ASCII
/// capture without the non-ASCII equivalences of I/i, K/k, or S/s in Unicode mode.
pub(super) fn backrefs(source: &str, modes: &[(usize, Alphabet)]) -> Result<String, String> {
    let [(0, alphabet)] = modes else {
        return Err("scoped case modes with group references are unsupported".into());
    };
    if source.contains("(?(") {
        return Err("case-insensitive conditionals are unsupported".into());
    }
    let mut referenced = BTreeSet::new();
    let mut pattern = String::with_capacity(source.len());
    let mut masked = String::with_capacity(source.len());
    let mut rest = source;
    while let Some((prefix, after)) = rest.split_once(r"\k<") {
        let (digits, tail) = after.split_once('>').ok_or("invalid group reference")?;
        let group: u32 = digits.parse().map_err(|_| "invalid group reference")?;
        referenced.insert(group);
        pattern.push_str(prefix);
        masked.push_str(prefix);
        // ASCII ignorecase on str expands the literals explicitly. Enable the VM's
        // case comparison only around the reference, leaving other text atoms alone.
        if *alphabet == Alphabet::AsciiIgnoreCase {
            pattern.push_str("(?i:");
            masked.push_str("(?i:");
        }
        write!(pattern, r"\k<{group}>").unwrap();
        // One opaque atom of the same source length. A referenced capture containing
        // another reference therefore cannot accidentally be proved ASCII-only.
        masked.push_str("(?:");
        masked.extend(std::iter::repeat_n('.', digits.len()));
        masked.push(')');
        if *alphabet == Alphabet::AsciiIgnoreCase {
            pattern.push(')');
            masked.push(')');
        }
        rest = tail;
    }
    pattern.push_str(rest);
    masked.push_str(rest);
    let masked = mask_assertions(&masked);
    let ast = ast::parse::Parser::new()
        .parse(&masked)
        .map_err(|e| e.to_string())?;
    let hir = Translator::new()
        .translate(&masked, &ast)
        .map_err(|e| e.to_string())?;
    let mut pending = vec![&hir];
    while let Some(node) = pending.pop() {
        if let HirKind::Capture(capture) = node.kind()
            && referenced.remove(&capture.index)
            && !ascii_capture(&capture.sub, *alphabet == Alphabet::UnicodeIgnoreCase)
        {
            return Err("case-insensitive group reference can capture non-ASCII text".into());
        }
        pending.extend(node.kind().subs());
    }
    if !referenced.is_empty() {
        return Err("cannot verify case-insensitive group reference".into());
    }
    Ok(fold(&ast, &pattern, modes))
}

fn ascii_capture(hir: &Hir, unicode: bool) -> bool {
    let safe = |lo: u32, hi: u32| {
        hi <= 0x7f && !(unicode && b"IiKkSs".iter().any(|&c| (lo..=hi).contains(&u32::from(c))))
    };
    let mut pending = vec![hir];
    while let Some(node) = pending.pop() {
        let allowed = match node.kind() {
            HirKind::Literal(lit) => lit.0.iter().all(|&c| safe(c.into(), c.into())),
            HirKind::Class(Class::Unicode(cls)) => {
                cls.iter().all(|r| safe(r.start().into(), r.end().into()))
            }
            HirKind::Class(Class::Bytes(cls)) => {
                cls.iter().all(|r| safe(r.start().into(), r.end().into()))
            }
            _ => true,
        };
        if !allowed {
            return false;
        }
        pending.extend(node.kind().subs());
    }
    true
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
