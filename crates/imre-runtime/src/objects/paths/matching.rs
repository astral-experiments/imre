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

// Keep token compilation out of the literal-component loop.
#[inline(never)]
fn complex_glob_matches(
    path: &str,
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> Result<bool> {
    Ok(match_components(path, pattern, sensitive, recursive, sep))
}

/// Match recursive components directly, using flat states when a character class can cross separators.
fn match_components(
    path: &str,
    pattern: &str,
    sensitive: bool,
    recursive: bool,
    sep: char,
) -> bool {
    if pattern.contains('[') && pattern.split(sep).any(|part| {
        let Some(first) = part.find('[') else { return false };
        matches!(Pattern::component(&part[first..]), Pattern::Wildcard(tokens) if tokens.iter().any(|token| {
            matches!(token, Token::Class { .. }) && token.matches(sep)
        }))
    }) {
        return match_crossing(path, pattern, sensitive, recursive, sep);
    }

    let (mut at, mut token) = (0, 0);
    let mut star = None;
    loop {
        let tail = &pattern[token..];
        let end = tail.find(sep).map_or(pattern.len(), |end| token + end);
        let part = &pattern[token..end];
        if recursive && part == "**" {
            if end == pattern.len() {
                return true;
            }
            token = end + sep.len_utf8();
            star = Some((token, at, at));
            continue;
        }
        let name_end = path[at..].find(sep).map_or(path.len(), |end| at + end);
        if match_component(&path[at..name_end], part, sensitive) && (part != "*" || name_end > at) {
            if end == pattern.len() && name_end == path.len() {
                return true;
            }
            if end < pattern.len() && name_end < path.len() {
                token = end + sep.len_utf8();
                at = name_end + sep.len_utf8();
                continue;
            }
        }
        let Some((retry, start, search)) = star.as_mut() else {
            return false;
        };
        let mut rest = &path[*search..];
        loop {
            let Some(offset) = rest.find(sep) else {
                return false;
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
        Pattern::Literal(text) => folded_equal(path.chars(), text.chars()),
        Pattern::Affixes { text, stars } => {
            let mut chars = path.chars();
            text[..stars.start]
                .chars()
                .all(|expected| chars.next().is_some_and(|actual| folded(actual, expected)))
                && text[stars.end..].chars().rev().all(|expected| {
                    chars
                        .next_back()
                        .is_some_and(|actual| folded(actual, expected))
                })
        }
        Pattern::Wildcard(tokens) => match_folded_tokens(path, &tokens),
    }
}

fn folded(actual: char, expected: char) -> bool {
    actual == expected
        || (actual.is_ascii() && expected.is_ascii() && actual.eq_ignore_ascii_case(&expected))
        || super::super::warnings::literal_fold(actual)
            == super::super::warnings::literal_fold(expected)
}

fn folded_equal(
    mut actual: impl Iterator<Item = char>,
    mut expected: impl Iterator<Item = char>,
) -> bool {
    loop {
        match (actual.next(), expected.next()) {
            (Some(actual), Some(expected)) if folded(actual, expected) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn folded_class(character: char, negated: bool, ranges: &[(char, char)]) -> bool {
    let contains = |c| ranges.iter().any(|&(start, end)| start <= c && c <= end);
    let matched = if contains(character) {
        true
    } else if ranges
        .iter()
        .all(|&(start, end)| start.is_ascii() && end.is_ascii())
    {
        let folding = super::super::warnings::literal_fold(character);
        char::from_u32(folding)
            .is_some_and(|c| c.is_ascii() && (contains(c) || contains(c.to_ascii_uppercase())))
    } else {
        let mut equivalents = ClassUnicode::new([ClassUnicodeRange::new(character, character)]);
        if matches!(character, 'i' | 'I' | 'İ' | 'ı') {
            for c in ['i', 'I', 'İ', 'ı'] {
                equivalents.push(ClassUnicodeRange::new(c, c));
            }
        }
        equivalents.case_fold_simple();
        equivalents.ranges().iter().any(|equivalent| {
            ranges
                .iter()
                .any(|&(start, end)| equivalent.start() <= end && start <= equivalent.end())
        })
    };
    matched != negated
}

fn token_matches(token: &Token, character: char, sensitive: bool) -> bool {
    if sensitive {
        return token.matches(character);
    }
    match token {
        Token::Literal(expected) => folded(character, *expected),
        Token::Any => true,
        Token::Star => false,
        Token::Class { negated, ranges } => folded_class(character, *negated, ranges),
    }
}

fn match_folded_tokens(path: &str, tokens: &[Token]) -> bool {
    let (mut at, mut token) = (0, 0);
    let mut star = None;
    while at < path.len() {
        if matches!(tokens.get(token), Some(Token::Star)) {
            star = Some((token, at));
            token += 1;
            continue;
        }
        let character = path[at..].chars().next().unwrap();
        if tokens
            .get(token)
            .is_some_and(|t| token_matches(t, character, false))
        {
            token += 1;
            at += character.len_utf8();
        } else if let Some((previous, consumed)) = star {
            let next = consumed + path[consumed..].chars().next().unwrap().len_utf8();
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

/// A recursive wildcard followed by a separator has an empty branch and a branch
/// that consumes at least one character before the separator. Keep those two states
/// beside ordinary shell tokens so a class can match a path separator.
enum GlobToken {
    Shell(Token),
    RecursiveStart,
    RecursiveBody,
    RecursiveTail,
}

fn append_component(tokens: &mut Vec<GlobToken>, part: &str, whole_component: bool) {
    match Pattern::component(part) {
        Pattern::Literal(text) => {
            tokens.extend(text.chars().map(|c| GlobToken::Shell(Token::Literal(c))))
        }
        Pattern::Affixes { text, stars } => {
            tokens.extend(
                text[..stars.start]
                    .chars()
                    .map(|c| GlobToken::Shell(Token::Literal(c))),
            );
            if part == "*" && whole_component {
                tokens.push(GlobToken::Shell(Token::Any));
            }
            tokens.push(GlobToken::Shell(Token::Star));
            tokens.extend(
                text[stars.end..]
                    .chars()
                    .map(|c| GlobToken::Shell(Token::Literal(c))),
            );
        }
        Pattern::Wildcard(shell) => tokens.extend(shell.into_iter().map(GlobToken::Shell)),
    }
}

/// Add one state and its deterministic epsilon successors once for this generation.
fn activate_state(
    mut index: usize,
    tokens: &[GlobToken],
    states: &mut Vec<usize>,
    marks: &mut [usize],
    generation: usize,
) {
    loop {
        if marks[index] == generation {
            return;
        }
        marks[index] = generation;
        states.push(index);
        match tokens.get(index) {
            Some(GlobToken::Shell(Token::Star) | GlobToken::RecursiveTail) => index += 1,
            Some(GlobToken::RecursiveStart) => index += 2,
            _ => return,
        }
    }
}

/// Store active token positions in flat arrays. Generation marks suppress duplicate
/// states without clearing a pattern-sized array for every input character.
fn match_crossing(path: &str, pattern: &str, sensitive: bool, recursive: bool, sep: char) -> bool {
    let first = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    let (prefix, pattern) = pattern.split_at(first);
    let path = if sensitive {
        path.strip_prefix(prefix)
    } else {
        let mut actual = path.chars();
        prefix
            .chars()
            .all(|expected| actual.next().is_some_and(|c| folded(c, expected)))
            .then_some(actual.as_str())
    };
    let Some(path) = path else { return false };
    let mut first_component = true;
    let mut tokens = Vec::new();
    let mut parts = pattern.split(sep).peekable();
    while let Some(part) = parts.next() {
        let whole = !first_component || prefix.is_empty() || prefix.ends_with(sep);
        if recursive && part == "**" && whole {
            if parts.peek().is_some() {
                tokens.extend([GlobToken::RecursiveStart, GlobToken::RecursiveBody]);
            } else {
                tokens.push(GlobToken::RecursiveTail);
            }
        } else {
            append_component(&mut tokens, part, whole);
            if parts.peek().is_some() {
                tokens.push(GlobToken::Shell(Token::Literal(sep)));
            }
        }
        first_component = false;
    }
    let mut marks = vec![0; tokens.len() + 1];
    let mut current = Vec::new();
    let mut next = Vec::new();
    let mut generation = 1;
    activate_state(0, &tokens, &mut current, &mut marks, generation);
    for (index, character) in path.chars().enumerate() {
        generation = index + 2;
        next.clear();
        for &state in &current {
            let Some(token) = tokens.get(state) else {
                continue;
            };
            let mut add = |index| activate_state(index, &tokens, &mut next, &mut marks, generation);
            match token {
                GlobToken::Shell(Token::Star) if character != sep => add(state),
                GlobToken::Shell(Token::Any) if character != sep => add(state + 1),
                GlobToken::Shell(Token::Star | Token::Any) => {}
                GlobToken::Shell(token) if token_matches(token, character, sensitive) => {
                    add(state + 1)
                }
                GlobToken::Shell(_) => {}
                GlobToken::RecursiveStart => add(state + 1),
                GlobToken::RecursiveBody => {
                    add(state);
                    if character == sep {
                        add(state + 1);
                    }
                }
                GlobToken::RecursiveTail => add(state),
            }
        }
        if next.is_empty() {
            return false;
        }
        std::mem::swap(&mut current, &mut next);
    }
    marks[tokens.len()] == generation
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
            "src/a/b",
            "a/b",
            "//a",
            "/a",
            "docs/RÉSUMÉ_2026.TXT",
            "src/Kelvin.txt",
            "src/İstanbul.txt",
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
            "**/a[!x]b",
            "**/**/a[!x]b",
            "**/a",
            "docs/résumé*.txt",
            "src/[A-Z]*.TXT",
            "src/[K]*.TXT",
            "src/[İ]*.TXT",
            "a[.-0]b",
            "*[!x]*b",
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

    #[test]
    fn crossing_patterns_have_no_token_limit() {
        for width in [128, 256, 512] {
            let prefix = "a".repeat(width);
            let path = format!("{prefix}/b");
            for pattern in [format!("{prefix}[!x]b"), format!("{prefix}*[!x]*b")] {
                assert!(glob_matches(&path, &pattern, true, true, '/').ok().unwrap());
            }
        }
        assert!(
            glob_matches("foo/x", "foo*/[!a]", true, true, '/')
                .ok()
                .unwrap()
        );
        assert!(
            !glob_matches("foo/bar/x", "foo**/[!a]", true, true, '/')
                .ok()
                .unwrap()
        );
    }
}
