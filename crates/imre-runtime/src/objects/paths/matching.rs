//! Lexical path matching. Only full_match lets a whole ** component cross separators.

use regex_automata::meta::Regex;
use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, Dot, Hir, Look, Repetition};

use super::{
    Error, Kind, Object, Pattern, PurePath, Result, Storage, Str, Token, finish, normalize, object,
};

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

fn component(pattern: &str, sensitive: bool, sep: char) -> Hir {
    match Pattern::component(pattern) {
        Pattern::Literal(text) => literal(&text, sensitive),
        Pattern::Affixes { text, stars } => Hir::concat(vec![
            literal(&text[..stars.start], sensitive),
            repeat(Hir::dot(Dot::AnyCharExcept(sep)), 0, None),
            literal(&text[stars.end..], sensitive),
        ]),
        Pattern::Wildcard(tokens) => Hir::concat(
            tokens
                .into_iter()
                .map(|token| match token {
                    Token::Any => Hir::dot(Dot::AnyCharExcept(sep)),
                    Token::Star => repeat(Hir::dot(Dot::AnyCharExcept(sep)), 0, None),
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
fn glob_matches(
    path: &str,
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> Result<bool> {
    if sensitive && !pattern.contains(['*', '?', '[']) {
        return Ok(path == pattern);
    }
    if let Some(matched) = match_literal_components(path, pattern, sensitive, recursive, sep) {
        return Ok(matched);
    }
    complex_glob_matches(path, pattern, sensitive, recursive, sep)
}

// Keep token and regex compilation out of the literal-component loop.
#[inline(never)]
fn complex_glob_matches(
    path: &str,
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> Result<bool> {
    if let Some(matched) = match_components(path, pattern, sensitive, recursive, sep) {
        return Ok(matched);
    }
    Ok(glob_regex(pattern, sensitive, recursive, sep)?.is_match(path))
}

/// Match recursive components directly. Character classes that can consume a separator
/// and Unicode case folding retain the regex implementation's full semantics.
fn match_components(
    path: &str,
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> Option<bool> {
    if !sensitive && !(path.is_ascii() && pattern.is_ascii()) {
        return None;
    }
    if pattern.contains('[') && pattern.split(sep).any(|part| {
        matches!(Pattern::component(part), Pattern::Wildcard(tokens) if tokens.iter().any(|token| {
            matches!(token, Token::Class { .. }) && token.matches(sep)
        }))
    }) {
        return None;
    }

    let (mut at, mut token) = (0, 0);
    let mut star = None;
    loop {
        let tail = &pattern[token..];
        let end = tail.find(sep).map_or(pattern.len(), |end| token + end);
        let part = &pattern[token..end];
        if recursive && part == "**" {
            if end == pattern.len() {
                return Some(true);
            }
            token = end + sep.len_utf8();
            star = Some((token, at, at));
            continue;
        }
        let name_end = path[at..].find(sep).map_or(path.len(), |end| at + end);
        if match_component(&path[at..name_end], part, sensitive) && (part != "*" || name_end > at) {
            if end == pattern.len() && name_end == path.len() {
                return Some(true);
            }
            if end < pattern.len() && name_end < path.len() {
                token = end + sep.len_utf8();
                at = name_end + sep.len_utf8();
                continue;
            }
        }
        let Some((retry, start, search)) = star.as_mut() else {
            return Some(false);
        };
        let mut rest = &path[*search..];
        loop {
            let Some(offset) = rest.find(sep) else {
                return Some(false);
            };
            let slash = *search + offset;
            *search = slash + sep.len_utf8();
            if slash > *start {
                at = *search;
                token = *retry;
                break;
            }
            rest = &path[*search..];
        }
    }
}

/// Match one path component. The single-star case checks the fixed suffix backwards,
/// while multiple stars reuse the shell matcher's bounded-backtracking algorithm.
fn match_component(path: &str, pattern: &str, sensitive: bool) -> bool {
    let parsed = Pattern::component(pattern);
    if sensitive {
        if let Pattern::Wildcard(tokens) = &parsed
            && let Some(matched) = match_token_component(path, tokens)
        {
            return matched;
        }
        return parsed.matches(path);
    }
    match parsed {
        Pattern::Literal(text) => path.eq_ignore_ascii_case(&text),
        Pattern::Affixes { text, stars } => {
            let prefix = &text[..stars.start];
            let suffix = &text[stars.end..];
            path.len() >= prefix.len() + suffix.len()
                && path[..prefix.len()].eq_ignore_ascii_case(prefix)
                && path[path.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        }
        Pattern::Wildcard(tokens) => match_ascii_tokens(path.as_bytes(), &tokens),
    }
}

fn match_ascii_tokens(path: &[u8], tokens: &[Token]) -> bool {
    let (mut at, mut token) = (0, 0);
    let mut star = None;
    while at < path.len() {
        if matches!(tokens.get(token), Some(Token::Star)) {
            star = Some((token, at));
            token += 1;
            continue;
        }
        let character = char::from(path[at]);
        let matches = tokens.get(token).is_some_and(|token| match token {
            Token::Literal(expected) => expected.eq_ignore_ascii_case(&character),
            Token::Any => true,
            Token::Star => false,
            Token::Class { negated, ranges } => {
                let lower = character.to_ascii_lowercase();
                let upper = character.to_ascii_uppercase();
                ranges.iter().any(|&(start, end)| {
                    (start <= lower && lower <= end) || (start <= upper && upper <= end)
                }) != *negated
            }
        });
        if matches {
            token += 1;
            at += 1;
        } else if let Some((previous, consumed)) = star {
            let next = consumed + 1;
            if next > path.len() {
                return false;
            }
            star = Some((previous, next));
            token = previous + 1;
            at = next;
        } else {
            return false;
        }
    }
    while matches!(tokens.get(token), Some(Token::Star)) {
        token += 1;
    }
    token == tokens.len()
}

/// Question marks, literals, and character classes each consume one character.
/// With one star, match the two fixed-width ends without retrying the suffix.
fn match_token_component(path: &str, tokens: &[Token]) -> Option<bool> {
    let mut chars = path.chars();
    let mut tokens = tokens.iter();
    while let Some(token) = tokens.next() {
        if matches!(token, Token::Star) {
            if tokens.as_slice().iter().any(|t| matches!(t, Token::Star)) {
                return None;
            }
            return Some(
                tokens
                    .rev()
                    .all(|t| chars.next_back().is_some_and(|c| t.matches(c))),
            );
        }
        if !chars.next().is_some_and(|c| token.matches(c)) {
            return Some(false);
        }
    }
    Some(chars.next().is_none())
}

/// A single run of stars between literal affixes has only one possible match
/// for each affix. Compare the borrowed components without compiling a regex.
fn match_literal_components(
    path: &str,
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> Option<bool> {
    // A bracket class can consume a separator. A recursive ** can consume
    // several components, and non-ASCII case folding can change byte lengths.
    if pattern.contains(['[', '?'])
        || (!sensitive && !(path.is_ascii() && pattern.is_ascii()))
        || (recursive && pattern.split(sep).any(|part| part == "**"))
    {
        return None;
    }
    let equal = |a: &[u8], b: &[u8]| {
        if sensitive {
            a == b
        } else {
            a.eq_ignore_ascii_case(b)
        }
    };
    let mut names = path.split(sep);
    for part in pattern.split(sep) {
        let Some(name) = names.next() else {
            return Some(false);
        };
        let name = name.as_bytes();
        if let Some((prefix, tail)) = part.split_once('*') {
            let suffix = tail.trim_start_matches('*');
            if suffix.contains('*') {
                return None;
            }
            if (part == "*" && name.is_empty())
                || name.len() < prefix.len() + suffix.len()
                || !equal(&name[..prefix.len()], prefix.as_bytes())
                || !equal(&name[name.len() - suffix.len()..], suffix.as_bytes())
            {
                return Some(false);
            }
        } else if !equal(name, part.as_bytes()) {
            return Some(false);
        }
    }
    Some(names.next().is_none())
}

/// Compile once for a filesystem selector that checks many names against the same pattern.
pub(super) fn glob_regex(
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> Result<Regex> {
    let slash = || Hir::literal([sep as u8]);
    let mut parts = pattern.split(sep).peekable();
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
                repeat(Hir::dot(Dot::AnyCharExcept(sep)), 1, None)
            } else {
                component(part, sensitive, sep)
            });
            if next.is_some() {
                expression.push(slash());
            }
        }
    }
    expression.push(Hir::look(Look::End));
    Regex::builder()
        .build_from_hir(&Hir::concat(expression))
        .map_err(|e| Error::new(Kind::MemoryError, e.to_string()))
}

/// A normalized anchor is a single part, even for a Windows UNC share. A
/// pattern supplied as a path keeps its own flavor when enumerating its parts.
#[derive(Clone, Copy)]
struct Spelling<'a> {
    text: &'a str,
    components: &'a str,
    anchor: usize,
    sep: char,
}

impl<'a> Spelling<'a> {
    fn of(path: &'a Storage) -> Self {
        let (drive, root) = path.drive_root();
        Self {
            text: path.path(),
            components: path.component_text(),
            anchor: drive.len() + root.len(),
            sep: if path.windows_path() { '\\' } else { '/' },
        }
    }
    fn of_pure(path: &'a PurePath) -> Self {
        Self {
            text: &path.text,
            components: &path.text,
            anchor: path.drive.len() + path.root.len(),
            sep: if path.windows { '\\' } else { '/' },
        }
    }
    fn parts(self) -> impl DoubleEndedIterator<Item = &'a str> {
        let (anchor, tail) = self.components.split_at(self.anchor);
        (!anchor.is_empty())
            .then_some(anchor)
            .into_iter()
            .chain(tail.split(self.sep).filter(|p| !p.is_empty() && *p != "."))
    }
    fn text(self) -> &'a str {
        if self.components == "." {
            ""
        } else {
            self.text
        }
    }
    fn matches(self, q: Self, case: u64, full: bool) -> Result<bool> {
        let sensitive = if case == 0 {
            self.sep == '/'
        } else {
            case == 2
        };
        if full {
            return glob_matches(self.text(), q.text(), sensitive, true, self.sep);
        }
        if q.text().is_empty() {
            return Err(Error::new(Kind::ValueError, "empty pattern"));
        }
        let mut actual = self.parts().rev();
        for pattern in q.parts().rev() {
            let Some(part) = actual.next() else {
                return Ok(false);
            };
            if !glob_matches(part, pattern, sensitive, false, self.sep)? {
                return Ok(false);
            }
        }
        Ok(q.anchor == 0 || actual.next().is_none())
    }
}

/// # Safety
/// Both inputs are rooted. The optional boolean uses 0=None, 1=False, 2=True.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_match(
    path: *const Object,
    pattern: *const Str,
    case: u64,
    full: u32,
) -> u32 {
    // SAFETY: both immutable inputs remain live until matching returns.
    let result = unsafe {
        object(path).0.with(|p| {
            let receiver = Spelling::of(p);
            if p.windows_path() {
                let pattern =
                    PurePath::new(true, vec![crate::StrOwner::acquire_word(pattern as usize)])?;
                receiver.matches(Spelling::of_pure(&pattern), case, full != 0)
            } else {
                let text = normalize(&*pattern);
                let anchor = text.len() - text.trim_start_matches('/').len();
                receiver.matches(
                    Spelling {
                        text: &text,
                        components: &text,
                        anchor,
                        sep: '/',
                    },
                    case,
                    full != 0,
                )
            }
        })
    };
    finish(result.map(u32::from), 0)
}

/// # Safety
/// The caller roots both path objects; neither can run callbacks during matching.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_path_match_object(
    path: *const Object,
    pattern: *const Object,
    case: u64,
    full: u32,
) -> u32 {
    // SAFETY: immutable inputs can alias and no borrowed spelling escapes.
    let result = unsafe {
        object(path).0.with(|p| {
            object(pattern)
                .0
                .with(|q| Spelling::of(p).matches(Spelling::of(q), case, full != 0))
        })
    };
    finish(result.map(u32::from), 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_components_agree_with_glob_regex() {
        let paths = [
            "",
            "/",
            "x",
            "/x",
            "a/x",
            "/a/x",
            "a/b/x",
            "a",
            "a/b",
            "a/B",
            "abc.bin",
            "a/b/c.bin",
            "é/雪.txt",
        ];
        let patterns = [
            "**",
            "**/x",
            "**/**/x",
            "a/**",
            "a/**/x",
            "*/x",
            "a/*",
            "a/?",
            "a/[A-Z]",
            "a/[!x]b",
            "**/*.bin",
            "a*b*c.bin",
            "é/**/[雪].txt",
        ];
        for path in paths {
            for pattern in patterns {
                for sensitive in [true, false] {
                    let expected = glob_regex(pattern, sensitive, true, '/')
                        .ok()
                        .unwrap()
                        .is_match(path);
                    assert_eq!(
                        glob_matches(path, pattern, sensitive, true, '/')
                            .ok()
                            .unwrap(),
                        expected,
                        "{path:?}, {pattern:?}, sensitive={sensitive}"
                    );
                }
            }
        }
    }

    #[test]
    fn multi_star_component_and_windows_case() {
        assert!(
            glob_matches("test_config.py", "t*[a-z]*f*.py", true, false, '/')
                .ok()
                .unwrap()
        );
        assert!(
            glob_matches("TEST_CONFIG.PY", "t*[a-z]*f*.py", false, false, '\\')
                .ok()
                .unwrap()
        );
        assert!(
            !glob_matches("testconfig.py", "t*[!a-z]*f*.py", true, false, '/')
                .ok()
                .unwrap()
        );
    }
}
