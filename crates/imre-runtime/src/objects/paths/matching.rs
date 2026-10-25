//! Lexical path matching. Only full_match lets a whole ** component cross separators.

use regex_automata::meta::Regex;
use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, Dot, Hir, Look, Repetition};

use super::{Error, Kind, Object, Pattern, Result, Str, Token, finish, normalize, object};

fn repeat(sub: Hir, min: u32, max: Option<u32>) -> Hir {
    Hir::repetition(Repetition {
        min,
        max,
        greedy: true,
        sub: Box::new(sub),
    })
}

fn character_class(ranges: &[(char, char)], negated: bool, sensitive: bool) -> Hir {
    let mut class = ClassUnicode::new(
        ranges
            .iter()
            .map(|&(lo, hi)| ClassUnicodeRange::new(lo, hi)),
    );
    if !sensitive {
        // CPython's Unicode IGNORECASE also groups the dotted and dotless I
        // with ASCII i and I, including inside (potentially negated) ranges.
        let variants = ['i', 'I', 'İ', 'ı'];
        if variants
            .iter()
            .any(|c| ranges.iter().any(|(lo, hi)| lo <= c && c <= hi))
        {
            for c in variants {
                class.push(ClassUnicodeRange::new(c, c));
            }
        }
        class.case_fold_simple();
    }
    if negated {
        class.negate();
    }
    Hir::class(Class::Unicode(class))
}

fn literal(text: &str, sensitive: bool) -> Hir {
    if sensitive {
        Hir::literal(text.as_bytes())
    } else {
        Hir::concat(
            text.chars()
                .map(|c| character_class(&[(c, c)], false, false))
                .collect(),
        )
    }
}

fn component(pattern: &str, sensitive: bool) -> Hir {
    match Pattern::component(pattern) {
        Pattern::Literal(text) => literal(&text, sensitive),
        Pattern::Wildcard(tokens) => Hir::concat(
            tokens
                .into_iter()
                .map(|token| match token {
                    Token::Any => Hir::dot(Dot::AnyCharExcept('/')),
                    Token::Star => repeat(Hir::dot(Dot::AnyCharExcept('/')), 0, None),
                    Token::Class { negated, ranges } => {
                        character_class(&ranges, negated, sensitive)
                    }
                    Token::Literal(c) => character_class(&[(c, c)], false, sensitive),
                })
                .collect(),
        ),
    }
}

/// Compile literal components, separators and recursive wildcards separately.
/// In particular, "a/**" still requires the slash after "a"; an intermediate
/// "**/" may consume zero components, including its own trailing separator.
fn glob_matches(path: &str, pattern: &str, sensitive: bool, recursive: bool) -> Result<bool> {
    if sensitive && !pattern.contains(['*', '?', '[']) {
        return Ok(path == pattern);
    }
    let slash = || Hir::literal(*b"/");
    let mut parts = pattern.split('/').peekable();
    let mut expression = vec![Hir::look(Look::Start)];
    while let Some(part) = parts.next() {
        let next = parts.peek().copied();
        if recursive && part == "**" {
            match next {
                Some("**") => {}
                Some(_) => expression.push(repeat(
                    Hir::concat(vec![repeat(Hir::dot(Dot::AnyChar), 1, None), slash()]),
                    0,
                    Some(1),
                )),
                None => expression.push(repeat(Hir::dot(Dot::AnyChar), 0, None)),
            }
        } else {
            expression.push(if part == "*" {
                repeat(Hir::dot(Dot::AnyCharExcept('/')), 1, None)
            } else {
                component(part, sensitive)
            });
            if next.is_some() {
                expression.push(slash());
            }
        }
    }
    expression.push(Hir::look(Look::End));
    let regex = Regex::builder()
        .build_from_hir(&Hir::concat(expression))
        .map_err(|e| Error::new(Kind::MemoryError, e.to_string()))?;
    Ok(regex.is_match(path))
}

/// The normalized POSIX anchor, including the distinct // root, is one part.
fn parts(path: &str) -> impl DoubleEndedIterator<Item = &str> {
    let (root, tail) = path.split_at(path.bytes().take_while(|&b| b == b'/').count());
    (!root.is_empty())
        .then_some(root)
        .into_iter()
        .chain(tail.split('/').filter(|p| !p.is_empty() && *p != "."))
}

fn matches(path: &str, pattern: &str, sensitive: bool, full: bool) -> Result<bool> {
    let pattern = normalize(pattern);
    if full {
        return glob_matches(
            if path == "." { "" } else { path },
            if pattern == "." { "" } else { &pattern },
            sensitive,
            true,
        );
    }
    if pattern == "." {
        return Err(Error::new(Kind::ValueError, "empty pattern"));
    }
    let mut actual = parts(path).rev();
    for expected in parts(&pattern).rev() {
        let Some(part) = actual.next() else {
            return Ok(false);
        };
        if !glob_matches(part, expected, sensitive, false)? {
            return Ok(false);
        }
    }
    Ok(!pattern.starts_with('/') || actual.next().is_none())
}

/// # Safety
/// Both inputs are rooted through this callback-free operation. The optional
/// boolean uses Imre's 0=None, 1=False, 2=True representation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_match(
    path: *const Object,
    pattern: *const Str,
    case: u64,
    full: u32,
) -> u32 {
    // SAFETY: the caller holds the immutable path and pattern until matching returns.
    let result = unsafe {
        object(path)
            .0
            .with(|p| matches(p.path(), &*pattern, case != 1, full != 0))
    };
    finish(result.map(u32::from), 0)
}
